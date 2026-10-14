//! Two-way sync between frms notes and a [BookStack](https://www.bookstackapp.com/)
//! instance over its REST API.
//!
//! No TLS crate — every request shells out to `curl`, the same convention
//! `telemetry`, `stats`, and `chat_api` follow (the project builds C-free, so
//! there is no rustls/openssl). The API token rides in a request header the way
//! `stats` passes its OAuth bearer; request bodies go in on stdin so a note's
//! text never lands in the process list.
//!
//! ## The mapping
//! ```text
//! Shelf "frms"
//! └── Book  = project (basename; "Global" for project-less notes)
//!     └── Chapter = category ("General" when uncategorized)
//!         └── Page = note
//! ```
//!
//! ## How the sync reconciles
//! [`reconcile`] runs both directions in one pass. Each note is tied to a
//! BookStack page by its stored page id ([`crate::notes::Note::bookstack_page_id`]);
//! a note also remembers the page's `updated_at` and a content hash from the last
//! successful sync ([`NoteForSync::synced_at`] / [`NoteForSync::synced_hash`]), so
//! the sync can tell which side changed:
//!
//! - **local changed only** → push the note up (create or update the page).
//! - **remote changed only** → pull the page down into the note.
//! - **both changed** → a [`Conflict`]: nothing is overwritten; the caller shows
//!   the user a diff and lets them pick a side.
//! - **neither changed** → nothing to do.
//!
//! Pages under the shelf that no note points at were authored in BookStack while
//! away from frms; they come back as [`PulledNote`]s the caller turns into new
//! notes. Books and chapters are resolved by name and created on demand, so the
//! layer is self-healing and safe to re-run.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

/// BookStack caps names (shelf/book/chapter/page) at 255 characters.
const NAME_MAX: usize = 255;

/// Chapter name used for notes with no category, and the category a page filed
/// directly under a book (or in a "General" chapter) pulls back as. Must match
/// [`crate::notes::UNCATEGORIZED_LABEL`] so a round-trip is stable.
const GENERAL_CHAPTER: &str = "General";

/// Book name used for project-less (global) notes, and the marker a page under
/// that book pulls back as so the caller knows to make the note global.
pub const GLOBAL_BOOK: &str = "Global";

/// Connection details, drawn from [`crate::prefs::Prefs`].
#[derive(Debug, Clone)]
pub struct Config {
    /// Instance root, e.g. `https://wiki.example.com` (no trailing `/api`).
    pub base_url:     String,
    /// The API token id half of the `Token <id>:<secret>` credential.
    pub token_id:     String,
    /// The API token secret half.
    pub token_secret: String,
    /// Name of the shelf every frms book is filed under. Defaults to `frms`.
    pub shelf_name:   String,
}

/// One note handed to [`reconcile`], with its project/category already resolved
/// to the book/chapter names the sync should file it under, plus the baseline
/// from its last successful sync.
#[derive(Debug, Clone)]
pub struct NoteForSync {
    /// The frms-side note id, echoed back in the [`Outcome`].
    pub id:           u64,
    pub title:        String,
    /// Note body, as markdown.
    pub body:         String,
    /// Book name — the project basename, or [`GLOBAL_BOOK`] for a global note.
    pub project_name: String,
    /// Chapter name — the category, or [`GENERAL_CHAPTER`] when uncategorized.
    pub category:     String,
    /// The BookStack page id from a previous sync, if any. `Some` → this note is
    /// tied to that page; `None` → it has never been pushed.
    pub page_id:      Option<u64>,
    /// The page's `updated_at` recorded at the last successful sync. `None` for a
    /// note that predates two-way sync (or has never synced). Differs from the
    /// page's current `updated_at` exactly when the page changed remotely.
    pub synced_at:    Option<String>,
    /// [`content_hash`] of the note's title+body at the last successful sync.
    /// Differs from the current hash exactly when the note changed locally.
    pub synced_hash:  Option<u64>,
}

/// The post-sync baseline to record back on a note whose page was created or
/// updated (pushed), so the next sync can tell it is in sync.
#[derive(Debug, Clone)]
pub struct SyncedRef {
    pub note_id:     u64,
    pub page_id:     u64,
    pub synced_at:   String,
    pub synced_hash: u64,
}

