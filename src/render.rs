//! Markdown to sanitized HTML, and splitting large documents into shards
//! that render independently.

use std::sync::LazyLock;

use comrak::options::Plugins;
use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use comrak::{Options, markdown_to_html_with_plugins};

/// Highlights fenced code with `hl-*` classes; the colors live in style.css.
static HIGHLIGHTER: LazyLock<SyntectAdapter> = LazyLock::new(|| {
    SyntectAdapterBuilder::new()
        .css_with_class_prefix("hl-")
        .build()
});

/// Raw HTML in Markdown is allowed (READMEs lean on `<img>`, `<details>`,
/// `<p align>`), but scripts, styles and event handlers are stripped.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(["class", "id", "align"])
        .add_tags(["input"])
        .add_tag_attributes("input", ["type", "checked", "disabled"]);
    builder
});

/// Loads the syntax definitions and sanitizer rules ahead of the first
/// document; meant to run on a background thread at startup.
pub fn warm_up() {
    LazyLock::force(&HIGHLIGHTER);
    LazyLock::force(&SANITIZER);
}

/// Documents smaller than this render in one piece.
const SHARD_THRESHOLD: usize = 256 * 1024;
/// The first shard is small so the page appears at once; the rest are larger.
const FIRST_SHARD_BYTES: usize = 24 * 1024;
const SHARD_BYTES: usize = 96 * 1024;

/// Splits a large document into shards of Markdown that render on their own
/// and, concatenated, read the same as the whole.
///
/// Cuts go only before an ATX heading (`# …`) at the start of a line after a
/// blank line, outside fenced code, HTML comments and front matter: there
/// every open block has already ended. Link reference definitions are copied
/// into every shard so `[text][ref]` links still resolve. Documents with
/// footnotes stay whole, since a reference and its definition must render
/// together.
pub fn split(text: &str) -> Vec<String> {
    if text.len() < SHARD_THRESHOLD {
        return vec![text.to_owned()];
    }
    let body = front_matter_len(text);
    let mut cuts = vec![0];
    let mut refs = String::new();
    let mut fence: Option<(u8, usize)> = None;
    let mut in_comment = false;
    let mut prev_blank = true;
    let mut offset = body;

    for line in text[body..].split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.trim_end_matches(['\n', '\r']);
        let trimmed = content.trim_start_matches(' ');
        let indent = content.len() - trimmed.len();

        if let Some((ch, len)) = fence {
            let run = run_len(trimmed, ch);
            if run >= len && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            prev_blank = false;
            continue;
        }
        if in_comment {
            in_comment = !content.contains("-->");
            prev_blank = false;
            continue;
        }
        if let Some(ch @ (b'`' | b'~')) = trimmed.bytes().next()
            && run_len(trimmed, ch) >= 3
        {
            fence = Some((ch, run_len(trimmed, ch)));
            prev_blank = false;
            continue;
        }
        if trimmed.starts_with("<!--") && !trimmed.contains("-->") {
            in_comment = true;
        }
        if indent <= 3 {
            match definition(trimmed) {
                Some(Definition::Footnote) => return vec![text.to_owned()],
                Some(Definition::Link) => {
                    refs.push_str(content);
                    refs.push('\n');
                }
                None => {}
            }
        }

        let target = if cuts.len() == 1 {
            FIRST_SHARD_BYTES
        } else {
            SHARD_BYTES
        };
        let size = start - cuts.last().unwrap();
        if prev_blank && indent == 0 && is_atx_heading(content) && size >= target {
            cuts.push(start);
        }
        prev_blank = content.trim().is_empty();
    }

    if cuts.len() == 1 {
        return vec![text.to_owned()];
    }
    let ends = cuts.iter().skip(1).copied().chain([text.len()]);
    cuts.iter()
        .zip(ends)
        .map(|(&start, end)| {
            let mut shard = text[start..end].to_owned();
            if !refs.is_empty() {
                shard.push_str("\n\n");
                shard.push_str(&refs);
            }
            shard
        })
        .collect()
}

/// Length of YAML front matter (`---` … `---`) at the start, if any.
fn front_matter_len(text: &str) -> usize {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return 0;
    };
    let mut offset = text.len() - rest.len();
    for line in rest.split_inclusive('\n') {
        offset += line.len();
        if line.trim_end() == "---" {
            return offset;
        }
    }
    0
}

fn run_len(s: &str, ch: u8) -> usize {
    s.bytes().take_while(|&b| b == ch).count()
}

fn is_atx_heading(line: &str) -> bool {
    let level = run_len(line, b'#');
    (1..=6).contains(&level) && matches!(line.as_bytes().get(level), None | Some(b' ' | b'\t'))
}

