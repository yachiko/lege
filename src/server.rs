//! Serves the `lector://` scheme the webview runs on.
//!
//! A document's URL is its absolute path (`lector://localhost/home/me/notes.md`),
//! so relative links and images in the Markdown resolve the way they would on
//! disk. Routes:
//!
//! - `/path/to/doc.md` — the full viewer page with that document
//! - `/path/to/doc.md?content` — just the rendered document, for in-page navigation
//! - `/path/to/doc.md?mtime` — the file's modification time, polled for live reload
//! - `/_lector/*` — the viewer's own assets, and the sidebar tree
//! - any other path — the file's bytes, e.g. images
//!
//! Requests are answered off the UI thread (see `main`), and rendered
//! documents are cached until their file changes. Opening a directory starts
//! a background prefetch of its files, so clicking through them is instant.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use wry::http::{Request, Response, header::CONTENT_TYPE};

use crate::{render, tree};

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
    rendered: Mutex<HashMap<PathBuf, Rendered>>,
    /// Bumped whenever the root changes, so a stale prefetch stops early.
    generation: AtomicU64,
}

struct Rendered {
    modified: SystemTime,
    html: Arc<str>,
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
            let small = std::fs::metadata(&path).is_ok_and(|m| m.len() <= PREFETCH_MAX_BYTES);
            if small {
                let _ = self.render(&path);
            }
        }
    }

    /// The document's HTML, from the cache unless the file changed since.
    fn render(&self, path: &Path) -> std::io::Result<Arc<str>> {
        let modified = std::fs::metadata(path)?.modified()?;
        if let Some(cached) = self.rendered.lock().unwrap().get(path)
            && cached.modified == modified
        {
            return Ok(cached.html.clone());
        }
        // Rendering happens outside the lock, so other requests aren't held up.
        let html: Arc<str> = render::markdown(&std::fs::read_to_string(path)?).into();
        let entry = Rendered {
            modified,
            html: html.clone(),
        };
        self.rendered
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), entry);
        Ok(html)
    }
}

pub fn handle(state: &State, request: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = percent_decode(request.uri().path());
    match path.as_str() {
        "/_lector/style.css" => respond("text/css", STYLE),
        "/_lector/app.js" => respond("text/javascript", SCRIPT),
        "/_lector/tree" => respond("text/html", sidebar(state)),
        "/" => respond("text/html", shell(state, None)),
        _ => {
            let path = PathBuf::from(path);
            if !tree::is_markdown(&path) {
                return file(&path);
            }
            match request.uri().query() {
                Some("mtime") => respond("text/plain", mtime(&path)),
                Some("content") => respond("text/html", content(state, &path)),
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
    let (title, content) = match doc {
        Some(path) => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            (format!("{name} — lector"), self::content(state, path))
        }
        None if has_root => ("lector".to_owned(), notice("No Markdown files here.")),
        None => (
            "lector".to_owned(),
            notice("Drop a Markdown file or folder here."),
        ),
    };
    fill(
        SHELL,
        &[
            ("title", &escape(&title)),
            ("class", if has_root { "has-sidebar" } else { "" }),
            ("mtime", &doc.map(mtime).unwrap_or_default()),
            ("sidebar", &sidebar(state)),
            ("content", &content),
        ],
    )
}

fn content(state: &State, path: &Path) -> String {
    match state.render(path) {
        Ok(html) => html.to_string(),
        Err(e) => notice(&format!("Cannot open {}: {e}", path.display())),
    }
}

fn notice(message: &str) -> String {
    format!(r#"<p class="notice">{}</p>"#, escape(message))
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
        Err(_) => Response::builder()
            .status(404)
            .body(Cow::Borrowed(&b""[..]))
            .unwrap(),
    }
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

        file("# One", 1_000);
        assert!(state.render(&path).unwrap().contains("One"));
        // Same modification time: served from the cache, even though the text differs.
        file("# Two", 1_000);
        assert!(state.render(&path).unwrap().contains("One"));
        file("# Two", 2_000);
        assert!(state.render(&path).unwrap().contains("Two"));

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
            state
                .rendered
                .lock()
                .unwrap()
                .contains_key(&root.join(name))
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
    fn fill_does_not_expand_placeholders_inside_values() {
        let out = fill("<{{a}}|{{b}}>", &[("a", "{{b}}"), ("b", "B")]);
        assert_eq!(out, "<{{b}}|B>");
    }
}
