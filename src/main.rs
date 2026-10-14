//! lector — a minimal Markdown viewer.
//!
//! Usage: `lector [FILE.md | DIR]`, or drop a file or folder onto the window.
//! A directory opens with a sidebar listing its Markdown files. The open file
//! reloads automatically when it changes on disk.

mod tree;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use tree::{Kind, Node};

/// Widest the text column gets, so long lines stay readable on big windows.
const READING_WIDTH: f32 = 760.0;
/// Minimum space on each side of the text column.
const GUTTER: f32 = 16.0;
/// How often the open file is checked for changes.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

fn main() -> eframe::Result {
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("lector")
            .with_inner_size([860.0, 1000.0])
            .with_min_inner_size([320.0, 240.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };

    eframe::run_native(
        "lector",
        options,
        Box::new(|cc| {
            // egui's defaults are sized for tool UIs; bump them for reading.
            cc.egui_ctx.all_styles_mut(|style| {
                for font in style.text_styles.values_mut() {
                    font.size *= 1.2;
                }
            });
            Ok(Box::new(Lector::new(path)))
        }),
    )
}

struct Document {
    path: PathBuf,
    text: String,
    modified: Option<SystemTime>,
}

impl Document {
    fn load(path: &Path) -> Result<Self, String> {
        let error = |e: std::io::Error| format!("{}: {e}", path.display());
        let path = path.canonicalize().map_err(error)?;
        let text = std::fs::read_to_string(&path).map_err(error)?;
        let modified = modified_time(&path);
        Ok(Self {
            path,
            text,
            modified,
        })
    }

    fn dir(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("/"))
    }

    /// Re-reads the file if its modification time changed. Returns true if it did.
    fn reload_if_changed(&mut self) -> bool {
        let modified = modified_time(&self.path);
        if modified.is_none() || modified == self.modified {
            return false;
        }
        // Editors often save by truncating or renaming, so a failed read is
        // likely mid-save: keep the old text and try again on the next poll.
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                self.text = text;
                self.modified = modified;
                true
            }
            Err(_) => false,
        }
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

struct Lector {
    doc: Option<Document>,
    error: Option<String>,
    /// The directory being browsed, if lector was opened on one.
    root: Option<PathBuf>,
    tree: Vec<Node>,
    sidebar_open: bool,
    /// Scroll the sidebar to the current file on the next frame.
    reveal_current: bool,
    /// Previously viewed files, for navigating back after following a link.
    history: Vec<PathBuf>,
    cache: CommonMarkCache,
    title: String,
}

impl Lector {
    fn new(path: Option<PathBuf>) -> Self {
        let mut app = Self {
            doc: None,
            error: None,
            root: None,
            tree: Vec::new(),
            sidebar_open: true,
            reveal_current: false,
            history: Vec::new(),
            cache: CommonMarkCache::default(),
            title: String::new(),
        };
        if let Some(path) = path {
            app.open_path(&path);
        }
        app
    }

