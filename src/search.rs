//! Search across the browsed files: fuzzy matching on file names, and plain
//! case-insensitive matching on their text.
//!
//! Text matching folds ASCII case only, which keeps byte offsets in the
//! lowercased copy identical to the original, so snippets can be cut from it.

use std::path::PathBuf;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Most file-name results shown.
pub const MAX_NAMES: usize = 8;
/// Most text matches listed per file, and in total.
pub const MAX_HITS_PER_FILE: usize = 5;
pub const MAX_HITS: usize = 60;
/// Bytes of context shown on each side of a text match.
const CONTEXT: usize = 56;

pub struct NameHit {
    pub path: PathBuf,
    pub label: String,
    /// Character indices in `label` that matched, ascending.
    pub matched: Vec<u32>,
}

pub struct TextHit {
    /// Which occurrence in the file this is, counting from 0.
    pub occurrence: usize,
    pub before: String,
    pub matched: String,
    pub after: String,
}

/// Ranks `files` (path and display label) by how well the label fuzzy-matches
/// `query`, best first.
pub fn names(query: &str, files: &[(PathBuf, String)]) -> Vec<NameHit> {
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut hits: Vec<(u32, NameHit)> = files
        .iter()
        .filter_map(|(path, label)| {
            let mut matched = Vec::new();
            let score =
                pattern.indices(Utf32Str::new(label, &mut buf), &mut matcher, &mut matched)?;
            matched.sort_unstable();
            matched.dedup();
            let hit = NameHit {
                path: path.clone(),
                label: label.clone(),
                matched,
            };
            Some((score, hit))
        })
        .collect();
    hits.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.label.cmp(&y.label)));
    hits.into_iter()
        .take(MAX_NAMES)
        .map(|(_, hit)| hit)
        .collect()
}

/// Finds `needle` (already ASCII-lowercased) in `text`, given its
/// ASCII-lowercased copy `lower`. Returns up to `limit` matches with their
/// context, and the total number of matches.
pub fn text(needle: &str, text: &str, lower: &str, limit: usize) -> (Vec<TextHit>, usize) {
    if needle.is_empty() {
        return (Vec::new(), 0);
    }
    let mut hits = Vec::new();
    let mut total = 0;
    for (start, _) in lower.match_indices(needle) {
        if hits.len() < limit {
            hits.push(snippet(text, start, start + needle.len(), total));
        }
        total += 1;
    }
    (hits, total)
}

/// The match and some context from its line, with Markdown block markers
/// (`#`, `>`, `-`, …) dropped from the line start and `…` where text is cut.
fn snippet(text: &str, start: usize, end: usize, occurrence: usize) -> TextHit {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);

    let from = text.floor_char_boundary(start.saturating_sub(CONTEXT).max(line_start));
    let to = text.ceil_char_boundary((end + CONTEXT).min(line_end));

    let mut before = text[from..start].to_owned();
    if from == line_start {
        before = before
            .trim_start_matches(['#', '>', '-', '*', '+', ' ', '\t'])
            .to_owned();
    } else {
        before.insert(0, '…');
    }
    let mut after = text[end..to].trim_end().to_owned();
    if to < line_end {
        after.push('…');
    }
    TextHit {
        occurrence,
        before,
        matched: text[start..end].to_owned(),
        after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(needle: &str, haystack: &str, limit: usize) -> (Vec<TextHit>, usize) {
        text(needle, haystack, &haystack.to_ascii_lowercase(), limit)
    }

    #[test]
    fn finds_text_ignoring_case_and_keeps_original_casing() {
        let (hits, total) = search("sidebar", "The Sidebar.\nA sidebar here.\n", 10);
        assert_eq!(total, 2);
        assert_eq!(hits[0].matched, "Sidebar");
        assert_eq!(
            (hits[0].before.as_str(), hits[0].after.as_str()),
            ("The ", ".")
        );
        assert_eq!(hits[1].occurrence, 1);
    }

    #[test]
    fn counts_every_match_but_lists_only_the_limit() {
        let (hits, total) = search("ab", &"ab ".repeat(20), 3);
        assert_eq!((hits.len(), total), (3, 20));
    }

    #[test]
    fn snippets_stay_on_one_line_and_mark_cuts() {
        let line = format!("{} needle {}", "x".repeat(100), "y".repeat(100));
        let (hits, _) = search("needle", &format!("before\n{line}\nafter"), 1);
        let hit = &hits[0];
        assert!(hit.before.starts_with('…') && hit.after.ends_with('…'));
        assert!(!hit.before.contains("before") && !hit.after.contains("after"));
    }

    #[test]
    fn snippets_drop_markdown_markers_and_respect_utf8() {
        let (hits, _) = search("title", "## Title of Ünïcödé\n", 1);
        assert_eq!(hits[0].before, "");
        assert_eq!(hits[0].after, " of Ünïcödé");
        let wide = format!("{}target", "é".repeat(60));
        assert_eq!(search("target", &wide, 1).0[0].matched, "target");
    }

    #[test]
    fn ranks_file_names_by_fuzzy_match() {
        let files: Vec<_> = ["docs/configuration.md", "notes/ideas.md", "docs/api.md"]
            .iter()
            .map(|l| (PathBuf::from(l), l.to_string()))
            .collect();
        let hits = names("cfg", &files);
        assert_eq!(hits[0].label, "docs/configuration.md");
        assert!(hits.iter().all(|h| h.label != "notes/ideas.md"));
        assert!(!hits[0].matched.is_empty());
    }
}
