//! Cloud SQL Auth Proxy supervision for the Database tool.
//!
//! GCP Cloud SQL instances are normally reached through Google's
//! `cloud-sql-proxy`, which authenticates with a service-account key (or
//! Application Default Credentials) and exposes the instance as a plain
//! TCP listener on loopback. The Database tool can start and stop that proxy
//! itself, so nothing about it is hard-coded: the credentials file is a user
//! preference ([`crate::prefs`]) and the instance connection name is a field of
//! the connection form.
//!
//! The binary itself is *installed*, not configured. The setup helper in
//! `scripts/` — shipped in the package as `frms-setup-cloud-sql-proxy`, the
//! same arrangement as `frms-setup-claude` — downloads the right build for the
//! machine, verifies its published SHA-256, and puts it on `PATH`.
//! [`detect_binary`] then finds it. Where it lives is never asked of the user
//! and never shown to them — the only proxy-binary UI is an Install button,
//! and only when there is nothing installed to find.
//!
//! The child is *managed* — frms spawns it, tails its stderr for the readiness
//! line, and kills it on Stop and on app shutdown, so quitting frms never
//! leaves a stray proxy holding a port.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long to wait for the proxy to announce itself before giving up and
/// killing it. A cold start has to reach Google's auth endpoint, so this is
/// generous by local-process standards.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Lines of the child's stderr kept for the panel's status/error text.
const MAX_LOG_LINES: usize = 200;

/// The line v2 of the proxy prints once it is accepting connections. Matched
/// case-insensitively; v1 printed "Ready for new connections" with the same
/// tail, so one substring covers both.
const READY_MARKER: &str = "ready for new connections";

/// The address the proxy is always started on. A Cloud SQL instance reached
/// through frms is for this machine only — binding anything routable is not a
/// choice the panel offers.
pub const PROXY_ADDRESS: &str = "127.0.0.1";

// ── spec ──────────────────────────────────────────────────────────────────────

/// Everything needed to launch one proxy. Assembled from prefs (the two paths)
/// plus the connection form (instance, address, port).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    /// Explicit path to the `cloud-sql-proxy` executable. Always empty in the
    /// app — "use the installed one", resolved by [`detect_binary`] at spawn
    /// time. It exists as a seam so the tests can point a spawn at a stand-in
    /// binary; there is no user-facing setting behind it.
    pub binary:      String,
    /// Service-account key (`auth.json`). Empty means the proxy falls back to
    /// Application Default Credentials (`gcloud auth application-default login`).
    pub credentials: String,
    /// Instance connection name — `project:region:instance`.
    pub instance:    String,
    /// Loopback address the proxy listens on.
    pub address:     String,
    /// Local port the proxy listens on — the same port the connection form
    /// then dials.
    pub port:        u16,
}

impl ProxySpec {
    /// The command line this spec runs, for display in the panel so the user
    /// can see exactly what was launched (and copy it into a shell).
    pub fn command_line(&self) -> String {
        let (program, args) = match self.command_parts() {
            Ok(v)  => v,
            Err(_) => (PathBuf::from("cloud-sql-proxy"), self.args()),
        };
        std::iter::once(program.to_string_lossy().into_owned())
            .chain(args)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Resolved program plus arguments, or a user-facing error explaining what
    /// is missing.
    fn command_parts(&self) -> Result<(PathBuf, Vec<String>), String> {
        Ok((resolve_binary(&self.binary)?, self.args()))
    }

    fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let creds = self.credentials.trim();
        if !creds.is_empty() {
            args.push(String::from("--credentials-file"));
            args.push(expand_home(creds).to_string_lossy().into_owned());
        }
        args.push(String::from("--address"));
        args.push(self.address.clone());
        args.push(String::from("--port"));
        args.push(self.port.to_string());
        args.push(self.instance.trim().to_string());
        args
    }

