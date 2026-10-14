//! The Markdown file tree shown in the sidebar in directory mode.

use std::fmt::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::server::{escape, url_for};

const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd"];

pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        MARKDOWN_EXTENSIONS
            .iter()
            .any(|m| m.eq_ignore_ascii_case(e))
    })
}

pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
}

pub enum Kind {
    File,
    /// Only directories that contain Markdown somewhere below them are kept.
    Dir(Vec<Node>),
}

/// Collects the Markdown files under `root`, skipping hidden and
/// `.gitignore`d entries the way ripgrep does.
pub fn scan(root: &Path) -> Vec<Node> {
    let files = WalkBuilder::new(root)
        .require_git(false)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()) && is_markdown(e.path()))
        .map(|e| e.into_path());

    let mut nodes = Vec::new();
    for file in files {
        if let Ok(relative) = file.strip_prefix(root) {
            let parts: Vec<_> = relative.iter().collect();
            insert(&mut nodes, root, &parts);
        }
    }
    sort(&mut nodes);
    nodes
}

fn insert(nodes: &mut Vec<Node>, dir: &Path, parts: &[&std::ffi::OsStr]) {
    let [first, rest @ ..] = parts else { return };
    let path = dir.join(first);
    let name = first.to_string_lossy().into_owned();

    if rest.is_empty() {
        nodes.push(Node {
            name,
            path,
            kind: Kind::File,
        });
        return;
    }
    let index = nodes
        .iter()
        .position(|n| matches!(n.kind, Kind::Dir(_)) && n.path == path)
        .unwrap_or_else(|| {
            nodes.push(Node {
                name,
                path: path.clone(),
                kind: Kind::Dir(Vec::new()),
            });
            nodes.len() - 1
        });
    if let Kind::Dir(children) = &mut nodes[index].kind {
        insert(children, &path, rest);
    }
}

/// Directories first, then files, each alphabetically ignoring case.
fn sort(nodes: &mut [Node]) {
    nodes.sort_by_cached_key(|n| (matches!(n.kind, Kind::File), n.name.to_lowercase()));
    for node in nodes {
        if let Kind::Dir(children) = &mut node.kind {
            sort(children);
        }
    }
}

/// The file to show first when opening a directory: a top-level README or
/// index if there is one, then any top-level file, then the first file in
/// the tree.
pub fn initial_file(nodes: &[Node]) -> Option<&Path> {
    let mut top_level = nodes.iter().filter(|n| matches!(n.kind, Kind::File));
    for wanted in ["readme", "index"] {
        let found = top_level.clone().find(|n| {
            n.path
                .file_stem()
                .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(wanted))
        });
        if let Some(node) = found {
            return Some(&node.path);
        }
    }
    top_level
        .next()
        .map(|n| n.path.as_path())
        .or_else(|| first_file(nodes))
}

fn first_file(nodes: &[Node]) -> Option<&Path> {
    nodes.iter().find_map(|n| match &n.kind {
        Kind::File => Some(n.path.as_path()),
        Kind::Dir(children) => first_file(children),
    })
}

/// Renders the tree as nested lists: folders are `<details>`, so they fold
/// without any script, and files are links to their viewer URLs.
pub fn to_html(nodes: &[Node]) -> String {
    let mut out = String::new();
    write_html(nodes, &mut out);
    out
}

fn write_html(nodes: &[Node], out: &mut String) {
    out.push_str("<ul>");
    for node in nodes {
        out.push_str("<li>");
        match &node.kind {
            Kind::Dir(children) => {
                let path = escape(&node.path.to_string_lossy());
                let _ = write!(
                    out,
                    r#"<details data-path="{path}"><summary>{}</summary>"#,
                    escape(&node.name)
                );
                write_html(children, out);
                out.push_str("</details>");
            }
            Kind::File => {
                // Show "guide" rather than "guide.md"; the full name is in the tooltip.
                let label = node
                    .path
                    .file_stem()
                    .map_or(node.name.clone(), |s| s.to_string_lossy().into_owned());
                let _ = write!(
                    out,
                    r#"<a href="{}" title="{}">{}</a>"#,
                    escape(&url_for(&node.path)),
                    escape(&node.name),
                    escape(&label),
                );
            }
        }
        out.push_str("</li>");
    }
    out.push_str("</ul>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.name.as_str()).collect()
    }

    #[test]
    fn builds_sorted_tree_of_markdown_files_only() {
        let root = std::env::temp_dir().join(format!("lector-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["docs/deep", "empty", "assets", ".hidden"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "b.md",
            "A.md",
            "docs/guide.md",
            "docs/deep/x.markdown",
            "assets/logo.png",
            ".hidden/secret.md",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }

        let tree = scan(&root);
        assert_eq!(names(&tree), ["docs", "A.md", "b.md"]);
        let Kind::Dir(docs) = &tree[0].kind else {
            panic!("docs is not a dir")
        };
        assert_eq!(names(docs), ["deep", "guide.md"]);
        assert_eq!(initial_file(&tree), Some(root.join("A.md").as_path()));

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn detects_markdown_extensions() {
        assert!(is_markdown(Path::new("README.md")));
        assert!(is_markdown(Path::new("notes.MARKDOWN")));
        assert!(!is_markdown(Path::new("image.png")));
        assert!(!is_markdown(Path::new("LICENSE")));
    }

    #[test]
    fn renders_tree_as_escaped_html() {
        let tree = vec![Node {
            name: "a&b".into(),
            path: PathBuf::from("/r/a&b"),
            kind: Kind::Dir(vec![Node {
                name: "<x>.md".into(),
                path: PathBuf::from("/r/a&b/<x>.md"),
                kind: Kind::File,
            }]),
        }];
        let html = to_html(&tree);
        assert!(html.contains("<summary>a&amp;b</summary>"), "{html}");
        assert!(html.contains(">&lt;x&gt;</a>"), "{html}");
        assert!(html.contains("/r/a%26b/%3Cx%3E.md"), "{html}");
    }

    #[test]
    fn prefers_readme_as_initial_file() {
        let file = |name: &str| Node {
            name: name.into(),
            path: PathBuf::from(name),
            kind: Kind::File,
        };
        let tree = vec![file("CHANGELOG.md"), file("README.md")];
        assert_eq!(initial_file(&tree), Some(Path::new("README.md")));
    }
}
