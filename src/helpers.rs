use regex::Regex;
use std::sync::LazyLock;

static SLUGIFY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("slugify regex"));

/// Convert text to a URL-safe slug.
/// Matches JS: toLowerCase, replace non-alnum with "-", trim dashes, truncate to 80.
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let replaced = SLUGIFY_RE.replace_all(&lower, "-");
    let trimmed = replaced.trim_matches('-');
    let end = trimmed
        .char_indices()
        .take_while(|&(i, _)| i < 80)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    trimmed[..end].to_string()
}

/// Today's date as YYYY-MM-DD.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Paginate a multi-line string. Page numbering starts at 1.
pub const PAGE_SIZE: usize = 50;

pub fn paginate(text: &str, page: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= PAGE_SIZE {
        return text.to_string();
    }
    let total_pages = (lines.len() + PAGE_SIZE - 1) / PAGE_SIZE;
    let p = page.max(1).min(total_pages);
    let start = (p - 1) * PAGE_SIZE;
    let end = (start + PAGE_SIZE).min(lines.len());
    let slice = &lines[start..end];
    format!(
        "{}\n--- page {}/{} ({} lines) | page:{} for more ---",
        slice.join("\n"),
        p,
        total_pages,
        lines.len(),
        p + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("Hello World!"), "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        assert_eq!(slugify("a@b#c"), "a-b-c");
    }

    #[test]
    fn test_slugify_leading_trailing_dashes() {
        assert_eq!(slugify("--hello--"), "hello");
    }

    #[test]
    fn test_slugify_truncate() {
        let long = "a".repeat(100);
        let result = slugify(&long);
        assert_eq!(result.len(), 80);
    }

    #[test]
    fn test_slugify_empty() {
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_today_format() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[7..8], "-");
    }

    #[test]
    fn test_paginate_short() {
        let text = "line1\nline2\nline3";
        assert_eq!(paginate(text, 1), text);
    }

    #[test]
    fn test_paginate_multipage() {
        let lines: Vec<String> = (0..120).map(|i| format!("line {i}")).collect();
        let text = lines.join("\n");

        let page1 = paginate(&text, 1);
        assert!(page1.contains("page 1/3"));
        assert!(page1.contains("120 lines"));

        let page2 = paginate(&text, 2);
        assert!(page2.contains("page 2/3"));
        assert!(page2.starts_with("line 50"));
    }
}
