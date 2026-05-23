use std::path::Path;
use crate::fs::Filesystem;

pub(crate) fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&' ') {
            chars.next();
            current.push(' ');
            continue;
        }
        if c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>') {
            if !current.is_empty() { tokens.push(std::mem::take(&mut current)); }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() { tokens.push(current); }
    tokens
}

pub(crate) fn trim_path_suffix(s: &str) -> &str {
    let s = s.trim_end_matches(|c| matches!(c, ',' | ';'));
    let s = s.trim_end_matches(|c| matches!(c, '.' | ':'));
    if let Some(idx) = s.rfind(':') {
        let after = &s[idx + 1..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            return &s[..idx];
        }
    }
    s
}

// Extract absolute paths (start with /).
// Also handles paths embedded after ':' or '=' (e.g. "PREFIX=/home/...", "error:/path").
pub(crate) fn extract_absolute(line: &str) -> Vec<String> {
    let mut results = Vec::new();
    for token in tokenize(line) {
        if token.starts_with('/') && token.len() > 1 {
            let s = trim_path_suffix(&token).to_string();
            if s.len() > 1 { results.push(s); }
        } else {
            let bytes = token.as_bytes();
            for i in 0..bytes.len().saturating_sub(1) {
                if (bytes[i] == b':' || bytes[i] == b'=') && bytes[i + 1] == b'/' {
                    let after = &token[i + 1..];
                    if after.len() > 1 {
                        let s = trim_path_suffix(after).to_string();
                        if s.len() > 1 { results.push(s); }
                    }
                    break;
                }
            }
        }
    }
    results
}