    /// Reject a spec that can't produce a working proxy before we spawn
    /// anything, so the panel shows a precise message instead of the child's.
    fn validate(&self) -> Result<(), String> {
        if self.instance.trim().is_empty() {
            return Err(String::from(
                "Instance connection name is required (project:region:instance).",
            ));
        }
        if self.instance.matches(':').count() != 2 {
            return Err(format!(
                "\"{}\" is not an instance connection name — expected project:region:instance.",
                self.instance.trim()
            ));
        }
        let creds = self.credentials.trim();
        if !creds.is_empty() && !expand_home(creds).is_file() {
            return Err(format!("Credentials file not found: {creds}"));
        }
        if self.port == 0 {
            return Err(String::from("Local port is required."));
        }
        Ok(())
    }
}

// ── binary discovery ──────────────────────────────────────────────────────────

/// Turn a [`ProxySpec::binary`] into something spawnable.
///
/// An explicit path (`~` expanded) must exist; the empty string — what the app
/// always passes — defers to [`detect_binary`]. Anything else is an error
/// pointing at the installer, since a proxy that was never installed is the
/// most likely first-run failure.
fn resolve_binary(configured: &str) -> Result<PathBuf, String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = expand_home(configured);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("Cloud SQL Proxy not found at {configured}"));
    }
    if let Some(found) = detect_binary() {
        return Ok(found);
    }
    Err(String::from(
        "cloud-sql-proxy is not installed — press Install, or run \
         frms-setup-cloud-sql-proxy in a shell",
    ))
}

/// Where [`setup_command`] installs the proxy, in the order it prefers. Probed
/// directly rather than trusting `PATH`, because a desktop-launched frms
/// inherits the session environment — no login shell, so no `~/.local/bin`.
#[cfg(not(windows))]
const INSTALL_DIRS: [&str; 2] = ["/usr/local/bin", "/usr/bin"];
#[cfg(windows)]
const INSTALL_DIRS: [&str; 0] = [];

/// Best guess at where the proxy already lives, used as the fallback when no
/// path is configured and to tell the panel whether an install is needed.
///
/// Checks `PATH` first, then the locations
/// `scripts/setup-cloud-sql-proxy.sh` installs to, then a source checkout's
/// `scripts/` — so a developer running from the repo and a user who ran the
/// packaged installer both get a working proxy with nothing to configure.
pub fn detect_binary() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "cloud-sql-proxy.exe" } else { "cloud-sql-proxy" };
    if crate::port::shell::on_path("cloud-sql-proxy") {
        return Some(PathBuf::from("cloud-sql-proxy"));
    }
    let mut candidates: Vec<PathBuf> =
        INSTALL_DIRS.iter().map(|d| PathBuf::from(d).join(exe)).collect();
    if let Some(home) = crate::port::dirs::home() {
        candidates.push(home.join(".local").join("bin").join(exe));
        candidates.push(home.join("bin").join(exe));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts").join(exe));
        candidates.push(cwd.join(exe));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// The command frms' Install button runs to fetch the proxy — the packaged
/// `frms-setup-cloud-sql-proxy` helper, or the same script straight out of a
/// source checkout when frms is being run from one.
///
/// The script is the single source of truth for the install (version
/// resolution, checksum, location); the app only decides how to reach it.
pub fn setup_command() -> String {
    const PACKAGED: &str = "frms-setup-cloud-sql-proxy";
    if crate::port::shell::on_path(PACKAGED) {
        return String::from(PACKAGED);
    }
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join("scripts").join("setup-cloud-sql-proxy.sh");
        if local.is_file() {
            return local.to_string_lossy().into_owned();
        }
    }
    // Not installed anywhere we can see: name it anyway, so the terminal's
    // "command not found" says what to go looking for.
    String::from(PACKAGED)
}

/// Expand a leading `~` against the user's home directory. Paths are typed by
/// hand in the panel, so `~/dev/tsr-back-end/auth.json` has to work.
fn expand_home(raw: &str) -> PathBuf {
    let raw = raw.trim();
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = crate::port::dirs::home() {
            return home.join(rest);
        }
    }
    if raw == "~" {
        if let Some(home) = crate::port::dirs::home() {
            return home;
        }
    }
    PathBuf::from(raw)
}

// ── running proxy ─────────────────────────────────────────────────────────────

