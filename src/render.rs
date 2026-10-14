//! Markdown to sanitized HTML.

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
    use super::markdown;

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