// Extract relative paths (contain / but don't start with /)
pub(crate) fn extract_relative(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| !t.starts_with('/') && t.contains('/'))
        .map(|t| trim_path_suffix(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// Extract single words containing a dot (likely filenames: foo.rs, config.yaml)
pub(crate) fn extract_dotwords(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| {
            !t.starts_with('/') && !t.contains('/') && t.contains('.') && !t.starts_with("http")
        })
        .map(|t| trim_path_suffix(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// Greedy joiner for ls-style output where spaces aren't escaped.
pub(crate) fn extract_absolute_greedy(line: &str, fs: &dyn Filesystem) -> Vec<String> {
    let words: Vec<&str> = line.split_ascii_whitespace().collect();
    let mut results = Vec::new();
    let mut skip_until = 0;
    for i in 0..words.len() {
        if i < skip_until { continue; }
        let word = words[i];
        if !word.starts_with('/') || word.len() <= 1 { continue; }
        let trimmed = trim_path_suffix(word).to_string();
        if trimmed.len() > 1 && fs.exists(Path::new(&trimmed)) {
            results.push(trimmed);
            continue;
        }
        let mut candidate = word.to_string();
        for j in (i + 1)..words.len().min(i + 20) {
            candidate.push(' ');
            candidate.push_str(words[j]);
            let trimmed = trim_path_suffix(&candidate).to_string();
            if trimmed.len() > 1 && fs.exists(Path::new(&trimmed)) {
                results.push(trimmed);
                skip_until = j + 1;
                break;
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::RealFs;

    #[test]
    fn tokenize_basic() {
        assert_eq!(tokenize("foo bar baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn tokenize_escaped_spaces() {
        assert_eq!(
            tokenize(r"la /Users/anton/Pictures/Screen\ Recording\ 2026.mov"),
            vec!["la", "/Users/anton/Pictures/Screen Recording 2026.mov"]
        );
    }

    #[test]
    fn tokenize_strips_delimiters() {
        assert_eq!(tokenize("foo(bar)"), vec!["foo", "bar"]);
        assert_eq!(tokenize("\"foo\" 'bar'"), vec!["foo", "bar"]);
    }

    #[test]
    fn tokenize_backslash_non_space() {
        assert_eq!(tokenize(r"foo\nbar"), vec![r"foo\nbar"]);
    }

    #[test]
    fn absolute_basic() {
        assert_eq!(extract_absolute("see /foo/bar and done"), vec!["/foo/bar"]);
    }

    #[test]
    fn absolute_trailing_punctuation() {
        assert_eq!(extract_absolute("error at /foo/bar.rs:10"), vec!["/foo/bar.rs"]);
        assert_eq!(extract_absolute("see /foo/bar,"), vec!["/foo/bar"]);
        assert_eq!(extract_absolute("/foo/bar.cmake:42:"), vec!["/foo/bar.cmake"]);
        assert_eq!(extract_absolute("/foo/bar.rs:10:"), vec!["/foo/bar.rs"]);
    }

    #[test]
    fn absolute_escaped_spaces() {
        assert_eq!(
            extract_absolute(r"la /Users/anton/Pictures/Screen\ Recording.mov"),
            vec!["/Users/anton/Pictures/Screen Recording.mov"]
        );
    }

    #[test]
    fn absolute_ignores_bare_slash() {
        assert!(extract_absolute("cd /").is_empty());
    }

    #[test]
    fn absolute_ignores_slash_from_dotdot() {
        assert!(extract_absolute("cd $(pwd)/..").iter().all(|p| p != "/"));
    }

    #[test]
    fn absolute_embedded_after_colon() {
        assert_eq!(extract_absolute("CACHE_DIR:/foo/bar"), vec!["/foo/bar"]);
        assert_eq!(extract_absolute("error:/foo/bar.rs"), vec!["/foo/bar.rs"]);
    }

    #[test]
    fn absolute_embedded_after_equals() {
        assert_eq!(extract_absolute("PREFIX=/foo/bar"), vec!["/foo/bar"]);
        assert_eq!(extract_absolute("CMAKE_INSTALL_PREFIX=/home/user/proj"), vec!["/home/user/proj"]);
    }

    #[test]
    fn relative_basic() {
        assert_eq!(extract_relative("see src/main.rs here"), vec!["src/main.rs"]);
    }

    #[test]
    fn relative_dotslash() {
        assert_eq!(extract_relative("run ./foo/bar"), vec!["./foo/bar"]);
    }

    #[test]
    fn relative_parent() {
        assert_eq!(extract_relative("edit ../config/x.yaml"), vec!["../config/x.yaml"]);
    }

    #[test]
    fn relative_ignores_absolute() {
        assert!(extract_relative("/foo/bar").is_empty());
    }

    #[test]
    fn relative_trailing_punctuation() {
        assert_eq!(extract_relative("see src/main.rs,"), vec!["src/main.rs"]);
    }

    #[test]
    fn dotwords_basic() {
        assert_eq!(extract_dotwords("edit main.rs"), vec!["main.rs"]);
        assert_eq!(extract_dotwords("config.yaml found"), vec!["config.yaml"]);
    }

    #[test]
    fn dotwords_ignores_absolute() {
        assert!(extract_dotwords("/foo/bar.rs").is_empty());
    }

    #[test]
    fn dotwords_ignores_relative_paths() {
        assert!(extract_dotwords("src/main.rs").is_empty());
    }

    #[test]
    fn dotwords_ignores_urls() {
        assert!(extract_dotwords("https://example.com").is_empty());
        assert!(extract_dotwords("http://foo.bar").is_empty());
    }

    #[test]
    fn dotwords_trailing_punctuation() {
        assert_eq!(extract_dotwords("see main.rs."), vec!["main.rs"]);
    }

    #[test]
    fn greedy_joins_spaces() {
        let results = extract_absolute_greedy("drwxr-xr-x  2 user  staff  /tmp", &RealFs);
        assert!(results.contains(&"/tmp".to_string()));
    }

    #[test]
    fn greedy_no_false_positive() {
        let results = extract_absolute_greedy("no path here at all", &RealFs);
        assert!(results.is_empty());
    }
}
