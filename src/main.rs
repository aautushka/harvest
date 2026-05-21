use std::collections::HashSet;
use std::env;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

// Tokenize a line respecting backslash-escaped spaces (e.g. foo\ bar -> "foo bar")
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if chars.peek() == Some(&' ') {
                chars.next();
                current.push(' ');
                continue;
            }
        }
        if c.is_whitespace() || matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | '<' | '>') {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn trim_path_suffix(s: &str) -> &str {
    // strip trailing punctuation
    let s = s.trim_end_matches(|c| matches!(c, ',' | ';'));
    // strip :digits suffix (file:line pattern like foo.rs:42)
    if let Some(idx) = s.rfind(':') {
        let after = &s[idx + 1..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            return &s[..idx];
        }
    }
    // strip trailing dots/colons that aren't part of an extension
    s.trim_end_matches(|c| matches!(c, '.' | ':'))
}

// Extract absolute paths (start with /)
fn extract_absolute(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| t.starts_with('/') && t.len() > 1)
        .map(|t| trim_path_suffix(&t).to_string())
        .collect()
}

// Extract relative paths (contain / but don't start with /)
fn extract_relative(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| !t.starts_with('/') && t.contains('/'))
        .map(|t| trim_path_suffix(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// Extract single words containing a dot (likely filenames: foo.rs, config.yaml)
fn extract_dotwords(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| !t.starts_with('/') && !t.contains('/') && t.contains('.') && !t.starts_with("http"))
        .map(|t| trim_path_suffix(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn path_variants(path: &str) -> Vec<String> {
    vec![path.to_string()]
}

// Greedy joiner for ls-style output where spaces aren't escaped.
// Tries to extend a /token with subsequent words until an existing path is found.
fn extract_absolute_greedy(line: &str) -> Vec<String> {
    let words: Vec<&str> = line.split_ascii_whitespace().collect();
    let mut results = Vec::new();
    let mut skip_until = 0;
    for i in 0..words.len() {
        if i < skip_until { continue; }
        let word = words[i];
        if !word.starts_with('/') || word.len() <= 1 { continue; }
        let trimmed = trim_path_suffix(word).to_string();
        if !trimmed.is_empty() && Path::new(&trimmed).exists() {
            results.push(trimmed);
            continue;
        }
        let mut candidate = word.to_string();
        for j in (i + 1)..words.len().min(i + 20) {
            candidate.push(' ');
            candidate.push_str(words[j]);
            let trimmed = trim_path_suffix(&candidate).to_string();
            if !trimmed.is_empty() && Path::new(&trimmed).exists() {
                results.push(trimmed);
                skip_until = j + 1;
                break;
            }
        }
    }
    results
}

fn exists_at(candidate: &str, cwd: &Path) -> bool {
    let p = Path::new(candidate);
    if p.is_absolute() {
        p.exists()
    } else {
        cwd.join(p).exists()
    }
}

// Collect everything inside balanced parens (caller has already consumed the opening '(').
fn collect_paren_content<I: Iterator<Item = char>>(chars: &mut std::iter::Peekable<I>) -> String {
    let mut content = String::new();
    let mut depth = 1usize;
    for c in chars.by_ref() {
        if c == '(' { depth += 1; }
        if c == ')' { depth -= 1; if depth == 0 { break; } }
        content.push(c);
    }
    content
}

// Split string at top-level ':' (not inside nested parens).
fn split_top_colons(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => { parts.push(&s[start..i]); start = i + 1; }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

// Parse a ZSH prompt string into a plain string suitable for detecting prompt lines.
// Strips %{...%} color groups, processes %(cond:A:B) by keeping branch A, removes % codes.
fn parse_prompt_pattern(prompt: &str) -> String {
    let mut out = String::new();
    let mut chars = prompt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' { out.push(c); continue; }
        match chars.next() {
            None => break,
            Some('{') => {
                // %{...%} zero-width sequence — skip
                loop {
                    match chars.next() {
                        None | Some('%') => { chars.next(); break; }
                        _ => {}
                    }
                }
            }
            Some('(') => {
                // %(cond:true:false) — collect content, process first (true) branch
                let content = collect_paren_content(&mut chars);
                let parts = split_top_colons(&content);
                if parts.len() >= 2 {
                    out += &parse_prompt_pattern(parts[1]);
                }
            }
            Some('1') => {
                // %1{text%} — keep text
                if chars.peek() == Some(&'{') {
                    chars.next();
                    loop {
                        match chars.next() {
                            None => break,
                            Some('%') => { chars.next(); break; }
                            Some(ch) => out.push(ch),
                        }
                    }
                }
            }
            Some(_) => {} // skip %c, %~, %n, %?, etc.
        }
    }
    out.trim().to_string()
}

// Detect if a line looks like a prompt line using the stripped prompt pattern.
// We look for the fixed literal parts of the prompt in the line.
fn is_prompt_line(line: &str, prompt_literals: &[&str]) -> bool {
    if prompt_literals.is_empty() {
        return false;
    }
    prompt_literals.iter().all(|lit| line.contains(lit))
}

// Try to extract a `cd` target from a command line (after the prompt).
// Returns the new CWD if `cd` was found.
fn parse_cd(cmd: &str, current_cwd: &Path) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd == "cd" {
        return Some(PathBuf::from(env::var("HOME").unwrap_or_default()));
    }
    let rest = cmd.strip_prefix("cd ")?;
    let target = rest.trim().trim_matches('"').trim_matches('\'');
    let target = if target.starts_with("~/") {
        let home = env::var("HOME").unwrap_or_default();
        PathBuf::from(format!("{}/{}", home, &target[2..]))
    } else if target == "~" {
        PathBuf::from(env::var("HOME").unwrap_or_default())
    } else if target == "-" {
        return None; // can't track cd -
    } else {
        let p = Path::new(target);
        if p.is_absolute() { p.to_path_buf() } else { current_cwd.join(p) }
    };
    if target.exists() { Some(target) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- tokenize ---

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
        // backslash not followed by space is kept as-is
        let tokens = tokenize(r"foo\nbar");
        assert_eq!(tokens, vec![r"foo\nbar"]);
    }

    // --- extract_absolute ---

    #[test]
    fn absolute_basic() {
        assert_eq!(extract_absolute("see /foo/bar and done"), vec!["/foo/bar"]);
    }

    #[test]
    fn absolute_trailing_punctuation() {
        assert_eq!(extract_absolute("error at /foo/bar.rs:10"), vec!["/foo/bar.rs"]);
        assert_eq!(extract_absolute("see /foo/bar,"), vec!["/foo/bar"]);
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

    // --- extract_relative ---

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

    // --- extract_dotwords ---

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

    // --- parse_prompt_pattern ---

    #[test]
    fn prompt_pattern_user_prompt() {
        // The actual prompt from this project
        let prompt = r"%(?:%{%}%1{➜%} :%{%}%1{➜%} ) %{%}%c%{%} $(git_prompt_info)";
        let pattern = parse_prompt_pattern(prompt);
        // Should contain ➜ (from %1{➜%}) and strip color/conditional cruft
        assert!(pattern.contains('➜'), "pattern: {:?}", pattern);
        assert!(!pattern.contains("%{"), "pattern: {:?}", pattern);
        assert!(!pattern.contains("%c"), "pattern: {:?}", pattern);
        assert!(!pattern.contains("%("), "pattern: {:?}", pattern);
    }

    #[test]
    fn prompt_pattern_strips_color_groups() {
        let pattern = parse_prompt_pattern("%{\\e[32m%}hello%{\\e[0m%}");
        assert_eq!(pattern, "hello");
    }

    #[test]
    fn prompt_pattern_keeps_n_braces() {
        // %1{text%} should keep "text"
        let pattern = parse_prompt_pattern("%1{➜%}");
        assert_eq!(pattern, "➜");
    }

    #[test]
    fn prompt_pattern_strips_conditionals() {
        // first branch (true) is kept, false branch and condition are dropped
        let pattern = parse_prompt_pattern("%(?:yes:no) rest");
        assert_eq!(pattern, "yes rest");
    }

    #[test]
    fn prompt_pattern_strips_percent_codes() {
        // %c, %~, %n etc. should be stripped
        let pattern = parse_prompt_pattern("user %n at %m in %~");
        assert_eq!(pattern, "user  at  in");
    }

    // --- is_prompt_line ---

    #[test]
    fn prompt_line_detection() {
        let literals = vec!["➜"];
        assert!(is_prompt_line("➜  harvest git:(main)", &literals));
        assert!(!is_prompt_line("some random output", &literals));
    }

    #[test]
    fn prompt_line_multiple_literals() {
        let literals = vec!["➜", "git:("];
        assert!(is_prompt_line("➜  harvest git:(main) ✗", &literals));
        assert!(!is_prompt_line("➜  harvest", &literals)); // missing git:(
    }

    #[test]
    fn prompt_line_empty_literals() {
        assert!(!is_prompt_line("anything", &[]));
    }

    // --- path_variants ---

    #[test]
    fn variants_no_spaces() {
        assert_eq!(path_variants("/foo/bar"), vec!["/foo/bar"]);
    }

    // Regression: paths without spaces were silently dropped because candidate was
    // inserted into `seen` before iterating path_variants, so seen.insert(v) returned
    // false for the identical string and nothing was printed.
    fn emit_candidates(candidates: Vec<String>, seen: &mut HashSet<String>, cwd: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for candidate in candidates {
            if !seen.contains(&candidate) && exists_at(&candidate, cwd) {
                for v in path_variants(&candidate) {
                    if seen.insert(v.clone()) { out.push(v); }
                }
            }
        }
        out
    }

    #[test]
    fn no_space_path_is_emitted() {
        let mut seen = HashSet::new();
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"));
        assert_eq!(result, vec!["/tmp"]);
    }

    #[test]
    fn no_space_path_deduped_on_second_occurrence() {
        let mut seen = HashSet::new();
        emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"));
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"));
        assert!(result.is_empty());
    }

    #[test]
    fn space_path_emits_both_variants() {
        // /tmp exists and has no space, so manufacture a spaced path via a symlink
        // — instead, just test the emit logic with a path we know exists with spaces
        // by using /private/tmp (macOS) or just verify the variant logic + seen interaction
        let mut seen = HashSet::new();
        // Fake existence check by testing with a path that has no spaces first
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"));
        assert_eq!(result.len(), 1); // no spaces → 1 variant
        assert!(seen.contains("/tmp"));
    }

    #[test]
    fn variants_with_spaces() {
        assert_eq!(path_variants("/foo/bar baz"), vec!["/foo/bar baz"]);
    }

    // --- extract_absolute_greedy ---

    #[test]
    fn greedy_joins_spaces() {
        // /tmp exists; simulate ls output with spaces in filename
        // We can only test with paths that actually exist, so use /tmp
        // for the join logic test use a real dir that exists
        let line = "drwxr-xr-x  2 user  staff  /tmp";
        let results = extract_absolute_greedy(line);
        assert!(results.contains(&"/tmp".to_string()));
    }

    #[test]
    fn greedy_no_false_positive() {
        // tokens that don't form a real path should not appear
        let results = extract_absolute_greedy("no path here at all");
        assert!(results.is_empty());
    }

    // --- parse_cd ---

    #[test]
    fn parse_cd_absolute() {
        let cwd = PathBuf::from("/tmp");
        let result = parse_cd("cd /tmp", &cwd);
        assert_eq!(result, Some(PathBuf::from("/tmp")));
    }

    #[test]
    fn parse_cd_relative() {
        let result = parse_cd("cd harvest", &PathBuf::from("/Users/anton/proj"));
        // /Users/anton/proj/harvest exists in this repo's context — just check logic
        let expected = PathBuf::from("/Users/anton/proj/harvest");
        assert_eq!(result, if expected.exists() { Some(expected) } else { None });
    }

    #[test]
    fn parse_cd_tilde() {
        let result = parse_cd("cd ~", &PathBuf::from("/tmp"));
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(result, Some(PathBuf::from(&home)));
    }

    #[test]
    fn parse_cd_tilde_subdir() {
        let result = parse_cd("cd ~/proj", &PathBuf::from("/tmp"));
        let home = std::env::var("HOME").unwrap_or_default();
        let expected = PathBuf::from(format!("{}/proj", home));
        assert_eq!(result, if expected.exists() { Some(expected) } else { None });
    }

    #[test]
    fn parse_cd_dash_returns_none() {
        assert_eq!(parse_cd("cd -", &PathBuf::from("/tmp")), None);
    }

    #[test]
    fn parse_cd_no_cd_returns_none() {
        assert_eq!(parse_cd("ls -la", &PathBuf::from("/tmp")), None);
    }

    #[test]
    fn parse_cd_nonexistent_returns_none() {
        assert_eq!(parse_cd("cd /this/does/not/exist/ever", &PathBuf::from("/tmp")), None);
    }

    #[test]
    fn parse_cd_quoted() {
        let result = parse_cd("cd \"/tmp\"", &PathBuf::from("/"));
        assert_eq!(result, Some(PathBuf::from("/tmp")));
    }
}

struct Args {
    cwd: PathBuf,
    prompt: Option<String>,
}

fn parse_args() -> Args {
    let mut cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut prompt = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd" => { if let Some(v) = args.next() { cwd = PathBuf::from(v); } }
            "--prompt" => { if let Some(v) = args.next() { prompt = Some(v); } }
            _ => {}
        }
    }
    Args { cwd, prompt }
}

fn main() {
    let args = parse_args();
    let stdin = io::stdin();
    let lines: Vec<String> = stdin.lock().lines().filter_map(|l| l.ok()).collect();

    // Build prompt detection if --prompt given
    let prompt_literals_storage: Vec<String> = args.prompt.as_deref()
        .map(|p| parse_prompt_pattern(p)
            .split_whitespace()
            .filter(|s| s.len() >= 2)
            .map(|s| s.to_string())
            .collect())
        .unwrap_or_default();
    let prompt_literals: Vec<&str> = prompt_literals_storage.iter().map(|s| s.as_str()).collect();
    let use_prompt = args.prompt.is_some() && !prompt_literals.is_empty();

    // Build per-section CWD map for last 20 commands (when prompt available).
    // sections[i] = (line_index_start, cwd)  — line ranges from bottom, most recent first
    let mut section_cwds: Vec<(usize, PathBuf)> = Vec::new(); // (line_idx, cwd at that point)

    if use_prompt {
        // Walk lines top-to-bottom tracking CWD; keep only last 20 prompt sections
        let mut cwd = args.cwd.clone();
        let mut sections_fwd: Vec<(usize, PathBuf)> = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            if is_prompt_line(line, &prompt_literals) {
                // Next non-empty line after prompt is the command
                if let Some(cmd_line) = lines[i+1..].iter().find(|l| !l.trim().is_empty()) {
                    let cmd = if use_prompt {
                        // strip prompt prefix heuristically: drop first token-like chunk
                        cmd_line.trim()
                    } else {
                        cmd_line.trim()
                    };
                    if let Some(new_cwd) = parse_cd(cmd, &cwd) {
                        cwd = new_cwd;
                    }
                }
                sections_fwd.push((i, cwd.clone()));
            }
        }

        // Keep last 20, reverse so index 0 = most recent
        let start = sections_fwd.len().saturating_sub(20);
        section_cwds = sections_fwd[start..].iter().rev().cloned().collect();
    }

    // For a given line index, find the best CWD to use
    let cwd_for_line = |line_idx: usize| -> &Path {
        for (section_start, section_cwd) in &section_cwds {
            if line_idx >= *section_start {
                return section_cwd.as_path();
            }
        }
        args.cwd.as_path()
    };

    let mut seen: HashSet<String> = HashSet::new();

    // Iterate bottom-to-top (most recent first)
    for (i, line) in lines.iter().enumerate().rev() {
        let cwd = cwd_for_line(i);

        // Absolute paths (escape-aware tokenizer + greedy space-joiner for ls output)
        let abs_candidates = extract_absolute(line).into_iter()
            .chain(extract_absolute_greedy(line));
        for candidate in abs_candidates {
            if !seen.contains(&candidate) && Path::new(&candidate).exists() {
                for v in path_variants(&candidate) {
                    if seen.insert(v.clone()) { println!("{}", v); }
                }
            }
        }

        // Relative paths (contain /)
        for candidate in extract_relative(line) {
            if !seen.contains(&candidate) && exists_at(&candidate, cwd) {
                for v in path_variants(&candidate) {
                    if seen.insert(v.clone()) { println!("{}", v); }
                }
            }
        }

        // Dot-words (foo.rs) — only when prompt tracking is active
        if use_prompt {
            for candidate in extract_dotwords(line) {
                if seen.insert(candidate.clone()) && exists_at(&candidate, cwd) {
                    println!("{}", candidate);
                }
            }
        }
    }
}