/// A live proxy child. Cloneable because it travels inside `Message`; every
/// clone shares the one child handle and log buffer.
#[derive(Clone)]
pub struct Proxy {
    pub spec: ProxySpec,
    pub pid:  u32,
    child:    Arc<Mutex<Child>>,
    log:      Arc<Mutex<Vec<String>>>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<cloud-sql-proxy pid {} → {}>", self.pid, self.spec.instance)
    }
}

impl Proxy {
    /// Where the connection form should point while this proxy is up.
    pub fn endpoint(&self) -> (String, u16) {
        (self.spec.address.clone(), self.spec.port)
    }

    /// Whether the child is still running. Reaps it if it has exited, so a
    /// crashed proxy doesn't linger as a zombie.
    pub fn is_alive(&self) -> bool {
        let Ok(mut child) = self.child.lock() else { return false; };
        matches!(child.try_wait(), Ok(None))
    }

    /// Stop the proxy and reap it. Safe to call on an already-dead child.
    pub fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    /// The last few stderr lines, for the panel's error text.
    pub fn log_tail(&self, lines: usize) -> String {
        let Ok(log) = self.log.lock() else { return String::new(); };
        let start = log.len().saturating_sub(lines);
        log[start..].join("\n")
    }
}

// ── start ─────────────────────────────────────────────────────────────────────

/// Spawn the proxy and wait until it reports itself ready.
///
/// Resolves only once the listener is up (or the attempt has failed), so the
/// caller can connect straight afterwards without racing the proxy's startup.
pub async fn start(spec: ProxySpec) -> Result<Proxy, String> {
    spec.validate()?;
    let (program, args) = spec.command_parts()?;

    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Could not start {}: {e}", program.to_string_lossy()))?;
    let pid = child.id();

    // The proxy logs everything — including the readiness line — to stderr.
    let stderr = child.stderr.take();
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    if let Some(stderr) = stderr {
        let log = Arc::clone(&log);
        // A blocking line reader rather than tokio::process so the child handle
        // stays a plain `std::process::Child`, killable from `shutdown()` with
        // no runtime in scope — the same shape the PTY panes use.
        tokio::task::spawn_blocking(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut log) = log.lock() {
                    log.push(line.clone());
                    if log.len() > MAX_LOG_LINES {
                        let overflow = log.len() - MAX_LOG_LINES;
                        log.drain(..overflow);
                    }
                }
                // Send failures just mean the starter has moved on; keep
                // draining so the pipe never fills and blocks the child.
                let _ = tx.send(line);
            }
        });
    }

    let proxy = Proxy {
        spec,
        pid,
        child: Arc::new(Mutex::new(child)),
        log: Arc::clone(&log),
    };

    let ready = tokio::time::timeout(READY_TIMEOUT, async {
        while let Some(line) = rx.recv().await {
            if line.to_ascii_lowercase().contains(READY_MARKER) {
                return true;
            }
        }
        // Channel closed → stderr hit EOF → the child is gone.
        false
    })
    .await;

    match ready {
        Ok(true) => Ok(proxy),
        Ok(false) => {
            proxy.kill();
            Err(match proxy.log_tail(6).trim() {
                ""   => String::from("The Cloud SQL Proxy exited without starting."),
                tail => format!("The Cloud SQL Proxy exited without starting:\n{tail}"),
            })
        }
        Err(_) => {
            proxy.kill();
            Err(match proxy.log_tail(6).trim() {
                ""   => String::from("Timed out waiting for the Cloud SQL Proxy to start."),
                tail => format!("Timed out waiting for the Cloud SQL Proxy to start:\n{tail}"),
            })
        }
    }
}

// ── panel state ───────────────────────────────────────────────────────────────

/// Where the Database tool's proxy currently stands. Mirrors
/// [`crate::db_panel::ConnState`] so the section reads the same way as the
/// connection banner above it.
#[derive(Debug, Clone, Default)]
pub enum ProxyState {
    #[default]
    Stopped,
    Starting,
    Running(Proxy),
    Failed(String),
}

impl ProxyState {
    pub fn running(&self) -> Option<&Proxy> {
        match self {
            ProxyState::Running(p) => Some(p),
            _ => None,
        }
    }

