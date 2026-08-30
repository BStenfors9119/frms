//! Project document index — the `_specs` and `_docs` folders of a session's
//! working directory, searchable by name and by modified date.
//!
//! These two folders are where a project's written material lives (specs,
//! design notes, runbooks), and they grow past the point where scrolling the
//! file browser to them is pleasant. This module keeps a flat index of both,
//! which the Docs toolbox tab filters live as the user types.
//!
//! The index is a snapshot, taken on demand ([`DocsState::rescan`]) rather than
//! watched: scanning is cheap next to the cost of a filesystem watcher, and the
//! tab rescans whenever it comes on screen, so what's shown is current every
//! time the user goes looking. Filtering never touches the disk — it runs over
//! the snapshot, so typing stays instant.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The project-relative folders scanned for documents, in listing order.
pub const DOC_DIRS: &[&str] = &["_specs", "_docs"];

/// How deep into a doc folder the scan descends. Deep enough for the nested
/// grouping these folders actually use (`_specs/2026/q3/…`), shallow enough
/// that a stray `node_modules` symlinked in can't turn the scan into a crawl.
const MAX_DEPTH: usize = 6;

/// Ceiling on indexed files. A doc folder this large is a misconfiguration
/// (something checked a build tree into `_docs`), and the list stops being
/// useful long before it — but the scan still has to end promptly.
const MAX_DOCS: usize = 2000;

/// One indexed document.
#[derive(Clone, Debug)]
pub struct Doc {
    /// File name, e.g. `TELEMETRY_NOTICE.md`.
    pub name: String,
    /// Absolute path — what [`crate::app::Message::OpenFile`] takes.
    pub path: PathBuf,
    /// Path relative to the project root, e.g. `_docs/TELEMETRY_NOTICE.md`.
    /// Shown under the name and searched alongside it, so a query can name a
    /// subfolder ("api notes") as well as a file.
    pub rel: String,
    /// Which of [`DOC_DIRS`] this came from — the list's group heading.
    pub root: &'static str,
    /// Last-modified time. `None` when the platform or filesystem doesn't
    /// report one; such a file is listed but never matches a date filter
    /// narrower than [`DateFilter::Any`].
    pub modified: Option<SystemTime>,
}

/// How far back a document's modified time may be for it to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFilter {
    Any,
    Today,
    Week,
    Month,
    Quarter,
    Year,
}

impl DateFilter {
    pub const ALL: &'static [DateFilter] = &[
        DateFilter::Any,
        DateFilter::Today,
        DateFilter::Week,
        DateFilter::Month,
        DateFilter::Quarter,
        DateFilter::Year,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateFilter::Any     => "Any date",
            DateFilter::Today   => "Today",
            DateFilter::Week    => "Last 7 days",
            DateFilter::Month   => "Last 30 days",
            DateFilter::Quarter => "Last 90 days",
            DateFilter::Year    => "Last year",
        }
    }

    /// The cutoff age, or `None` for [`DateFilter::Any`] (no cutoff).
    fn max_age(self) -> Option<Duration> {
        let days = match self {
            DateFilter::Any     => return None,
            DateFilter::Today   => 1,
            DateFilter::Week    => 7,
            DateFilter::Month   => 30,
            DateFilter::Quarter => 90,
            DateFilter::Year    => 365,
        };
        Some(Duration::from_secs(days * 24 * 60 * 60))
    }

    /// Whether a document last modified at `modified` passes this filter, as of
    /// `now`. A file with no modified time passes only `Any`; one modified in
    /// the future (clock skew, a checkout that stamped ahead) counts as brand
    /// new and passes everything.
    pub fn matches(self, modified: Option<SystemTime>, now: SystemTime) -> bool {
        let Some(max_age) = self.max_age() else { return true };
        let Some(modified) = modified else { return false };
        match now.duration_since(modified) {
            Ok(age) => age <= max_age,
            Err(_)  => true, // modified in the future — treat as just now
        }
    }
}