    /// Opens a dropped or command-line path, which may be a file or a directory.
    fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            self.open_dir(path);
        } else {
            self.open(path, true);
        }
    }

    fn open_dir(&mut self, path: &Path) {
        let root = match path.canonicalize() {
            Ok(root) => root,
            Err(e) => {
                self.error = Some(format!("{}: {e}", path.display()));
                return;
            }
        };
        self.tree = tree::scan(&root);
        self.root = Some(root);
        self.doc = None;
        self.error = None;
        self.history.clear();
        if let Some(first) = tree::initial_file(&self.tree).map(Path::to_path_buf) {
            self.open(&first, false);
        }
    }

    fn rescan(&mut self) {
        if let Some(root) = &self.root {
            self.tree = tree::scan(root);
        }
    }

    fn open(&mut self, path: &Path, remember_current: bool) {
        match Document::load(path) {
            Ok(doc) => {
                if remember_current && let Some(current) = self.doc.take() {
                    self.history.push(current.path);
                }
                self.doc = Some(doc);
                self.error = None;
                self.reveal_current = true;
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn back(&mut self) {
        if let Some(path) = self.history.pop() {
            self.open(&path, false);
        }
    }

    fn handle_input(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};

        let dropped = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(path) = dropped {
            self.open_path(&path);
        }

        // Pick up files added or removed while the window was in the background.
        let focused = ctx.input(|i| i.events.contains(&egui::Event::WindowFocused(true)));
        if focused {
            self.rescan();
        }

        let pressed = |modifiers, key| {
            ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(modifiers, key)))
        };
        if pressed(Modifiers::NONE, Key::Backspace) || pressed(Modifiers::ALT, Key::ArrowLeft) {
            self.back();
        }
        if pressed(Modifiers::COMMAND, Key::R) || pressed(Modifiers::NONE, Key::F5) {
            self.rescan();
            if let Some(path) = self.doc.as_ref().map(|d| d.path.clone()) {
                ctx.forget_all_images();
                self.open(&path, false);
            }
        }
        // F4 toggles the navigation pane in Acrobat; Ctrl+B is the editor habit.
        if pressed(Modifiers::NONE, Key::F4) || pressed(Modifiers::COMMAND, Key::B) {
            self.sidebar_open = !self.sidebar_open;
        }
        if pressed(Modifiers::COMMAND, Key::Q) || pressed(Modifiers::COMMAND, Key::W) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.doc {
            Some(doc) => {
                let name = doc.path.file_name().unwrap_or_default().to_string_lossy();
                format!("{name} — lector")
            }
            None => "lector".to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Takes over clicks on links to local Markdown files so they open in
    /// lector instead of the system handler.
    fn intercept_links(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.doc else { return };
        let dir = doc.dir().to_path_buf();

        let mut target = None;
        ctx.output_mut(|o| {
            for command in &mut o.commands {
                let egui::OutputCommand::OpenUrl(open) = command else {
                    continue;
                };
                match resolve_local_link(&dir, &open.url) {
                    Some(path) if is_markdown(&path) => target = Some(path),
                    // Relative links to other files would otherwise be
                    // resolved against the working directory, not the document.
                    Some(path) => open.url = format!("file://{}", path.display()),
                    None => {}
                }
            }
            if target.is_some() {
                o.commands
                    .retain(|c| !matches!(c, egui::OutputCommand::OpenUrl(_)));
            }
        });
        if let Some(path) = target {
            self.open(&path, true);
        }
    }

    fn show_document(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.doc else { return };

        // One scroll position per file, so going back lands where you were.
        egui::ScrollArea::vertical()
            .id_salt(&doc.path)
            .auto_shrink(false)
            .show(ui, |ui| {
                let available = ui.available_width();
                let width = (available - 2.0 * GUTTER).clamp(0.0, READING_WIDTH);
                ui.horizontal_top(|ui| {
                    ui.add_space((available - width) / 2.0);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.add_space(24.0);
                        CommonMarkViewer::new()
                            .default_implicit_uri_scheme(format!("file://{}/", doc.dir().display()))
                            .max_image_width(Some(width as usize))
                            .enable_scroll_to_heading(true)
                            .show(ui, &mut self.cache, &doc.text);
                        ui.add_space(48.0);
                    });
                });
            });
    }

    fn show_sidebar(&mut self, ui: &mut egui::Ui) {
        let current = self.doc.as_ref().map(|d| d.path.as_path());
        let reveal = std::mem::take(&mut self.reveal_current);
        let mut clicked = None;

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(240.0)
            .size_range(140.0..=600.0)
            .show_collapsible(ui, &mut self.sidebar_open, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        show_tree(ui, &self.tree, current, reveal, &mut clicked);
                    });
            });

        if let Some(path) = clicked {
            self.open(&path, true);
        }
    }

    fn show_empty(&self, ui: &mut egui::Ui) {
        let message = match &self.root {
            Some(root) => format!("No Markdown files in {}", root.display()),
            None => "Drop a Markdown file or folder here\nor run: lector FILE.md | DIR".to_owned(),
        };
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new(message).size(18.0).weak());
        });
    }
}

