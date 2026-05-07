use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Match `[[target]]` where `target` may contain spaces, slashes, hyphens, or
/// other readable characters. We deliberately reject `]` and newlines in the
/// target. `[\[\[` and `\]\]` are escaped for clarity.
pub(crate) static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[([^\]\n]+?)\]\]").expect("link regex"));

/// Match `#tag` where the tag chars are alphanumeric, `-`, `_`, or `/`. We
/// require the `#` to be at the start of the slice or preceded by whitespace
/// or one of `(`, `[`, `{`, `>`, `,`. The "previous-char" check happens in
/// `parse_tags`, not in the regex.
static TAG_BODY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#([A-Za-z0-9][A-Za-z0-9_\-/]*)").expect("tag regex"));

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

/// Split a markdown file into its frontmatter prefix (including the closing
/// `---\n`) and the body. The frontmatter prefix is byte-identical to the
/// input prefix; concatenating the two halves reproduces the original.
///
/// If the file has no frontmatter, the prefix is empty and `body` is the
/// entire input.
pub fn split_frontmatter_and_body(content: &str) -> (&str, &str) {
    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        // Find closing fence "\n---" after the opening line.
        if let Some(pos) = content[3..].find("\n---") {
            // Closing fence ends at content[3+pos+4]; advance past trailing
            // newline if present so the body starts on its first content
            // line.
            let mut end = 3 + pos + 4;
            // Skip the newline after the closing `---`.
            if content.as_bytes().get(end) == Some(&b'\r') {
                end += 1;
            }
            if content.as_bytes().get(end) == Some(&b'\n') {
                end += 1;
            }
            if end <= content.len() {
                return (&content[..end], &content[end..]);
            }
        }
    }
    ("", content)
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

/// Strip inline-code spans (single-backtick) from a line so their contents
/// don't get scanned for links/tags. We do not handle nested backticks; this
/// is a heuristic that matches GitHub-flavored markdown well enough for
/// memory bodies.
fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            // consume until next backtick or end
            for c2 in chars.by_ref() {
                if c2 == '`' {
                    break;
                }
            }
            // replace removed span with a single space so adjacent words don't merge
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Iterate the body line-by-line, yielding only lines that are NOT inside a
/// fenced code block (``` ... ```). Frontmatter is already stripped via
/// `extract_body` upstream.
fn body_lines_outside_code(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in body.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push(strip_inline_code(line));
        }
    }
    out
}

/// Parse `[[wikilinks]]` from a memory's full content. Frontmatter, fenced
/// code blocks, and inline `code` spans are excluded. Returns the raw target
/// strings in order of appearance, deduplicated by exact match.
pub fn parse_links(content: &str) -> Vec<String> {
    let body = extract_body(content);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in body_lines_outside_code(&body) {
        for cap in LINK_RE.captures_iter(&line) {
            if let Some(m) = cap.get(1) {
                let target = m.as_str().trim().to_string();
                if !target.is_empty() && seen.insert(target.clone()) {
                    out.push(target);
                }
            }
        }
    }
    out
}