impl std::fmt::Display for DateFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Result ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    /// Alphabetical by relative path — stable, so the list doesn't reshuffle
    /// under the user as they edit files.
    Name,
    /// Most recently modified first. Files with no modified time sort last.
    Newest,
}

impl SortBy {
    pub const ALL: &'static [SortBy] = &[SortBy::Name, SortBy::Newest];

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Name   => "Name",
            SortBy::Newest => "Newest",
        }
    }
}

impl std::fmt::Display for SortBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Per-session state for the Docs tab: the search controls plus the last scan.
pub struct DocsState {
    /// Name/path search text. Empty matches everything.
    pub query: String,
    pub date:  DateFilter,
    pub sort:  SortBy,
    /// The last scan's results, in [`DOC_DIRS`] order then alphabetical.
    pub docs:  Vec<Doc>,
    /// Which of [`DOC_DIRS`] actually exist under the project root. Drives the
    /// empty-state message, so "no results" can distinguish an over-narrow
    /// search from a project with no doc folders at all.
    pub roots: Vec<&'static str>,
    /// True once a scan has run — before that the tab shows nothing rather
    /// than claiming the project has no documents.
    pub scanned: bool,
    /// True when the scan stopped at [`MAX_DOCS`], so the tab can say the
    /// listing is partial instead of silently truncating.
    pub truncated: bool,
}

impl Default for DocsState {
    fn default() -> Self {
        Self {
            query:     String::new(),
            date:      DateFilter::Any,
            sort:      SortBy::Name,
            docs:      Vec::new(),
            roots:     Vec::new(),
            scanned:   false,
            truncated: false,
        }
    }
}

impl DocsState {
    /// Re-read `_specs` and `_docs` under `root`, replacing the index. Search
    /// controls are left alone so a rescan doesn't throw away what the user is
    /// in the middle of typing.
    pub fn rescan(&mut self, root: &Path) {
        let mut docs = Vec::new();
        let mut roots = Vec::new();
        let mut truncated = false;

        for dir in DOC_DIRS {
            let path = root.join(dir);
            if !path.is_dir() {
                continue;
            }
            roots.push(*dir);
            truncated |= collect(&path, dir, root, 0, &mut docs);
        }

        // Group order follows DOC_DIRS, then alphabetical within each group.
        // `sort_results` re-sorts the filtered view; this ordering is what a
        // Name sort settles on and what Newest falls back to for ties.
        docs.sort_by(|a, b| {
            let ai = DOC_DIRS.iter().position(|d| d == &a.root);
            let bi = DOC_DIRS.iter().position(|d| d == &b.root);
            ai.cmp(&bi).then_with(|| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()))
        });

        self.docs      = docs;
        self.roots     = roots;
        self.truncated = truncated;
        self.scanned   = true;
    }

    /// The documents matching the current query and date filter, in the
    /// current sort order. `now` is passed in rather than read here so the
    /// filtering is testable.
    pub fn results(&self, now: SystemTime) -> Vec<&Doc> {
        let terms = query_terms(&self.query);
        let mut out: Vec<&Doc> = self.docs.iter()
            .filter(|d| matches_terms(d, &terms))
            .filter(|d| self.date.matches(d.modified, now))
            .collect();
        sort_results(&mut out, self.sort);
        out
    }

    /// Whether any search control is narrowing the list — drives the "Clear"
    /// button and the wording of the empty state.
    pub fn is_filtered(&self) -> bool {
        !self.query.trim().is_empty() || self.date != DateFilter::Any
    }

    /// Reset the query and date filter, keeping the sort order (which is a
    /// display preference, not a filter).
    pub fn clear(&mut self) {
        self.query.clear();
        self.date = DateFilter::Any;
    }
}

