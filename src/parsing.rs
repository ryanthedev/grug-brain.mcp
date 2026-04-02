use std::collections::HashMap;

/// Extract YAML-like frontmatter from markdown content.
/// Matches JS: /^---\n([\s\S]*?)\n---/
/// Returns key-value pairs parsed by splitting on first colon per line.
pub fn extract_frontmatter(content: &str) -> HashMap<String, String> {
    let mut result = HashMap::new();

    if !content.starts_with("---\n") {
        return result;
    }

    // Find closing "\n---" after the opening "---\n"
    let after_open = &content[4..];
    let close_pos = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return result,
    };

    let fm_block = &after_open[..close_pos];

    for line in fm_block.split('\n') {
        if let Some(idx) = line.find(':') {
            if idx > 0 {
                let key = line[..idx].trim().to_string();
                let value = line[idx + 1..].trim().to_string();
                result.insert(key, value);
            }
        }
    }

    result
}

/// Extract the body of a markdown file (everything after frontmatter).
/// Matches JS: content.replace(/^---[\s\S]*?---\n*/, "").trim()
pub fn extract_body(content: &str) -> String {
    if content.starts_with("---") {
        // Find closing "\n---" after position 3
        if let Some(pos) = content[3..].find("\n---") {
            let after_close = pos + 3 + 4; // skip past "\n---"
            if after_close <= content.len() {
                return content[after_close..]
                    .trim_start_matches('\n')
                    .trim()
                    .to_string();
            }
        }
    }
    content.trim().to_string()
}

/// Extract a description from the body: first non-empty, non-header, non-code line.
/// Strips markdown formatting (backticks, underscores, asterisks) and truncates to 120 chars.
pub fn extract_description(content: &str) -> String {
    let body = extract_body(content);
    for line in body.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with("```") {
            continue;
        }
        if trimmed.starts_with(":::") {
            continue;
        }
        if trimmed.starts_with("import ") {
            continue;
        }
        let cleaned: String = trimmed.chars().filter(|c| !matches!(c, '`' | '_' | '*')).collect();
        let end = cleaned
            .char_indices()
            .take_while(|&(i, _)| i < 120)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        return cleaned[..end].to_string();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_frontmatter_basic() {
        let input = "---\nname: test\ntype: note\n---\nbody";
        let fm = extract_frontmatter(input);
        assert_eq!(fm.get("name").unwrap(), "test");
        assert_eq!(fm.get("type").unwrap(), "note");
        assert_eq!(fm.len(), 2);
    }

    #[test]
    fn test_extract_frontmatter_empty() {
        let input = "no frontmatter here";
        let fm = extract_frontmatter(input);
        assert!(fm.is_empty());
    }

    #[test]
    fn test_extract_frontmatter_multiword_value() {
        let input = "---\ndescription: a longer value here\n---\n";
        let fm = extract_frontmatter(input);
        assert_eq!(fm.get("description").unwrap(), "a longer value here");
    }

    #[test]
    fn test_extract_frontmatter_value_with_colon() {
        // Values like URLs contain colons -- only split on first colon
        let input = "---\ngit: https://github.com/foo/bar\n---\n";
        let fm = extract_frontmatter(input);
        assert_eq!(fm.get("git").unwrap(), "https://github.com/foo/bar");
    }

    #[test]
    fn test_extract_body_with_frontmatter() {
        let input = "---\nname: test\n---\n\nBody content here";
        assert_eq!(extract_body(input), "Body content here");
    }

    #[test]
    fn test_extract_body_no_frontmatter() {
        let input = "Just body";
        assert_eq!(extract_body(input), "Just body");
    }

    #[test]
    fn test_extract_body_strips_leading_newlines() {
        let input = "---\nname: test\n---\n\n\n\nBody";
        assert_eq!(extract_body(input), "Body");
    }

    #[test]
    fn test_extract_description_skips_headers() {
        let input = "---\nname: test\n---\n# Header\n\nActual description";
        assert_eq!(extract_description(input), "Actual description");
    }

    #[test]
    fn test_extract_description_skips_code_fence_marker() {
        // JS behavior: only the line starting with ``` is skipped, not content inside
        let input = "---\n---\n```rust\nDescription line";
        assert_eq!(extract_description(input), "Description line");
    }

    #[test]
    fn test_extract_description_skips_admonition_marker() {
        // JS behavior: only the line starting with ::: is skipped, not content inside
        let input = "---\n---\n:::note\nReal desc";
        assert_eq!(extract_description(input), "Real desc");
    }

    #[test]
    fn test_extract_description_skips_imports() {
        let input = "---\n---\nimport Foo from 'bar'\nReal desc";
        assert_eq!(extract_description(input), "Real desc");
    }

    #[test]
    fn test_extract_description_strips_formatting() {
        let input = "---\n---\n**bold** and `code` and _italic_";
        assert_eq!(extract_description(input), "bold and code and italic");
    }

    #[test]
    fn test_extract_description_truncates_120() {
        let long_line = "a".repeat(200);
        let input = format!("---\n---\n{long_line}");
        let desc = extract_description(&input);
        assert_eq!(desc.len(), 120);
    }

    #[test]
    fn test_extract_description_empty_body() {
        let input = "---\nname: test\n---\n";
        assert_eq!(extract_description(input), "");
    }

    #[test]
    fn test_extract_description_only_headers() {
        let input = "---\n---\n# Header 1\n## Header 2";
        assert_eq!(extract_description(input), "");
    }
}
