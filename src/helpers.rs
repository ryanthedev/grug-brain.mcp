use regex::Regex;
use std::sync::LazyLock;

static SLUGIFY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("slugify regex"));

/// Validate that a memory `category` or `path` argument is safe.
///
/// Rejects:
/// - empty strings
/// - null bytes
/// - absolute paths (leading `/`)
/// - parent traversal (`..` component) and `.` component
/// - shell metacharacters and control characters
///
/// This runs *before* `slugify` so that the original user input cannot escape
/// the brain directory or smuggle metacharacters into downstream tools (git,
/// shell expansion, log lines). After validation the value is still slugified
/// for filesystem use.
pub fn validate_memory_path(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("path is empty".to_string());
    }
    if s.contains('\0') {
        return Err("path contains null byte".to_string());
    }
    if s.starts_with('/') {
        return Err("absolute paths are not allowed".to_string());
    }
    for component in s.split('/') {
        if component == ".." {
            return Err("parent traversal (..) is not allowed".to_string());
        }
        if component == "." {
            return Err("current-dir reference (.) is not allowed".to_string());
        }
    }
    // Shell metacharacters and control whitespace -- never legal in a path token,
    // even before slugification, because raw values appear in error messages and
    // command logs.
    const FORBIDDEN: &[char] = &[
        ';', '|', '&', '$', '`', '<', '>', '*', '?', '!', '(', ')', '{', '}',
        '[', ']', '\\', '\n', '\r', '\t',
    ];
    for c in s.chars() {
        if FORBIDDEN.contains(&c) {
            return Err(format!("path contains forbidden character: {c:?}"));
        }
    }
    Ok(())
}

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
    let total_pages = lines.len().div_ceil(PAGE_SIZE);
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
    fn test_dw_1_4_validate_memory_path_accepts_normal() {
        assert!(validate_memory_path("notes").is_ok());
        assert!(validate_memory_path("notes/sub").is_ok());
        assert!(validate_memory_path("My Notes").is_ok());
        assert!(validate_memory_path("a-b_c.d").is_ok());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_empty() {
        assert!(validate_memory_path("").is_err());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_null_byte() {
        assert!(validate_memory_path("notes\0bad").is_err());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_absolute() {
        assert!(validate_memory_path("/etc/passwd").is_err());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_parent_traversal() {
        assert!(validate_memory_path("..").is_err());
        assert!(validate_memory_path("../escape").is_err());
        assert!(validate_memory_path("notes/../escape").is_err());
        assert!(validate_memory_path("notes/..").is_err());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_dot_component() {
        assert!(validate_memory_path(".").is_err());
        assert!(validate_memory_path("notes/./x").is_err());
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_shell_metachars() {
        for c in [';', '|', '&', '$', '`', '<', '>', '*', '?', '!', '(', ')', '{', '}', '[', ']', '\\'] {
            let s = format!("bad{c}name");
            assert!(
                validate_memory_path(&s).is_err(),
                "expected rejection for {c:?} in {s:?}"
            );
        }
    }

    #[test]
    fn test_dw_1_4_validate_memory_path_rejects_control_whitespace() {
        assert!(validate_memory_path("a\nb").is_err());
        assert!(validate_memory_path("a\rb").is_err());
        assert!(validate_memory_path("a\tb").is_err());
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