fn sort_results(docs: &mut [&Doc], sort: SortBy) {
    match sort {
        // Already in DOC_DIRS-then-alphabetical order from the scan.
        SortBy::Name => {}
        // Newest first. A file with no modified time has nothing to sort on, so
        // it sinks below every dated one rather than floating to the top.
        SortBy::Newest => docs.sort_by(|a, b| {
            b.modified.cmp(&a.modified)
                .then_with(|| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()))
        }),
    }
}

/// Split a query into lower-cased terms. Whitespace-separated, all of which
/// must match — so "telemetry backend" finds `_docs/TELEMETRY_BACKEND.md`
/// without the user having to reproduce the separator.
fn query_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether every term appears somewhere in the document's relative path. The
/// path (not just the file name) is searched so a folder name narrows the list.
fn matches_terms(doc: &Doc, terms: &[String]) -> bool {
    if terms.is_empty() {
        return true;
    }
    let hay = doc.rel.to_lowercase();
    terms.iter().all(|t| hay.contains(t.as_str()))
}

/// Recursively index files under `dir`. Returns true if the [`MAX_DOCS`] cap
/// was hit, meaning the listing is incomplete.
fn collect(
    dir:   &Path,
    root_label: &'static str,
    project: &Path,
    depth: usize,
    out:   &mut Vec<Doc>,
) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        // An unreadable subfolder is skipped rather than failing the scan —
        // the rest of the index is still worth showing.
        return false;
    };

    let mut truncated = false;
    for entry in entries.flatten() {
        if out.len() >= MAX_DOCS {
            return true;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Hidden entries are editor scratch and VCS bookkeeping, not documents.
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            truncated |= collect(&path, root_label, project, depth + 1, out);
            continue;
        }

        let rel = path.strip_prefix(project)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/"); // display the same on Windows as elsewhere
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
        out.push(Doc { name, path, rel, root: root_label, modified });
    }
    truncated
}