impl eframe::App for Lector {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.handle_input(&ctx);
        if let Some(doc) = &mut self.doc {
            doc.reload_if_changed();
        }
        self.update_title(&ctx);

        if self.root.is_some() {
            self.show_sidebar(ui);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if self.doc.is_some() {
                self.show_document(ui);
            } else {
                self.show_empty(ui);
            }
        });

        let hovering_file = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering_file {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop_overlay"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(160));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(24.0),
                egui::Color32::WHITE,
            );
        }

        self.intercept_links(&ctx);
        ctx.request_repaint_after(POLL_INTERVAL);
    }
}

fn show_tree(
    ui: &mut egui::Ui,
    nodes: &[Node],
    current: Option<&Path>,
    reveal: bool,
    clicked: &mut Option<PathBuf>,
) {
    for node in nodes {
        match &node.kind {
            Kind::Dir(children) => {
                let contains_current = current.is_some_and(|c| c.starts_with(&node.path));
                egui::CollapsingHeader::new(&node.name)
                    .id_salt(&node.path)
                    .default_open(contains_current)
                    // Unfold the folders leading to a file opened by a link.
                    .open((reveal && contains_current).then_some(true))
                    .show(ui, |ui| show_tree(ui, children, current, reveal, clicked));
            }
            Kind::File => {
                let selected = current == Some(node.path.as_path());
                let response = ui.selectable_label(selected, &node.name);
                if response.clicked() && !selected {
                    *clicked = Some(node.path.clone());
                }
                if selected && reveal {
                    response.scroll_to_me(None);
                }
            }
        }
    }
}

/// Resolves a link from a document in `dir` to a local path. Returns `None`
/// for remote URLs, `mailto:` links and in-page `#anchors`.
fn resolve_local_link(dir: &Path, url: &str) -> Option<PathBuf> {
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.starts_with('#') || url.contains("://") || url.starts_with("mailto:") {
        return None;
    }
    let path = url.split(['#', '?']).next().unwrap_or(url);
    if path.is_empty() {
        return None;
    }
    Some(dir.join(percent_decode(path)))
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        MARKDOWN_EXTENSIONS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(e))
    })
}

/// Decodes `%XX` escapes, e.g. the `%20` in `[notes](my%20notes.md)`.
fn percent_decode(s: &str) -> String {
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
    fn resolves_relative_markdown_links() {
        let dir = Path::new("/docs");
        assert_eq!(
            resolve_local_link(dir, "guide.md#setup"),
            Some(PathBuf::from("/docs/guide.md"))
        );
        assert_eq!(
            resolve_local_link(dir, "my%20notes.md"),
            Some(PathBuf::from("/docs/my notes.md"))
        );
        assert_eq!(
            resolve_local_link(dir, "/abs/x.md"),
            Some(PathBuf::from("/abs/x.md"))
        );
        assert_eq!(
            resolve_local_link(dir, "file:///abs/x.md"),
            Some(PathBuf::from("/abs/x.md"))
        );
    }

    #[test]
    fn ignores_remote_and_anchor_links() {
        let dir = Path::new("/docs");
        assert_eq!(resolve_local_link(dir, "https://example.com/a.md"), None);
        assert_eq!(resolve_local_link(dir, "mailto:me@example.com"), None);
        assert_eq!(resolve_local_link(dir, "#install"), None);
    }

    #[test]
    fn detects_markdown_extensions() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown(Path::new("image.png")));
        assert!(!is_markdown(Path::new("LICENSE")));
    }

    #[test]
    fn percent_decode_leaves_invalid_escapes() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
