//! lector — a minimal Markdown viewer and explorer.
//!
//! Usage: `lege [FILE.md | DIR]`, or drop a file or folder onto the window.
//! A directory opens with a sidebar listing its Markdown files. The open file
//! reloads automatically when it changes on disk.
//!
//! The UI is a system webview: Rust renders Markdown and serves it over the
//! `lector://` scheme (see `server`), and `assets/app.js` handles navigation.

mod render;
mod search;
mod server;
mod tree;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use tao::dpi::LogicalSize;
#[cfg(target_os = "macos")]
use tao::event::StartCause;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Theme, Window, WindowBuilder};
use wry::{DragDropEvent, NewWindowResponse, WebView, WebViewBuilder};

use server::{ORIGIN, State};

const USAGE: &str = "\
lege — a minimal Markdown viewer and explorer

Usage: lege [FILE.md | DIR]

  FILE.md   view a Markdown file
  DIR       browse a folder of Markdown files
  (none)    open an empty window; drop a file or folder onto it

Options:
  -h, --help     print this help
  -V, --version  print the version
";

/// The app icon: assets/icon.svg rendered by `make icon`.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

enum UserEvent {
    Open(PathBuf),
    Quit,
}

fn main() {
    let arg = std::env::args_os().nth(1);
    match arg.as_ref().and_then(|a| a.to_str()) {
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return;
        }
        Some("-V" | "--version") => {
            println!("lege {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        _ => {}
    }

    // Load syntax highlighting while the window comes up.
    std::thread::spawn(render::warm_up);

    let state = Arc::new(State::default());
    let start = match arg {
        Some(arg) => target_url(Path::new(&arg), &state).unwrap_or_else(|e| {
            eprintln!("lege: {e}");
            std::process::exit(1);
        }),
        None => format!("{ORIGIN}/"),
    };

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    let menu = macos_menu();
    let proxy = event_loop.create_proxy();
    let window = Rc::new(
        WindowBuilder::new()
            .with_title("lector")
            .with_inner_size(LogicalSize::new(1000.0, 1100.0))
            .with_min_inner_size(LogicalSize::new(360.0, 300.0))
            .with_window_icon(window_icon())
            .build(&event_loop)
            .expect("failed to create window"),
    );
    // Matches the canvas color in style.css, to avoid a white flash at startup.
    let background = match window.theme() {
        Theme::Dark => (18, 19, 21, 255),
        _ => (214, 215, 219, 255),
    };

    let builder = WebViewBuilder::new()
        // Each request is answered on its own thread: rendering a large
        // document must never block the UI thread the webview runs on.
        .with_asynchronous_custom_protocol("lector".into(), {
            let state = state.clone();
            move |_, request, responder| {
                let state = state.clone();
                std::thread::spawn(move || responder.respond(server::handle(&state, &request)));
            }
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
        #[cfg(target_os = "macos")]
        let _keep_menu_alive = &menu;
        *control_flow = ControlFlow::Wait;
        match event {
            // Set after launch, so macOS doesn't replace it with the default.
            #[cfg(target_os = "macos")]
            Event::NewEvents(StartCause::Init) => set_dock_icon(),
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
            | Event::UserEvent(UserEvent::Quit) => *control_flow = ControlFlow::Exit,
            Event::UserEvent(UserEvent::Open(path)) => match target_url(&path, &state) {
                Ok(url) => {
                    let _ = webview.load_url(&url);
                }
                Err(e) => eprintln!("lege: {e}"),
            },
            _ => {}
        }
    });
}

/// Works out what to show for a path given on the command line or dropped
/// on the window, updating the browsed directory. Returns the URL to load.
fn target_url(path: &Path, state: &Arc<State>) -> Result<String, String> {
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;

    if path.is_dir() {
        let first = tree::initial_file(&tree::scan(&path)).map(server::url_for);
        state.set_root(Some(path));
        return Ok(first.unwrap_or_else(|| format!("{ORIGIN}/")));
    }
    if !tree::is_markdown(&path) {
        return Err(format!("{}: not a Markdown file", path.display()));
    }
    // A file from outside the browsed directory leaves directory mode.
    if state.root().is_some_and(|root| !path.starts_with(root)) {
        state.set_root(None);
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
        Err(e) => eprintln!("lege: cannot open {target}: {e}"),
    }
}

/// The window icon, for desktops that take it from the window (X11). Wayland
/// compositors look up the icon of the matching desktop entry instead, which
/// `make install` provides.
#[cfg(not(target_os = "macos"))]
fn window_icon() -> Option<tao::window::Icon> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(ICON_PNG))
        .read_info()
        .ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    rgba.truncate(info.buffer_size());
    tao::window::Icon::from_rgba(rgba, info.width, info.height).ok()
}

/// macOS windows have no icon of their own; a bare binary shows the Dock icon.
#[cfg(target_os = "macos")]
fn window_icon() -> Option<tao::window::Icon> {
    None
}

#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // SAFETY: the only caveat on this method is that it may reject nil.
        unsafe {
            NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image))
        };
    }
}

/// macOS delivers Copy, Paste and Select All to a webview only through the
/// app's Edit menu, so lege needs a minimal menu bar there. The returned menu
/// must be kept alive.
#[cfg(target_os = "macos")]
fn macos_menu() -> muda::Menu {
    use muda::{AboutMetadata, Menu, PredefinedMenuItem as Item, Submenu};

    let about = AboutMetadata {
        name: Some("Lege".to_owned()),
        version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        ..Default::default()
    };
    let app = Submenu::with_items(
        "Lege",
        true,
        &[
            &Item::about(None, Some(about)),
            &Item::separator(),
            &Item::hide(None),
            &Item::hide_others(None),
            &Item::show_all(None),
            &Item::separator(),
            &Item::quit(None),
        ],
    );
    let edit = Submenu::with_items(
        "Edit",
        true,
        &[
            &Item::undo(None),
            &Item::redo(None),
            &Item::separator(),
            &Item::cut(None),
            &Item::copy(None),
            &Item::paste(None),
            &Item::select_all(None),
        ],
    );
    let window = Submenu::with_items(
        "Window",
        true,
        &[
            &Item::minimize(None),
            &Item::fullscreen(None),
            &Item::close_window(None),
        ],
    );
    let (app, edit, window) = (app.unwrap(), edit.unwrap(), window.unwrap());
    let menu = Menu::with_items(&[&app, &edit, &window]).expect("failed to build the menu bar");
    menu.init_for_nsapp();
    menu
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

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn embedded_icon_decodes() {
        assert!(
            super::window_icon().is_some(),
            "assets/icon.png must be 8-bit RGBA"
        );
    }
}