/// Compact relative age for a document, e.g. "today", "3d ago", "2mo ago".
/// Deliberately coarse: the exact minute a spec was touched is noise next to
/// "this week or last quarter?", which is the question the date filter answers.
pub fn age_label(modified: Option<SystemTime>, now: SystemTime) -> String {
    let Some(modified) = modified else { return String::from("—") };
    let Ok(age) = now.duration_since(modified) else {
        return String::from("today"); // future stamp — clock skew
    };
    let days = age.as_secs() / (24 * 60 * 60);
    match days {
        0          => "today".to_string(),
        1          => "1d ago".to_string(),
        2..=6      => format!("{days}d ago"),
        7..=13     => "1w ago".to_string(),
        14..=59    => format!("{}w ago", days / 7),
        60..=364   => format!("{}mo ago", days / 30),
        365..=729  => "1y ago".to_string(),
        _          => format!("{}y ago", days / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(rel: &str, days_old: Option<u64>) -> Doc {
        let modified = days_old.map(|d| {
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000 - d * 24 * 60 * 60)
        });
        Doc {
            name: rel.rsplit('/').next().unwrap().to_string(),
            path: PathBuf::from(rel),
            rel:  rel.to_string(),
            root: if rel.starts_with("_specs") { "_specs" } else { "_docs" },
            modified,
        }
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000)
    }

    fn state(docs: Vec<Doc>) -> DocsState {
        DocsState { docs, scanned: true, ..DocsState::default() }
    }

    /// Terms match anywhere in the relative path and in any order, so the user
    /// doesn't have to reproduce a file's exact separators or casing.
    #[test]
    fn query_terms_match_across_separators_and_case() {
        let s = {
            let mut s = state(vec![
                doc("_docs/TELEMETRY_BACKEND.md", Some(1)),
                doc("_docs/NDA.md", Some(1)),
            ]);
            s.query = "backend telemetry".to_string();
            s
        };
        let hits = s.results(now());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "_docs/TELEMETRY_BACKEND.md");
    }

    /// A folder name in the query narrows to that subtree — the reason the
    /// whole relative path is searched rather than just the file name.
    #[test]
    fn query_can_name_a_folder() {
        let mut s = state(vec![
            doc("_specs/api/auth.md", Some(1)),
            doc("_docs/auth.md", Some(1)),
        ]);
        s.query = "specs auth".to_string();
        let hits = s.results(now());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "_specs/api/auth.md");
    }

    #[test]
    fn date_filter_uses_modified_time() {
        let s = {
            let mut s = state(vec![
                doc("_docs/fresh.md", Some(2)),
                doc("_docs/stale.md", Some(200)),
            ]);
            s.date = DateFilter::Week;
            s
        };
        let hits = s.results(now());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rel, "_docs/fresh.md");
    }

    /// A file the filesystem gave no mtime for still lists under "Any date" —
    /// it just can't satisfy a narrower window, since there's nothing to test.
    #[test]
    fn undated_files_survive_any_but_no_narrower_filter() {
        let mut s = state(vec![doc("_docs/undated.md", None)]);
        assert_eq!(s.results(now()).len(), 1);
        s.date = DateFilter::Year;
        assert!(s.results(now()).is_empty());
    }

    /// Clock skew (a file stamped in the future) reads as brand new rather than
    /// as impossibly old, which is what `duration_since`'s error case would
    /// otherwise imply.
    #[test]
    fn future_timestamps_count_as_new() {
        let future = now() + Duration::from_secs(60 * 60);
        assert!(DateFilter::Today.matches(Some(future), now()));
        assert_eq!(age_label(Some(future), now()), "today");
    }

    #[test]
    fn newest_sort_puts_undated_files_last() {
        let mut s = state(vec![
            doc("_docs/old.md", Some(30)),
            doc("_docs/undated.md", None),
            doc("_docs/new.md", Some(1)),
        ]);
        s.sort = SortBy::Newest;
        let rels: Vec<&str> = s.results(now()).iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(rels, ["_docs/new.md", "_docs/old.md", "_docs/undated.md"]);
    }

    #[test]
    fn clear_resets_filters_but_keeps_sort() {
        let mut s = state(vec![doc("_docs/a.md", Some(1))]);
        s.query = "zzz".to_string();
        s.date  = DateFilter::Today;
        s.sort  = SortBy::Newest;
        assert!(s.is_filtered());
        s.clear();
        assert!(!s.is_filtered());
        assert_eq!(s.sort, SortBy::Newest);
        assert_eq!(s.results(now()).len(), 1);
    }

    /// End-to-end over a real directory tree: nested folders are indexed,
    /// hidden entries are skipped, and paths come back project-relative.
    #[test]
    fn rescan_indexes_both_doc_dirs_recursively() {
        let root = std::env::temp_dir().join(format!("frms-docs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("_specs/api")).unwrap();
        std::fs::create_dir_all(root.join("_docs")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("_specs/api/auth.md"), "x").unwrap();
        std::fs::write(root.join("_docs/NDA.md"), "x").unwrap();
        std::fs::write(root.join("_docs/.hidden.md"), "x").unwrap();
        std::fs::write(root.join("src/main.rs"), "x").unwrap();

        let mut s = DocsState::default();
        s.rescan(&root);

        let rels: Vec<&str> = s.docs.iter().map(|d| d.rel.as_str()).collect();
        // _specs before _docs (DOC_DIRS order); the hidden file and everything
        // outside the two doc folders stay out.
        assert_eq!(rels, ["_specs/api/auth.md", "_docs/NDA.md"]);
        assert_eq!(s.roots, ["_specs", "_docs"]);
        assert!(s.scanned && !s.truncated);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project with no `_specs`/`_docs` scans clean rather than erroring, and
    /// reports no roots so the tab can say why the list is empty.
    #[test]
    fn rescan_of_a_project_without_doc_dirs_is_empty() {
        let root = std::env::temp_dir().join(format!("frms-docs-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let mut s = DocsState::default();
        s.rescan(&root);
        assert!(s.scanned);
        assert!(s.docs.is_empty());
        assert!(s.roots.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
