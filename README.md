# lector

A minimal Markdown viewer and explorer, in the spirit of old-school PDF viewers.
The command is `lege` (Latin for "read!"), short to type.

```sh
lege notes.md   # view a file
lege .          # browse a directory: Markdown files in a collapsible sidebar
```

You can also drop a file or folder onto the window. The open file reloads when
it changes on disk, and links to other local `.md` files open in place; web
links and other files open in their default apps.

The sidebar skips hidden and `.gitignore`d paths.

## Keys

| Key                       | Action                  |
|---------------------------|-------------------------|
| `F4` / `Ctrl+B`           | Toggle the sidebar      |
| `Backspace` / `Alt+←`     | Back                    |
| `Alt+→`                   | Forward                 |
| `Ctrl+R` / `F5`           | Reload                  |
| `Ctrl +` / `Ctrl -` / `Ctrl 0` | Zoom              |
| `Ctrl+Q` / `Ctrl+W`       | Quit                    |

On macOS, use `Cmd` instead of `Ctrl`. Drag the sidebar's edge to resize it.

## Build

```sh
make release    # optimized build in dist/lege, plus a tarball
make install    # into ~/.local/bin (or PREFIX=/usr/local)
```

or `cargo install --path .`, which puts `lege` in `~/.cargo/bin`.

lector renders through the system webview: WebKitGTK on Linux
(`webkit2gtk-4.1` on Arch) and WKWebView on macOS, which ships with the OS.