/// Parse `#tags` from a memory's full content. Frontmatter, fenced code,
/// inline code, and tags inside URL fragments are excluded. Returns deduped
/// tag strings (without the `#`).
pub fn parse_tags(content: &str) -> Vec<String> {
    let body = extract_body(content);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in body_lines_outside_code(&body) {
        // Find each `#word` and inspect the preceding char on the original line.
        for m in TAG_BODY_RE.find_iter(&line) {
            let start = m.start();
            // Preceding char check: the `#` must be at start-of-line OR
            // preceded by whitespace or one of a small set of opening
            // punctuation. This rejects URL fragments like `example.com#x`
            // and inline tokens like `foo#bar`.
            let prev_ok = if start == 0 {
                true
            } else {
                let prev = line[..start].chars().next_back().unwrap_or(' ');
                prev.is_whitespace()
                    || matches!(prev, '(' | '[' | '{' | '>' | ',')
            };
            if !prev_ok {
                continue;
            }
            // Capture group 1 is the tag without the `#`.
            let tag_with_hash = m.as_str();
            let tag = &tag_with_hash[1..]; // strip leading '#'
            if seen.insert(tag.to_string()) {
                out.push(tag.to_string());
            }
        }
    }
    out
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

    // ----- DW-2.1: parse_links -----

    #[test]
    fn test_dw_2_1_parse_links_basic() {
        let input = "Body has [[Alpha]] and [[Beta]] references.";
        assert_eq!(parse_links(input), vec!["Alpha", "Beta"]);
    }

    #[test]
    fn test_dw_2_1_parse_links_with_slash_and_spaces() {
        let input = "See [[notes/My File]] and [[refs/another item]]";
        assert_eq!(
            parse_links(input),
            vec!["notes/My File", "refs/another item"]
        );
    }

    #[test]
    fn test_dw_2_1_parse_links_dedupes() {
        let input = "[[A]] then [[A]] again and [[B]]";
        assert_eq!(parse_links(input), vec!["A", "B"]);
    }

    #[test]
    fn test_dw_2_1_parse_links_in_fenced_code_ignored() {
        let input = "Real [[Real]]\n```\nfake [[Fake]]\n```\nMore [[Other]]";
        let got = parse_links(input);
        assert!(got.contains(&"Real".to_string()));
        assert!(got.contains(&"Other".to_string()));
        assert!(!got.contains(&"Fake".to_string()), "got {got:?}");
    }

    #[test]
    fn test_dw_2_1_parse_links_in_inline_code_ignored() {
        let input = "Real [[Real]] but inline `[[Fake]]` should be skipped.";
        let got = parse_links(input);
        assert_eq!(got, vec!["Real"]);
    }

    #[test]
    fn test_dw_2_1_parse_links_in_frontmatter_ignored() {
        let input = "---\nname: test\nseed: [[NotALink]]\n---\nReal [[Body]]";
        let got = parse_links(input);
        assert_eq!(got, vec!["Body"]);
    }

    #[test]
    fn test_dw_2_1_parse_links_empty_target_skipped() {
        let input = "Empty [[]] should not produce a link, but [[X]] should.";
        let got = parse_links(input);
        assert_eq!(got, vec!["X"]);
    }

    // ----- DW-2.2: parse_tags -----

    #[test]
    fn test_dw_2_2_parse_tags_basic() {
        let input = "I like #rust and #systems-programming.";
        let got = parse_tags(input);
        assert_eq!(got, vec!["rust", "systems-programming"]);
    }

    #[test]
    fn test_dw_2_2_parse_tags_allowed_chars() {
        let input = "tags: #foo_bar #ns/sub #a-b-c #123abc";
        let got = parse_tags(input);
        assert_eq!(got, vec!["foo_bar", "ns/sub", "a-b-c", "123abc"]);
    }

    #[test]
    fn test_dw_2_2_parse_tags_in_code_ignored() {
        let input = "Real #real\n```\nfake #fake\n```\nInline `#also-fake` here.";
        let got = parse_tags(input);
        assert!(got.contains(&"real".to_string()));
        assert!(!got.contains(&"fake".to_string()));
        assert!(!got.contains(&"also-fake".to_string()));
    }

    #[test]
    fn test_dw_2_2_parse_tags_in_url_ignored() {
        let input = "See https://example.com#section for details, and #real-tag.";
        let got = parse_tags(input);
        assert_eq!(got, vec!["real-tag"]);
    }

    #[test]
    fn test_dw_2_2_parse_tags_in_frontmatter_ignored() {
        let input = "---\nname: test\nseed: #notatag\n---\nBody #realtag";
        let got = parse_tags(input);
        assert_eq!(got, vec!["realtag"]);
    }

    #[test]
    fn test_dw_2_2_parse_tags_dedupes() {
        let input = "#a #b #a #c";
        assert_eq!(parse_tags(input), vec!["a", "b", "c"]);
    }
}