/// A remote page whose content should overwrite the matching local note (the
/// note was unchanged locally but its page changed in BookStack).
#[derive(Debug, Clone)]
pub struct RemoteUpdate {
    pub note_id:     u64,
    pub title:       String,
    pub body:        String,
    /// Chapter name; [`GENERAL_CHAPTER`] means uncategorized.
    pub category:    String,
    pub synced_at:   String,
    pub synced_hash: u64,
}

/// A page under the shelf that no local note points at — authored in BookStack —
/// which the caller turns into a new local note.
#[derive(Debug, Clone)]
pub struct PulledNote {
    pub page_id:      u64,
    pub title:        String,
    pub body:         String,
    /// Book name; [`GLOBAL_BOOK`] means a global note.
    pub project_name: String,
    /// Chapter name; [`GENERAL_CHAPTER`] means uncategorized.
    pub category:     String,
    pub synced_at:    String,
    pub synced_hash:  u64,
}

/// A note that changed on both sides since the last sync. Nothing is overwritten;
/// this carries both versions so the caller can show a diff and let the user
/// pick a side (see [`crate::notes::NotesState::resolve_conflict_use_remote`] /
/// [`crate::notes::NotesState::resolve_conflict_keep_local`]).
#[derive(Debug, Clone)]
pub struct Conflict {
    pub note_id:         u64,
    pub local_title:     String,
    pub local_body:      String,
    pub remote_title:    String,
    pub remote_body:     String,
    /// The chapter the remote page sits in ([`GENERAL_CHAPTER`] = uncategorized).
    pub remote_category: String,
    /// The remote page's current `updated_at`, recorded when the user resolves so
    /// the conflict doesn't immediately re-fire.
    pub remote_synced_at: String,
}

/// Summary of a completed [`reconcile`] run.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Pages created in BookStack (notes pushed up for the first time).
    pub created: usize,
    /// Pages updated in BookStack (local edits pushed up).
    pub updated: usize,
    /// Local notes created from BookStack pages (authored while away).
    pub pulled_new: usize,
    /// Local notes overwritten with newer BookStack content.
    pub pulled_updated: usize,
    /// Baselines to record on notes whose pages were created/updated.
    pub synced: Vec<SyncedRef>,
    /// Remote content to write into existing notes.
    pub remote_updates: Vec<RemoteUpdate>,
    /// New notes to create locally from remote pages.
    pub new_notes: Vec<PulledNote>,
    /// Notes that changed on both sides — surfaced for the user to resolve.
    pub conflicts: Vec<Conflict>,
    /// Per-note failures `(note id, message)`. A sync pushes/pulls everything it
    /// can and collects the rest here rather than aborting on the first error.
    pub failed:  Vec<(u64, String)>,
}

impl Outcome {
    /// A one-line summary for the Notes-tab status line.
    pub fn summary(&self) -> String {
        let pushed = self.created + self.updated;
        let pulled = self.pulled_new + self.pulled_updated;
        let mut s = format!("Synced: {pushed} pushed, {pulled} pulled");
        if !self.conflicts.is_empty() {
            s.push_str(&format!(", {} conflict(s)", self.conflicts.len()));
        }
        if !self.failed.is_empty() {
            s.push_str(&format!(", {} failed", self.failed.len()));
        }
        s
    }
}

/// Hash a note's content (title + body) into the value stored as
/// [`NoteForSync::synced_hash`]. Kept here so the reconcile and the caller agree
/// on exactly what "the note changed" means.
pub fn content_hash(title: &str, body: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut h);
    "\n".hash(&mut h);
    body.hash(&mut h);
    h.finish()
}

/// Async wrapper: run [`reconcile`] on a blocking thread so the curl round-trips
/// never block the UI. Driven from the app with `Task::perform`.
pub async fn reconcile_async(
    cfg: Config,
    notes: Vec<NoteForSync>,
    declined: HashSet<u64>,
) -> Result<Outcome, String> {
    tokio::task::spawn_blocking(move || reconcile(cfg, notes, declined))
        .await
        .unwrap_or_else(|e| Err(format!("sync task failed: {e}")))
}

