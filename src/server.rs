//! Serves the `lector://` scheme the webview runs on.
//!
//! A document's URL is its absolute path (`lector://localhost/home/me/notes.md`),
//! so relative links and images in the Markdown resolve the way they would on
//! disk. Routes:
//!
//! - `/path/to/doc.md` — the full viewer page with that document
//! - `/path/to/doc.md?content` — the document's first shard, for in-page
//!   navigation; the `X-Shards` header says how many there are
//! - `/path/to/doc.md?shard=N` — shard N; large documents arrive in shards
//!   (see `render::split`) that the page appends as they come
//! - `/path/to/doc.md?mtime` — the file's modification time, polled for live reload
//! - `/_lector/*` — the viewer's own assets, the sidebar tree, and search
//!   (`/_lector/search?q=…&doc=…`, answered with the results as HTML)
//! - any other path — the file's bytes, e.g. images
//!
//! Requests are answered off the UI thread (see `main`), and rendered
//! documents are cached until their file changes. Opening a directory starts
//! a background prefetch of its files, so clicking through them is instant.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use wry::http::{Request, Response, header::CONTENT_TYPE};

use crate::{render, search, tree};

#[cfg(not(windows))]
pub const ORIGIN: &str = "lector://localhost";
#[cfg(windows)]
pub const ORIGIN: &str = "http://lector.localhost";

const SHELL: &str = include_str!("../assets/shell.html");
const STYLE: &str = include_str!("../assets/style.css");
const SCRIPT: &str = include_str!("../assets/app.js");

/// Files larger than this aren't prefetched; they render when opened.
const PREFETCH_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Default)]
pub struct State {
    /// The directory shown in the sidebar, if lector is browsing one.
    root: Mutex<Option<PathBuf>>,
    documents: Mutex<HashMap<PathBuf, Arc<Document>>>,
    /// Raw file contents, for search.
    texts: Mutex<HashMap<PathBuf, Arc<Text>>>,
    /// Bumped whenever the root changes, so a stale prefetch stops early.
    generation: AtomicU64,
}

struct Text {
    modified: SystemTime,
    text: String,
    /// `text` with ASCII lowercased, for case-insensitive search.
    lower: String,
}

/// A file split into shards, each rendered on first use.
struct Document {
    modified: SystemTime,
    sources: Vec<String>,
    shards: Vec<OnceLock<Arc<str>>>,
    /// Set when a newer version of the file replaces this one.
    stale: AtomicBool,
}

impl Document {
    fn len(&self) -> usize {
        self.sources.len()
    }

    /// Shard `i` as HTML, rendering it now unless another thread has (or is).
    fn shard(&self, i: usize) -> Option<Arc<str>> {
        let source = self.sources.get(i)?;
        let html = self.shards[i].get_or_init(|| {
            format!(r#"<div class="shard">{}</div>"#, render::markdown(source)).into()
        });
        Some(html.clone())
    }

    /// Renders every shard after the first, in parallel. The first is left to
    /// whoever asked for the document, so it isn't queued behind the rest.
    fn render_rest(&self) {
        let next = AtomicUsize::new(1);
        let workers = std::thread::available_parallelism()
            .map_or(2, |n| n.get())
            .min(self.len() - 1);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= self.len() || self.stale.load(Ordering::Relaxed) {
                            break;
                        }
                        self.shard(i);
                    }
                });
            }
        });
    }
}

impl State {
    pub fn root(&self) -> Option<PathBuf> {
        self.root.lock().unwrap().clone()
    }

    /// Switches the browsed directory and starts prefetching its files.
    pub fn set_root(self: &Arc<Self>, root: Option<PathBuf>) {
        *self.root.lock().unwrap() = root.clone();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(root) = root {
            let state = self.clone();
            std::thread::spawn(move || state.prefetch(&root, generation));
        }
    }

    fn prefetch(&self, root: &Path, generation: u64) {
        for path in tree::files(&tree::scan(root)) {
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            // Every file's text is loaded for search; only small ones are rendered.
            let _ = self.text(&path);
            let small = std::fs::metadata(&path).is_ok_and(|m| m.len() <= PREFETCH_MAX_BYTES);
            if small && let Ok(doc) = self.document(&path) {
                doc.shard(0);
            }
        }
    }