enum Definition {
    Link,
    Footnote,
}

/// Recognizes `[label]: …` (a link reference) and `[^label]: …` (a footnote).
fn definition(line: &str) -> Option<Definition> {
    let label = line.strip_prefix('[')?;
    let close = label.find(']')?;
    if !label[close + 1..].starts_with(':') || close == 0 {
        return None;
    }
    Some(if label.starts_with('^') {
        Definition::Footnote
    } else {
        Definition::Link
    })
}

pub fn markdown(text: &str) -> String {
    let mut options = Options::default();
    let ext = &mut options.extension;
    ext.strikethrough = true;
    ext.table = true;
    ext.autolink = true;
    ext.tasklist = true;
    ext.footnotes = true;
    ext.alerts = true;
    ext.header_id_prefix = Some(String::new());
    ext.front_matter_delimiter = Some("---".to_owned());
    options.render.r#unsafe = true;
    options.render.tasklist_classes = true;

    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&*HIGHLIGHTER);

    let html = markdown_to_html_with_plugins(text, &options, &plugins);
    SANITIZER.clean(&html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document over the shard threshold: `sections` chapters of filler.
    fn big_doc(sections: usize) -> String {
        let filler = "Some words to fill the section out a little.\n\n".repeat(200);
        (0..sections)
            .map(|i| format!("# Chapter {i}\n\n{filler}"))
            .collect()
    }

    #[test]
    fn small_documents_stay_whole() {
        assert_eq!(split("# A\n\ntext\n"), vec!["# A\n\ntext\n".to_owned()]);
    }

    #[test]
    fn splits_before_headings_and_loses_nothing() {
        let doc = big_doc(60);
        let shards = split(&doc);
        assert!(shards.len() > 5, "{} shards", shards.len());
        assert!(
            shards[0].len() < shards[1].len(),
            "first shard is the small one"
        );
        assert!(shards[1..].iter().all(|s| s.starts_with("# Chapter")));
        assert_eq!(shards.concat(), doc);
    }

    #[test]
    fn never_splits_inside_fenced_code_or_comments() {
        let fenced = format!("{}```md\n\n# not a heading\n\n```\n", big_doc(10));
        let commented = format!("{}<!--\n\n# not a heading\n\n-->\n", big_doc(10));
        for doc in [fenced, commented] {
            let doc = doc.repeat(2);
            assert!(
                split(&doc)
                    .iter()
                    .all(|s| !s.starts_with("# not a heading"))
            );
        }
    }

    #[test]
    fn copies_link_definitions_into_every_shard() {
        let doc = format!(
            "See [the site][site].\n\n{}[site]: https://example.com\n",
            big_doc(60)
        );
        let shards = split(&doc);
        assert!(shards.len() > 1);
        assert!(
            shards
                .iter()
                .all(|s| s.contains("[site]: https://example.com"))
        );
        assert!(markdown(&shards[0]).contains(r#"href="https://example.com""#));
    }

    #[test]
    fn keeps_documents_with_footnotes_whole() {
        let doc = format!("Claim.[^1]\n\n{}[^1]: Source.\n", big_doc(60));
        assert_eq!(split(&doc).len(), 1);
    }

    #[test]
    fn front_matter_comments_are_not_headings() {
        let doc = format!("---\n# yaml comment\ntitle: x\n---\n\n{}", big_doc(60));
        let shards = split(&doc);
        assert!(shards[0].starts_with("---\n# yaml comment"));
        assert!(!markdown(&shards[0]).contains("yaml comment"));
    }

    #[test]
    fn strips_scripts_and_handlers() {
        let html = markdown("<script>alert(1)</script>\n\n<img src=x.png onerror=alert(1)>");
        assert!(!html.contains("script"), "{html}");
        assert!(!html.contains("onerror"), "{html}");
        assert!(html.contains(r#"src="x.png""#), "{html}");
    }

    #[test]
    fn keeps_github_features() {
        let html =
            markdown("# Title\n\n- [x] done\n\n> [!NOTE]\n> hi\n\n```rust\nfn main() {}\n```\n");
        assert!(html.contains(r#"id="title""#), "{html}");
        assert!(html.contains(r#"type="checkbox""#), "{html}");
        assert!(html.contains("markdown-alert-note"), "{html}");
        assert!(html.contains("hl-storage"), "{html}");
    }

    #[test]
    fn hides_front_matter() {
        let html = markdown("---\ntitle: x\n---\n\nBody");
        assert!(!html.contains("title: x"), "{html}");
    }
}