/// Reconcile `notes` against the BookStack instance described by `cfg`, in both
/// directions.
///
/// Fails outright only when the config is incomplete or the shelf can't be
/// resolved/read — without a shelf there is nowhere to file anything and nothing
/// to pull. Once the shelf is in hand, every note is reconciled independently and
/// per-note errors are collected in [`Outcome::failed`].
///
/// `declined` holds page ids the user has dismissed from the import list; pages
/// under the shelf whose id is in that set are not offered again.
pub fn reconcile(
    cfg: Config,
    notes: Vec<NoteForSync>,
    declined: HashSet<u64>,
) -> Result<Outcome, String> {
    if cfg.base_url.trim().is_empty()
        || cfg.token_id.trim().is_empty()
        || cfg.token_secret.trim().is_empty()
    {
        return Err("BookStack URL and API token are required.".into());
    }

    let shelf_id = ensure_shelf(&cfg)?;
    // Everything currently under the shelf, keyed by page id, so each note can be
    // matched to its page and leftover pages can be pulled down as new notes.
    let remote = fetch_shelf_pages(&cfg, shelf_id)?;

    // Resolve each project→book and (book, category)→chapter once per run.
    let mut books:    HashMap<String, u64>        = HashMap::new();
    let mut chapters: HashMap<(u64, String), u64> = HashMap::new();
    let mut out = Outcome::default();
    let mut matched: HashSet<u64> = HashSet::new();

    for note in &notes {
        let cur_hash = content_hash(&note.title, &note.body);
        let result = (|| -> Result<(), String> {
            match note.page_id.and_then(|pid| remote.get(&pid).map(|r| (pid, r))) {
                // The note's page still exists remotely — decide the direction.
                Some((pid, rp)) => {
                    matched.insert(pid);

                    // Identical content → nothing to move; just (re)record the
                    // baseline. Also covers a coincidental same-edit on both sides.
                    if note.title == rp.title && note.body == rp.markdown {
                        out.synced.push(SyncedRef {
                            note_id: note.id, page_id: pid,
                            synced_at: rp.updated_at.clone(), synced_hash: cur_hash,
                        });
                        return Ok(());
                    }

                    // No baseline yet — a note from the push-only era, or the very
                    // first two-way run. Trust the local note as the source of
                    // truth and simply adopt a baseline: never a conflict, never an
                    // overwrite. Any text difference here is BookStack's markdown
                    // normalisation, not a real edit; a later genuine local edit
                    // pushes up normally once a baseline exists.
                    let has_baseline = note.synced_at.is_some() || note.synced_hash.is_some();
                    if !has_baseline {
                        out.synced.push(SyncedRef {
                            note_id: note.id, page_id: pid,
                            synced_at: rp.updated_at.clone(), synced_hash: cur_hash,
                        });
                        return Ok(());
                    }

                    // With a baseline, "changed" is measured against it. A side
                    // whose marker is missing is treated as unchanged (the other
                    // marker still discriminates), so a cosmetic normalisation diff
                    // never fabricates a conflict.
                    let local_changed  = note.synced_hash.map(|h| h != cur_hash).unwrap_or(false);
                    let remote_changed = note.synced_at.as_deref()
                        .map(|s| s != rp.updated_at).unwrap_or(false);

                    match (local_changed, remote_changed) {
                        // In sync per the baseline — ignore any leftover
                        // normalisation difference rather than re-pushing forever.
                        (false, false) => {}
                        (true, false) => {
                            let (_b, chapter_id) = resolve_book_chapter(
                                &cfg, shelf_id, &note.project_name, &note.category,
                                &mut books, &mut chapters,
                            )?;
                            let updated_at = update_page(&cfg, pid, chapter_id, &note.title, &note.body)?;
                            out.updated += 1;
                            out.synced.push(SyncedRef {
                                note_id: note.id, page_id: pid,
                                synced_at: updated_at, synced_hash: cur_hash,
                            });
                        }
                        (false, true) => {
                            out.pulled_updated += 1;
                            out.remote_updates.push(RemoteUpdate {
                                note_id: note.id,
                                title:   rp.title.clone(),
                                body:    rp.markdown.clone(),
                                category: rp.chapter_name.clone(),
                                synced_at: rp.updated_at.clone(),
                                synced_hash: content_hash(&rp.title, &rp.markdown),
                            });
                        }
                        // Both sides changed since the baseline — let the user decide.
                        (true, true) => out.conflicts.push(Conflict {
                            note_id: note.id,
                            local_title:  note.title.clone(),
                            local_body:   note.body.clone(),
                            remote_title: rp.title.clone(),
                            remote_body:  rp.markdown.clone(),
                            remote_category:  rp.chapter_name.clone(),
                            remote_synced_at: rp.updated_at.clone(),
                        }),
                    }
                }
                // Never pushed, or the page was deleted in BookStack — (re)create
                // it rather than lose the note.
                None => {
                    let (_b, chapter_id) = resolve_book_chapter(
                        &cfg, shelf_id, &note.project_name, &note.category,
                        &mut books, &mut chapters,
                    )?;
                    let (new_id, updated_at) =
                        create_page(&cfg, chapter_id, &note.title, &note.body)?;
                    out.created += 1;
                    out.synced.push(SyncedRef {
                        note_id: note.id, page_id: new_id,
                        synced_at: updated_at, synced_hash: cur_hash,
                    });
                }
            }
            Ok(())
        })();

        if let Err(e) = result {
            out.failed.push((note.id, e));
        }
    }

    // Any page under the shelf that no note claimed was authored in BookStack —
    // offer it as a new note (the caller confirms the import). Pages the user has
    // dismissed stay dismissed. Sorted by id for a stable order.
    let mut leftover: Vec<(&u64, &RemotePage)> = remote.iter()
        .filter(|(pid, _)| !matched.contains(pid) && !declined.contains(*pid))
        .collect();
    leftover.sort_by_key(|(pid, _)| **pid);
    for (pid, rp) in leftover {
        out.pulled_new += 1;
        out.new_notes.push(PulledNote {
            page_id: *pid,
            title:   rp.title.clone(),
            body:    rp.markdown.clone(),
            project_name: rp.book_name.clone(),
            category:     rp.chapter_name.clone(),
            synced_at:    rp.updated_at.clone(),
            synced_hash:  content_hash(&rp.title, &rp.markdown),
        });
    }

    Ok(out)
}