    /// The file's contents, from the cache unless the file changed since.
    fn text(&self, path: &Path) -> std::io::Result<Arc<Text>> {
        let modified = std::fs::metadata(path)?.modified()?;
        if let Some(text) = self.texts.lock().unwrap().get(path)
            && text.modified == modified
        {
            return Ok(text.clone());
        }
        let text = std::fs::read_to_string(path)?;
        let lower = text.to_ascii_lowercase();
        let text = Arc::new(Text {
            modified,
            text,
            lower,
        });
        self.texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), text.clone());
        Ok(text)
    }

    /// Search results as HTML: matching file names (when browsing a
    /// directory), then text matches grouped by file, the open `doc` first.
    /// In file mode only `doc` is searched.
    fn search(&self, query: &str, doc: Option<&Path>) -> String {
        let query = query.trim();
        if query.is_empty() {
            return String::new();
        }
        let root = self.root();
        let mut files = match &root {
            Some(root) => tree::files(&tree::scan(root)),
            None => doc.map(Path::to_path_buf).into_iter().collect(),
        };
        let label = |path: &Path| match &root {
            Some(root) => path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned(),
            None => path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        };
        let mut out = String::new();

        if root.is_some() {
            let labeled: Vec<_> = files.iter().map(|p| (p.clone(), label(p))).collect();
            let hits = search::names(query, &labeled);
            if !hits.is_empty() {
                out.push_str(r#"<div class="section">Files</div>"#);
            }
            for hit in hits {
                let _ = write!(
                    out,
                    r#"<a class="result" href="{}">{}</a>"#,
                    escape(&url_for(&hit.path)),
                    highlight(&hit.label, &hit.matched),
                );
            }
        }

        if let Some(doc) = doc
            && let Some(i) = files.iter().position(|p| p == doc)
        {
            let current = files.remove(i);
            files.insert(0, current);
        }
        let needle = query.to_ascii_lowercase();
        let mut listed = 0;
        let mut matches = String::new();
        for path in files.iter().filter(|_| needle.len() >= 2) {
            if listed >= search::MAX_HITS {
                break;
            }
            let Ok(text) = self.text(path) else { continue };
            let limit = search::MAX_HITS_PER_FILE.min(search::MAX_HITS - listed);
            let (hits, total) = search::text(&needle, &text.text, &text.lower, limit);
            if total == 0 {
                continue;
            }
            listed += hits.len();
            let _ = write!(
                matches,
                r#"<div class="group"><span>{}</span><span class="count">{total}</span></div>"#,
                escape(&label(path)),
            );
            for hit in hits {
                let _ = write!(
                    matches,
                    r#"<a class="result hit" href="{}" data-find="{}" data-index="{}">{}<mark>{}</mark>{}</a>"#,
                    escape(&url_for(path)),
                    escape(query),
                    hit.occurrence,
                    escape(&hit.before),
                    escape(&hit.matched),
                    escape(&hit.after),
                );
            }
        }
        if !matches.is_empty() {
            out.push_str(r#"<div class="section">In text</div>"#);
            out.push_str(&matches);
        }
        if out.is_empty() {
            out.push_str(r#"<p class="empty">No results</p>"#);
        }
        out
    }

    /// The file as a document, from the cache unless the file changed since.
    /// A new multi-shard document starts rendering its shards in the background.
    fn document(&self, path: &Path) -> std::io::Result<Arc<Document>> {
        let modified = std::fs::metadata(path)?.modified()?;
        if let Some(doc) = self.documents.lock().unwrap().get(path)
            && doc.modified == modified
        {
            return Ok(doc.clone());
        }
        let sources = render::split(&std::fs::read_to_string(path)?);
        let doc = Arc::new(Document {
            modified,
            shards: sources.iter().map(|_| OnceLock::new()).collect(),
            sources,
            stale: AtomicBool::new(false),
        });
        let old = self
            .documents
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), doc.clone());
        if let Some(old) = old {
            old.stale.store(true, Ordering::Relaxed);
        }
        if doc.len() > 1 {
            let doc = doc.clone();
            std::thread::spawn(move || doc.render_rest());
        }
        Ok(doc)
    }
}