    pub fn is_starting(&self) -> bool {
        matches!(self, ProxyState::Starting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ProxySpec {
        ProxySpec {
            binary:      String::from("/opt/cloud-sql-proxy"),
            credentials: String::new(),
            instance:    String::from("proj:us-west2:inst"),
            address:     String::from("127.0.0.1"),
            port:        3306,
        }
    }

    #[test]
    fn args_carry_address_port_and_instance() {
        assert_eq!(
            spec().args(),
            vec!["--address", "127.0.0.1", "--port", "3306", "proj:us-west2:inst"],
        );
    }

    #[test]
    fn credentials_file_is_passed_only_when_set() {
        let mut s = spec();
        s.credentials = String::from("/keys/auth.json");
        let args = s.args();
        assert_eq!(&args[..2], &["--credentials-file", "/keys/auth.json"]);
        // Blank means Application Default Credentials — no flag at all.
        s.credentials = String::from("   ");
        assert!(!s.args().iter().any(|a| a == "--credentials-file"));
    }

    #[test]
    fn validate_rejects_a_malformed_instance_name() {
        let mut s = spec();
        s.instance = String::from("just-a-project");
        assert!(s.validate().unwrap_err().contains("project:region:instance"));

        s.instance = String::new();
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_a_missing_credentials_file() {
        let mut s = spec();
        s.credentials = String::from("/nope/definitely/not/here/auth.json");
        assert!(s.validate().unwrap_err().starts_with("Credentials file not found"));
    }

    #[test]
    fn the_install_helper_is_named_so_a_shell_can_find_it() {
        // Either the packaged command or this checkout's copy of the script —
        // never an empty string or some other binary, whichever machine the
        // tests run on.
        let cmd = setup_command();
        assert!(
            cmd == "frms-setup-cloud-sql-proxy" || cmd.ends_with("setup-cloud-sql-proxy.sh"),
            "unexpected install command: {cmd}",
        );
    }

    #[test]
    fn a_missing_configured_binary_names_the_path_it_looked_at() {
        let err = resolve_binary("/nope/cloud-sql-proxy").unwrap_err();
        assert!(err.contains("/nope/cloud-sql-proxy"));
    }

    // ── the spawn/readiness plumbing, against a stand-in for the real proxy ──

    /// Write an executable shell script that prints `stderr_lines` to stderr,
    /// then either lingers or exits. Stands in for `cloud-sql-proxy` so the
    /// start path can be exercised without Google credentials.
    #[cfg(unix)]
    fn fake_proxy(name: &str, stderr_lines: &[&str], linger: bool) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("frms-fake-proxy-{name}"));
        let echoes: String = stderr_lines
            .iter()
            .map(|l| format!("echo '{l}' >&2\n"))
            .collect();
        let tail = if linger { "sleep 20\n" } else { "exit 1\n" };
        std::fs::write(&path, format!("#!/bin/sh\n{echoes}{tail}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A runtime the test keeps alive for its whole body. Dropping one joins
    /// the stderr reader, which only ends when the child does — so the proxy
    /// must be killed before the runtime goes out of scope.
    #[cfg(unix)]
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn start_resolves_once_the_proxy_announces_itself() {
        let rt  = runtime();
        let bin = fake_proxy(
            "ready",
            &[
                "Authorizing with the credentials file",
                "The proxy has started successfully and is ready for new connections!",
            ],
            true,
        );
        let mut s = spec();
        s.binary = bin.to_string_lossy().into_owned();

        let proxy = rt.block_on(start(s)).expect("the fake proxy reports itself ready");
        assert!(proxy.is_alive());
        assert_eq!(proxy.endpoint(), (String::from("127.0.0.1"), 3306));
        proxy.kill();
        assert!(!proxy.is_alive(), "kill takes the child down");
        let _ = std::fs::remove_file(&bin);
    }

    #[cfg(unix)]
    #[test]
    fn start_fails_with_the_proxy_own_words_when_it_exits_early() {
        let rt  = runtime();
        let bin = fake_proxy("early-exit", &["failed to connect to instance: permission denied"], false);
        let mut s = spec();
        s.binary = bin.to_string_lossy().into_owned();

        let err = rt.block_on(start(s)).unwrap_err();
        assert!(err.contains("permission denied"), "the child's own error is surfaced: {err}");
        let _ = std::fs::remove_file(&bin);
    }
}