/// A page under the frms shelf, as [`fetch_shelf_pages`] returns it.
#[derive(Debug, Clone)]
struct RemotePage {
    title:        String,
    markdown:     String,
    book_name:    String,
    chapter_name: String,
    updated_at:   String,
}

/// Read every page under the shelf, keyed by page id, with the book and chapter
/// names resolved and each page's markdown fetched.
fn fetch_shelf_pages(cfg: &Config, shelf_id: u64) -> Result<HashMap<u64, RemotePage>, String> {
    let shelf = request(cfg, "GET", &format!("shelves/{shelf_id}"), None)?;
    let books: Vec<(u64, String)> = shelf["books"].as_array()
        .map(|a| a.iter()
            .filter_map(|b| Some((b["id"].as_u64()?, b["name"].as_str()?.to_string())))
            .collect())
        .unwrap_or_default();

    let mut out = HashMap::new();
    for (book_id, book_name) in books {
        // chapter id → name, so each page's chapter_id can be labelled.
        let chapter_rows = list_all(cfg, "chapters", &[("filter[book_id]", &book_id.to_string())])?;
        let chap_names: HashMap<u64, String> = chapter_rows.iter()
            .filter_map(|c| Some((c["id"].as_u64()?, c["name"].as_str()?.to_string())))
            .collect();

        let page_rows = list_all(cfg, "pages", &[("filter[book_id]", &book_id.to_string())])?;
        for p in page_rows {
            let Some(pid) = p["id"].as_u64() else { continue; };
            let chapter_name = p["chapter_id"].as_u64()
                .and_then(|cid| chap_names.get(&cid).cloned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| GENERAL_CHAPTER.to_string());

            // The list endpoint omits the body, so fetch the page for its
            // markdown and authoritative updated_at.
            let detail = request(cfg, "GET", &format!("pages/{pid}"), None)?;
            let title = detail["name"].as_str().unwrap_or_default().to_string();
            out.insert(pid, RemotePage {
                title,
                markdown:     page_markdown(&detail),
                book_name:    book_name.clone(),
                chapter_name,
                updated_at:   detail["updated_at"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    Ok(out)
}

/// A page's body as markdown: the `markdown` source field when present (pages
/// frms pushed, or ones written in BookStack's markdown editor), otherwise the
/// rendered `html` as a fallback (a page authored in the WYSIWYG editor has no
/// markdown source — better to pull the HTML than an empty note).
fn page_markdown(detail: &Value) -> String {
    let md = detail["markdown"].as_str().unwrap_or_default();
    if !md.trim().is_empty() {
        md.to_string()
    } else {
        detail["html"].as_str().unwrap_or_default().to_string()
    }
}

// ── entity resolution ───────────────────────────────────────────────────────

fn ensure_shelf(cfg: &Config) -> Result<u64, String> {
    if let Some(id) = find_by_name(cfg, "shelves", &cfg.shelf_name)? {
        return Ok(id);
    }
    let body = json!({ "name": clamp(&cfg.shelf_name) });
    let v = request(cfg, "POST", "shelves", Some(&body))?;
    id_of(&v, "shelf")
}

/// Resolve (and cache) the book for `project_name` and the chapter for
/// `category` within it, creating either on demand.
fn resolve_book_chapter(
    cfg: &Config,
    shelf_id: u64,
    project_name: &str,
    category: &str,
    books:    &mut HashMap<String, u64>,
    chapters: &mut HashMap<(u64, String), u64>,
) -> Result<(u64, u64), String> {
    let book_id = match books.get(project_name) {
        Some(id) => *id,
        None => {
            let id = ensure_book(cfg, shelf_id, project_name)?;
            books.insert(project_name.to_string(), id);
            id
        }
    };
    let key = (book_id, category.to_string());
    let chapter_id = match chapters.get(&key) {
        Some(id) => *id,
        None => {
            let id = ensure_chapter(cfg, book_id, category)?;
            chapters.insert(key, id);
            id
        }
    };
    Ok((book_id, chapter_id))
}

fn ensure_book(cfg: &Config, shelf_id: u64, name: &str) -> Result<u64, String> {
    if let Some(id) = find_by_name(cfg, "books", name)? {
        // Already exists — make sure it is on our shelf (a book created in an
        // earlier run, or the shelf edited by hand, may have it detached).
        attach_book_to_shelf(cfg, shelf_id, id)?;
        return Ok(id);
    }
    let body = json!({ "name": clamp(name) });
    let v = request(cfg, "POST", "books", Some(&body))?;
    let id = id_of(&v, "book")?;
    attach_book_to_shelf(cfg, shelf_id, id)?;
    Ok(id)
}

fn ensure_chapter(cfg: &Config, book_id: u64, name: &str) -> Result<u64, String> {
    let v = list(cfg, "chapters", &[
        ("filter[book_id]", &book_id.to_string()),
        ("filter[name]", name),
        ("count", "1"),
    ])?;
    if let Some(id) = first_id(&v) {
        return Ok(id);
    }
    let body = json!({ "book_id": book_id, "name": clamp(name) });
    let v = request(cfg, "POST", "chapters", Some(&body))?;
    id_of(&v, "chapter")
}

/// Add `book_id` to the shelf's book list without dropping the books already on
/// it — a shelf PUT replaces the whole list, so we read, union, and write back.
fn attach_book_to_shelf(cfg: &Config, shelf_id: u64, book_id: u64) -> Result<(), String> {
    let shelf = request(cfg, "GET", &format!("shelves/{shelf_id}"), None)?;
    let mut ids: Vec<u64> = shelf["books"]
        .as_array()
        .map(|a| a.iter().filter_map(|b| b["id"].as_u64()).collect())
        .unwrap_or_default();
    if ids.contains(&book_id) {
        return Ok(());
    }
    ids.push(book_id);
    request(cfg, "PUT", &format!("shelves/{shelf_id}"), Some(&json!({ "books": ids })))?;
    Ok(())
}

/// Create a page and return its `(id, updated_at)` so the caller can baseline it.
fn create_page(cfg: &Config, chapter_id: u64, title: &str, body: &str)
    -> Result<(u64, String), String>
{
    let v = request(cfg, "POST", "pages", Some(&json!({
        "chapter_id": chapter_id,
        "name":       clamp(title),
        "markdown":   markdown(body),
    })))?;
    Ok((id_of(&v, "page")?, updated_at_of(&v)))
}

/// Update a page in place and return its new `updated_at`.
fn update_page(cfg: &Config, page_id: u64, chapter_id: u64, title: &str, body: &str)
    -> Result<String, String>
{
    // Re-send chapter_id too, so a note that changed category is moved.
    let v = request(cfg, "PUT", &format!("pages/{page_id}"), Some(&json!({
        "chapter_id": chapter_id,
        "name":       clamp(title),
        "markdown":   markdown(body),
    })))?;
    Ok(updated_at_of(&v))
}

/// The id of the first entry in a paginated list response, if any.
fn find_by_name(cfg: &Config, endpoint: &str, name: &str) -> Result<Option<u64>, String> {
    let v = list(cfg, endpoint, &[("filter[name]", name), ("count", "1")])?;
    Ok(first_id(&v))
}

fn first_id(list: &Value) -> Option<u64> {
    list["data"].as_array().and_then(|a| a.first()).and_then(|x| x["id"].as_u64())
}

fn id_of(v: &Value, what: &str) -> Result<u64, String> {
    v["id"].as_u64().ok_or_else(|| format!("{what} create: response had no id"))
}

fn updated_at_of(v: &Value) -> String {
    v["updated_at"].as_str().unwrap_or_default().to_string()
}

// ── content helpers ─────────────────────────────────────────────────────────

/// Truncate a name to BookStack's 255-char limit (by chars, not bytes, so a
/// multi-byte character is never split).
fn clamp(s: &str) -> String {
    if s.chars().count() <= NAME_MAX {
        s.to_string()
    } else {
        s.chars().take(NAME_MAX).collect()
    }
}

/// A page needs some content — BookStack rejects an empty body — so an empty
/// note becomes a small placeholder rather than a failed sync.
fn markdown(body: &str) -> String {
    if body.trim().is_empty() {
        "*(empty note)*".to_string()
    } else {
        body.to_string()
    }
}

// ── curl transport ──────────────────────────────────────────────────────────

/// A `GET` list request with url-encoded query parameters (`-G
/// --data-urlencode` lets curl encode names and filter values safely).
fn list(cfg: &Config, endpoint: &str, params: &[(&str, &str)]) -> Result<Value, String> {
    let mut cmd = base_curl(cfg, "GET", endpoint);
    cmd.arg("-G");
    for (k, v) in params {
        cmd.args(["--data-urlencode", &format!("{k}={v}")]);
    }
    run(cmd, None)
}

/// List an endpoint across all its pages, returning every `data` row. BookStack
/// paginates with `count`/`offset` and reports `total`, so we walk until we've
/// seen them all (guarding against a `total` that never catches up).
fn list_all(cfg: &Config, endpoint: &str, base_params: &[(&str, &str)]) -> Result<Vec<Value>, String> {
    const PAGE: usize = 200;
    let mut all: Vec<Value> = Vec::new();
    let mut offset = 0usize;
    loop {
        let offset_s = offset.to_string();
        let mut params: Vec<(&str, &str)> = base_params.to_vec();
        params.push(("count", "200"));
        params.push(("offset", &offset_s));
        let v = list(cfg, endpoint, &params)?;
        let rows = v["data"].as_array().cloned().unwrap_or_default();
        let got = rows.len();
        all.extend(rows);
        let total = v["total"].as_u64().unwrap_or(all.len() as u64) as usize;
        offset += got;
        if got == 0 || got < PAGE || all.len() >= total {
            break;
        }
    }
    Ok(all)
}

/// A `GET`/`POST`/`PUT` request with an optional JSON body (sent on stdin).
fn request(cfg: &Config, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let mut cmd = base_curl(cfg, method, path);
    let bytes = match body {
        Some(v) => {
            cmd.args(["--data-binary", "@-"]);
            Some(serde_json::to_vec(v).map_err(|e| format!("encode body: {e}"))?)
        }
        None => None,
    };
    run(cmd, bytes)
}

/// The shared curl invocation: auth + JSON headers, a bounded timeout, and a
/// trailing `%{http_code}` on its own line so [`run`] can read the status even
/// when BookStack returns an error body.
fn base_curl(cfg: &Config, method: &str, path: &str) -> Command {
    let url  = format!("{}/api/{}", cfg.base_url.trim_end_matches('/'), path);
    let auth = format!("Authorization: Token {}:{}", cfg.token_id, cfg.token_secret);
    let mut cmd = Command::new("curl");
    cmd.args([
        "-sS",                    // quiet, but still print transport errors
        "-w", "\n%{http_code}",   // status code on a final line
        "-X", method,
        "-H", &auth,
        "-H", "Content-Type: application/json",
        "-H", "Accept: application/json",
        "--max-time", "30",
    ]);
    cmd.arg(&url);
    cmd
}

/// Spawn `cmd`, feed `body` in on stdin if given, and parse the response.
fn run(mut cmd: Command, body: Option<Vec<u8>>) -> Result<Value, String> {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("could not run curl: {e}"))?;

    if let Some(bytes) = body {
        // Take the handle so it drops (closing stdin) before we wait.
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&bytes).map_err(|e| format!("write request body: {e}"))?;
        }
    }

    let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);

    // No output at all → a transport failure (offline, bad host, TLS): surface
    // curl's own message from stderr.
    if stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() { "no response from BookStack".into() } else { err.to_string() });
    }

    // Split the trailing "\n<code>" the -w format appended.
    let (body_str, code_str) = stdout.rsplit_once('\n').unwrap_or(("", stdout.as_ref()));
    let code: u16 = code_str.trim().parse().unwrap_or(0);

    if !(200..300).contains(&code) {
        // BookStack error bodies look like {"error":{"message":"…"}}.
        let msg = serde_json::from_str::<Value>(body_str)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(String::from))
            .unwrap_or_else(|| format!("HTTP {code}"));
        return Err(msg);
    }

    if body_str.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(body_str).map_err(|e| format!("unexpected response: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_limits_by_char_count() {
        assert_eq!(clamp("short"), "short");
        let long: String = "x".repeat(300);
        assert_eq!(clamp(&long).chars().count(), NAME_MAX);
    }

    #[test]
    fn empty_body_becomes_a_placeholder() {
        assert_eq!(markdown("   \n  "), "*(empty note)*");
        assert_eq!(markdown("# real"), "# real");
    }

    #[test]
    fn incomplete_config_is_rejected_before_any_request() {
        let cfg = Config {
            base_url: String::new(),
            token_id: "id".into(),
            token_secret: "secret".into(),
            shelf_name: "frms".into(),
        };
        assert!(reconcile(cfg, vec![], HashSet::new()).is_err());
    }

    #[test]
    fn outcome_summary_reports_both_directions_and_conflicts() {
        let clean = Outcome { created: 2, updated: 1, pulled_new: 3, ..Default::default() };
        assert_eq!(clean.summary(), "Synced: 3 pushed, 3 pulled");
        let messy = Outcome {
            created: 1,
            conflicts: vec![Conflict {
                note_id: 9,
                local_title: String::new(), local_body: String::new(),
                remote_title: String::new(), remote_body: String::new(),
                remote_category: String::new(), remote_synced_at: String::new(),
            }],
            failed: vec![(9, "nope".into())],
            ..Default::default()
        };
        assert_eq!(messy.summary(), "Synced: 1 pushed, 0 pulled, 1 conflict(s), 1 failed");
    }

    #[test]
    fn content_hash_tracks_title_and_body() {
        let base = content_hash("t", "b");
        assert_eq!(base, content_hash("t", "b"));
        assert_ne!(base, content_hash("t", "b2"));
        assert_ne!(base, content_hash("t2", "b"));
        // A title/body split can't be spoofed by shifting the boundary.
        assert_ne!(content_hash("ab", "c"), content_hash("a", "bc"));
    }

    #[test]
    fn first_id_reads_the_leading_list_entry() {
        let v = json!({ "data": [ { "id": 7, "name": "a" }, { "id": 8 } ] });
        assert_eq!(first_id(&v), Some(7));
        assert_eq!(first_id(&json!({ "data": [] })), None);
    }

    #[test]
    fn page_markdown_prefers_source_falls_back_to_html() {
        assert_eq!(page_markdown(&json!({ "markdown": "# hi", "html": "<h1>hi</h1>" })), "# hi");
        assert_eq!(page_markdown(&json!({ "markdown": "  ", "html": "<p>x</p>" })), "<p>x</p>");
        assert_eq!(page_markdown(&json!({})), "");
    }
}