pub fn handle(state: &State, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = percent_decode(request.uri().path());
    match path.as_str() {
        "/_lector/style.css" => respond("text/css", STYLE),
        "/_lector/app.js" => respond("text/javascript", SCRIPT),
        "/_lector/tree" => respond("text/html", sidebar(state)),
        "/_lector/search" => {
            let (mut query, mut doc) = (String::new(), None);
            for pair in request.uri().query().unwrap_or_default().split('&') {
                match pair.split_once('=') {
                    Some(("q", value)) => query = percent_decode(value),
                    Some(("doc", value)) if !value.is_empty() => {
                        doc = Some(PathBuf::from(percent_decode(value)))
                    }
                    _ => {}
                }
            }
            respond("text/html", state.search(&query, doc.as_deref()))
        }
        "/" => respond("text/html", shell(state, None)),
        _ => {
            let path = PathBuf::from(path);
            if !tree::is_markdown(&path) {
                return file(&path);
            }
            match request.uri().query() {
                Some("mtime") => respond("text/plain", mtime(&path)),
                Some("content") => {
                    let (html, shards) = first_shard(state, &path);
                    let mut response = respond("text/html", html);
                    response.headers_mut().insert("X-Shards", shards.into());
                    response
                }
                Some(query) if query.starts_with("shard=") => {
                    let index = query["shard=".len()..].parse().ok();
                    let shard = state.document(&path).ok().and_then(|doc| doc.shard(index?));
                    match shard {
                        Some(html) => respond("text/html", html.to_string()),
                        None => not_found(),
                    }
                }
                _ => respond("text/html", shell(state, Some(&path))),
            }
        }
    }
}

/// The URL that shows `path` in the viewer.
pub fn url_for(path: &Path) -> String {
    format!("{ORIGIN}{}", percent_encode(&path.to_string_lossy()))
}

fn shell(state: &State, doc: Option<&Path>) -> String {
    let has_root = state.root().is_some();
    let (title, (content, shards)) = match doc {
        Some(path) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            (format!("{name} — lector"), first_shard(state, path))
        }
        None if has_root => ("lector".to_owned(), (notice("No Markdown files here."), 1)),
        None => (
            "lector".to_owned(),
            (notice("Drop a Markdown file or folder here."), 1),
        ),
    };
    fill(
        SHELL,
        &[
            ("title", &escape(&title)),
            ("class", if has_root { "has-sidebar" } else { "" }),
            ("mtime", &doc.map(mtime).unwrap_or_default()),
            ("shards", &shards.to_string()),
            ("sidebar", &sidebar(state)),
            ("content", &content),
        ],
    )
}

/// The document's first shard, and how many shards it has in total.
fn first_shard(state: &State, path: &Path) -> (String, usize) {
    let doc = state.document(path);
    match doc.as_ref().map(|doc| (doc.shard(0), doc.len())) {
        Ok((Some(html), shards)) => (html.to_string(), shards),
        Ok((None, _)) => (notice("Empty document."), 1),
        Err(e) => (notice(&format!("Cannot open {}: {e}", path.display())), 1),
    }
}

fn notice(message: &str) -> String {
    format!(
        r#"<div class="shard"><p class="notice">{}</p></div>"#,
        escape(message)
    )
}

/// `label` with the characters at `matched` (ascending char indices) in `<mark>`.
fn highlight(label: &str, matched: &[u32]) -> String {
    let mut out = String::new();
    let mut matched = matched.iter().peekable();
    let mut open = false;
    for (i, ch) in label.chars().enumerate() {
        let hit = matched.next_if(|&&m| m as usize == i).is_some();
        if hit != open {
            out.push_str(if hit { "<mark>" } else { "</mark>" });
            open = hit;
        }
        out.push_str(&escape(ch.encode_utf8(&mut [0; 4])));
    }
    if open {
        out.push_str("</mark>");
    }
    out
}

/// The browsed folder's name as a header, then its file tree.
fn sidebar(state: &State) -> String {
    let Some(root) = state.root() else {
        return String::new();
    };
    let full = root.to_string_lossy();
    let name = root
        .file_name()
        .map_or(full.clone(), |n| n.to_string_lossy());
    format!(
        r#"<header title="{}">{}</header>{}"#,
        escape(&full),
        escape(&name),
        tree::to_html(&tree::scan(&root))
    )
}

fn mtime(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}

fn file(path: &Path) -> Response<Cow<'static, [u8]>> {
    match std::fs::read(path) {
        Ok(bytes) => Response::builder()
            .header(CONTENT_TYPE, mime_type(path))
            .body(Cow::Owned(bytes))
            .unwrap(),
        Err(_) => not_found(),
    }
}

fn not_found() -> Response<Cow<'static, [u8]>> {
    Response::builder()
        .status(404)
        .body(Cow::Borrowed(&b""[..]))
        .unwrap()
}

fn respond(mime: &str, body: impl Into<String>) -> Response<Cow<'static, [u8]>> {
    let body: String = body.into();
    Response::builder()
        .header(CONTENT_TYPE, format!("{mime}; charset=utf-8"))
        .header("Cache-Control", "no-store")
        .body(Cow::Owned(body.into_bytes()))
        .unwrap()
}

