# lector

A minimal Markdown viewer and explorer, in the spirit of old-school PDF viewers.

```sh
lector notes.md   # view a file
lector .          # browse a directory: Markdown files in a collapsible sidebar
```

You can also drop a file or folder onto the window. The open file reloads when
it changes on disk, and links to other local `.md` files open in place.

The sidebar skips hidden and `.gitignore`d paths.

## Keys

| Key                       | Action                  |
|---------------------------|-------------------------|
| `F4` / `Ctrl+B`           | Toggle the sidebar      |
| `Backspace` / `Alt+←`     | Back                    |
| `Ctrl+R` / `F5`           | Reload                  |
| `Ctrl +` / `Ctrl -`       | Zoom                    |
| `Ctrl+Q` / `Ctrl+W`       | Quit                    |

On macOS, use `Cmd` instead of `Ctrl`. The sidebar edge can also be dragged
to resize or collapse it.

## Build

```sh
cargo install --path .
```
