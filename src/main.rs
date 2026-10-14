//! lector — a minimal Markdown viewer and explorer.
//!
//! Usage: `lector [FILE.md | DIR]`, or drop a file or folder onto the window.
//! A directory opens with a sidebar listing its Markdown files. The open file
//! reloads automatically when it changes on disk.
//!
//! The UI is a system webview: Rust renders Markdown and serves it over the
//! `lector://` scheme (see `server`), and `assets/app.js` handles navigation.

mod render;
mod server;
mod tree;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Theme, Window, WindowBuilder};
use wry::{DragDropEvent, NewWindowResponse, WebView, WebViewBuilder};

use server::{ORIGIN, Root};

enum UserEvent {
    Open(PathBuf),
    Quit,
}

fn main() {
    let root: Root = Arc::new(Mutex::new(None));
    let start = match std::env::args_os().nth(1) {
        Some(arg) => target_url(Path::new(&arg), &root).unwrap_or_else(|e| {
            eprintln!("lector: {e}");
            std::process::exit(1);
        }),
        None => format!("{ORIGIN}/"),
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = Rc::new(
        WindowBuilder::new()
            .with_title("lector")
            .with_inner_size(LogicalSize::new(1000.0, 1100.0))
            .with_min_inner_size(LogicalSize::new(360.0, 300.0))
            .build(&event_loop)
            .expect("failed to create window"),
    );
    // Matches the canvas color in style.css, to avoid a white flash at startup.
    let background = match window.theme() {
        Theme::Dark => (18, 19, 21, 255),
        _ => (214, 215, 219, 255),
    };

    let builder = WebViewBuilder::new()
        .with_custom_protocol("lector".into(), {
            let root = root.clone();
            move |_, request| server::handle(&root, &request)
        })
        .with_url(start)
        .with_background_color(background)
        .with_navigation_handler(allow_navigation)
        .with_new_window_req_handler(|url, _| {
            open_externally(&url);
            NewWindowResponse::Deny
        })
        .with_document_title_changed_handler({
            let window = window.clone();
            move |title| window.set_title(&title)
        })
        .with_ipc_handler({
            let proxy = proxy.clone();
            move |request| {
                if request.body() == "quit" {
                    let _ = proxy.send_event(UserEvent::Quit);
                }
            }
        })
        .with_drag_drop_handler(move |event| {
            if let DragDropEvent::Drop { paths, .. } = event
                && let Some(path) = paths.into_iter().next()
            {
                let _ = proxy.send_event(UserEvent::Open(path));
            }
            // Always handle drops ourselves; the default would navigate to the file.
            true
        });
    let webview = build_webview(builder, &window).expect("failed to create webview");

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(UserEvent::Quit) => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Open(path)) => match target_url(&path, &root) {
                Ok(url) => {
                    let _ = webview.load_url(&url);
                }
                Err(e) => eprintln!("lector: {e}"),
            },
            _ => {}
        }
    });
}

/// Works out what to show for a path given on the command line or dropped
/// on the window, updating the browsed directory. Returns the URL to load.
fn target_url(path: &Path, root: &Root) -> Result<String, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut root = root.lock().unwrap();

    if path.is_dir() {
        let first = tree::initial_file(&tree::scan(&path)).map(server::url_for);
        *root = Some(path);
        return Ok(first.unwrap_or_else(|| format!("{ORIGIN}/")));
    }
    if !tree::is_markdown(&path) {
        return Err(format!("{}: not a Markdown file", path.display()));
    }
    // A file from outside the browsed directory leaves directory mode.
    if !root.as_ref().is_some_and(|r| path.starts_with(r)) {
        *root = None;
    }
    Ok(server::url_for(&path))
}

/// Keeps the webview on Markdown documents; anything else (web links, local
/// PDFs, folders…) goes to the system's default app instead.
fn allow_navigation(url: String) -> bool {
    if let Some(rest) = url.strip_prefix(ORIGIN) {
        let path = server::percent_decode(rest.split(['?', '#']).next().unwrap_or_default());
        if path.is_empty() || path == "/" || tree::is_markdown(Path::new(&path)) {
            return true;
        }
        open_externally(&path);
        return false;
    }
    if url.starts_with("about:") {
        return true;
    }
    open_externally(&url);
    false
}

fn open_externally(target: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(target).spawn() {
        // Reap the child so it doesn't linger as a zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => eprintln!("lector: cannot open {target}: {e}"),
    }
}

#[cfg(not(target_os = "linux"))]
fn build_webview(builder: WebViewBuilder, window: &Window) -> wry::Result<WebView> {
    builder.build(window)
}

#[cfg(target_os = "linux")]
fn build_webview(builder: WebViewBuilder, window: &Window) -> wry::Result<WebView> {
    use tao::platform::unix::WindowExtUnix;
    use wry::WebViewBuilderExtUnix;
    builder.build_gtk(window.default_vbox().expect("window has no GTK container"))
}