fn mime_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Replaces each `{{key}}` in `template` in a single pass, so values that
/// happen to contain `{{…}}` are left alone.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").expect("unclosed placeholder in template");
        let key = &after[..end];
        let value = values.iter().find(|(k, _)| *k == key);
        out.push_str(value.expect("unknown placeholder in template").1);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Decodes `%XX` escapes, leaving malformed ones as they are.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_round_trips() {
        for path in ["/home/me/my notes.md", "/tmp/Über (1)#?.md", "/a/b.md"] {
            assert_eq!(percent_decode(&percent_encode(path)), path);
        }
        assert_eq!(percent_encode("/my notes.md"), "/my%20notes.md");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn rerenders_only_when_the_file_changes() {
        let path = std::env::temp_dir().join(format!("lector-cache-{}.md", std::process::id()));
        let state = State::default();
        let file = |text: &str, secs: u64| {
            std::fs::write(&path, text).unwrap();
            let time = UNIX_EPOCH + std::time::Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(time)
                .unwrap();
        };

        let html = || state.document(&path).unwrap().shard(0).unwrap();

        file("# One", 1_000);
        assert!(html().contains("One"));
        // Same modification time: served from the cache, even though the text differs.
        file("# Two", 1_000);
        assert!(html().contains("One"));
        file("# Two", 2_000);
        assert!(html().contains("Two"));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn opening_a_directory_prefetches_its_small_files() {
        let root = std::env::temp_dir().join(format!("lector-prefetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.md"), "# A").unwrap();
        std::fs::write(root.join("sub/b.md"), "# B").unwrap();
        let big = "x".repeat(PREFETCH_MAX_BYTES as usize + 1);
        // Sorts before a.md, so once a.md is cached the big file was already skipped.
        std::fs::write(root.join("0-big.md"), big).unwrap();

        let state = Arc::new(State::default());
        state.set_root(Some(root.clone()));
        let cached = |name: &str| {
            let documents = state.documents.lock().unwrap();
            documents
                .get(&root.join(name))
                .is_some_and(|doc| doc.shards[0].get().is_some())
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !(cached("a.md") && cached("sub/b.md")) {
            assert!(
                std::time::Instant::now() < deadline,
                "prefetch did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!cached("0-big.md"), "files over the limit render on demand");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn large_documents_render_every_shard_in_the_background() {
        let path = std::env::temp_dir().join(format!("lector-shards-{}.md", std::process::id()));
        let filler = "Words to fill out the chapter.\n\n".repeat(300);
        let text: String = (0..100)
            .map(|i| format!("# Chapter {i}\n\n{filler}"))
            .collect();
        std::fs::write(&path, &text).unwrap();

        let state = State::default();
        let doc = state.document(&path).unwrap();
        assert!(doc.len() > 5, "{} shards", doc.len());
        let html: String = (0..doc.len())
            .map(|i| doc.shard(i).unwrap().to_string())
            .collect();
        assert_eq!(html.matches("<h1").count(), 100);
        assert!(doc.shard(doc.len()).is_none());
        assert!(doc.shards.iter().all(|s| s.get().is_some()));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn searches_names_and_text_with_the_open_document_first() {
        let root = std::env::temp_dir().join(format!("lector-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/configuration.md"),
            "Set the Sidebar width.\n",
        )
        .unwrap();
        std::fs::write(root.join("notes.md"), "The sidebar <b>folds</b>.\n").unwrap();

        let state = Arc::new(State::default());
        state.set_root(Some(root.clone()));
        let html = state.search("sidebar", Some(&root.join("notes.md")));
        assert!(html.contains("In text"), "{html}");
        let notes = html.find(">notes.md<").expect("notes.md group");
        let config = html
            .find(">docs/configuration.md<")
            .expect("configuration.md group");
        assert!(notes < config, "the open document comes first: {html}");
        assert!(
            html.contains("&lt;b&gt;folds"),
            "snippets are escaped: {html}"
        );
        assert!(
            html.contains(r#"data-find="sidebar" data-index="0""#),
            "{html}"
        );

        let html = state.search("cfg", None);
        assert!(html.contains("Files") && html.contains("<mark>"), "{html}");
        assert_eq!(
            state.search("zzz-nothing", None),
            r#"<p class="empty">No results</p>"#
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn highlights_matched_characters() {
        assert_eq!(highlight("a&bc", &[1, 2]), "a<mark>&amp;b</mark>c");
        assert_eq!(highlight("abc", &[2]), "ab<mark>c</mark>");
    }

    #[test]
    fn fill_does_not_expand_placeholders_inside_values() {
        let out = fill("<{{a}}|{{b}}>", &[("a", "{{b}}"), ("b", "B")]);
        assert_eq!(out, "<{{b}}|B>");
    }
}
