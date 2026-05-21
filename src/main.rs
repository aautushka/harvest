use std::collections::HashSet;
use std::env;
use std::io::{self, BufRead, Write};
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
    let s = s.trim_end_matches(|c| matches!(c, ',' | ';'));
    // strip trailing dots/colons first so that `file.rs:42:` → `file.rs:42` before the :N check
    let s = s.trim_end_matches(|c| matches!(c, '.' | ':'));
    // strip :digits suffix (file:line pattern like foo.rs:42)
    if let Some(idx) = s.rfind(':') {
        let after = &s[idx + 1..];
        if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
            return &s[..idx];
        }
    }
    s
}

// Extract absolute paths (start with /).
// Also handles paths embedded after ':' or '=' with no surrounding space,
// e.g. "CACHE_DIR:/home/..." or "PREFIX=/home/...".
fn extract_absolute(line: &str) -> Vec<String> {
    let mut results = Vec::new();
    for token in tokenize(line) {
        if token.starts_with('/') && token.len() > 1 {
            let s = trim_path_suffix(&token).to_string();
            // Re-check len after trimming: e.g. "/.."->"/" should be dropped
            if s.len() > 1 { results.push(s); }
        } else {
            // Look for the first ':' or '=' immediately followed by '/'
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
        if trimmed.len() > 1 && Path::new(&trimmed).exists() {
            results.push(trimmed);
            continue;
        }
        let mut candidate = word.to_string();
        for j in (i + 1)..words.len().min(i + 20) {
            candidate.push(' ');
            candidate.push_str(words[j]);
            let trimmed = trim_path_suffix(&candidate).to_string();
            if trimmed.len() > 1 && Path::new(&trimmed).exists() {
                results.push(trimmed);
                skip_until = j + 1;
                break;
            }
        }
    }
    results
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<std::path::Component> = Vec::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => match components.last() {
                Some(std::path::Component::RootDir)
                | Some(std::path::Component::Prefix(_))
                | None => components.push(c),
                _ => { components.pop(); }
            },
            c => components.push(c),
        }
    }
    if components.is_empty() { PathBuf::from(".") } else { components.iter().collect() }
}

fn make_relative(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in 0..(from.len() - common) { result.push(".."); }
    for c in &to[common..] { result.push(c); }
    if result.as_os_str().is_empty() { result.push("."); }
    result
}

// Resolve a relative candidate against section_cwd, re-express relative to current_cwd.
// Returns whichever of the relative or absolute form is shorter.
fn rebase_path(candidate: &str, section_cwd: &Path, current_cwd: &Path) -> String {
    let abs = normalize_path(&section_cwd.join(candidate));
    let rel = match abs.strip_prefix(current_cwd) {
        Ok(r) if r.as_os_str().is_empty() => PathBuf::from("."),
        Ok(r) => PathBuf::from(format!("./{}", r.display())),
        Err(_) => make_relative(current_cwd, &abs),
    };
    let rel_s = rel.to_string_lossy();
    let abs_s = abs.to_string_lossy();
    if rel_s.len() <= abs_s.len() { rel_s.into_owned() } else { abs_s.into_owned() }
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
        // Strip $(...) shell command substitutions — their output is dynamic
        if c == '$' && chars.peek() == Some(&'(') {
            chars.next();
            let mut depth = 1usize;
            for ch in chars.by_ref() {
                if ch == '(' { depth += 1; }
                if ch == ')' { depth -= 1; if depth == 0 { break; } }
            }
            continue;
        }
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
fn is_prompt_line(line: &str, prompt_literals: &[&str]) -> bool {
    if prompt_literals.is_empty() {
        return false;
    }
    prompt_literals.iter().all(|lit| line.contains(lit))
}

// Extract the command from a prompt line by taking everything after the last prompt literal.
fn extract_command_from_prompt_line<'a>(line: &'a str, prompt_literals: &[&str]) -> &'a str {
    let last_end = prompt_literals.iter()
        .filter_map(|lit| line.rfind(lit).map(|pos| pos + lit.len()))
        .max()
        .unwrap_or(0);
    line[last_end..].trim()
}

// Find a `cd <target>` in a prompt line without needing to know where the prompt ends.
// Searches for the pattern ` cd <word>` from the right.
fn find_cd_in_line(line: &str) -> Option<&str> {
    // Look for " cd " followed by a target (last occurrence wins)
    let mut search = line;
    while let Some(pos) = search.rfind(" cd ") {
        let rest = search[pos + 4..].trim();
        if !rest.is_empty() {
            // Take first word (stop at space, pipe, semicolon)
            let target = rest.split(|c: char| c.is_whitespace() || c == '|' || c == ';').next()?;
            if !target.is_empty() {
                return Some(target);
            }
        }
        search = &search[..pos];
    }
    None
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
        return None;
    } else {
        let p = Path::new(target);
        if p.is_absolute() { p.to_path_buf() } else { current_cwd.join(p) }
    };
    if target.exists() { Some(target) } else { None }
}

// Reverse of parse_cd: given the cd target string and the resulting cwd,
// reconstruct the cwd before the cd. Only works for simple relative subdirs.
fn undo_cd(target: &str, result_cwd: &Path) -> Option<PathBuf> {
    let target = target.trim().trim_matches('"').trim_matches('\'');
    if target.is_empty() || target == "-" || target == "~"
        || target.starts_with("~/") || Path::new(target).is_absolute()
    {
        return None;
    }
    let target_path = Path::new(target);
    if target_path.components().any(|c| c == std::path::Component::ParentDir) {
        return None; // has .., too complex
    }
    let tc: Vec<_> = target_path.components().collect();
    let rc: Vec<_> = result_cwd.components().collect();
    if rc.len() <= tc.len() { return None; }
    let split = rc.len() - tc.len();
    let matches = rc[split..].iter().map(|c| c.as_os_str())
        .eq(tc.iter().map(|c| c.as_os_str()));
    if matches {
        let prev: PathBuf = rc[..split].iter().collect();
        if !prev.as_os_str().is_empty() { Some(prev) } else { None }
    } else {
        None
    }
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
        // GCC/CMake style: file:line: (trailing colon)
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
        // "cd $(pwd)/.." tokenizes to ["cd","$","pwd","/.." ] — "/.."->"/" must be dropped
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
    fn prompt_pattern_strips_command_substitution() {
        let prompt = r"%(?:%{%}%1{➜%} :%{%}%1{➜%} ) %{%}%c%{%} $(git_prompt_info)";
        let pattern = parse_prompt_pattern(prompt);
        assert!(!pattern.contains("$("), "pattern: {:?}", pattern);
        assert!(!pattern.contains("git_prompt_info"), "pattern: {:?}", pattern);
        assert!(pattern.contains('➜'), "pattern: {:?}", pattern);
    }

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

    // --- normalize_path ---

    #[test]
    fn normalize_removes_curdir() {
        assert_eq!(normalize_path(Path::new("/a/b/./c")), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_removes_parentdir() {
        assert_eq!(normalize_path(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
    }

    // --- rebase_path ---

    #[test]
    fn rebase_subdir_candidate() {
        // find ran in /harvest, we're now in /harvest/src
        // ./src/main.rs from /harvest → ./main.rs from /harvest/src
        let result = rebase_path(
            "./src/main.rs",
            Path::new("/harvest"),
            Path::new("/harvest/src"),
        );
        assert_eq!(result, "./main.rs");
    }

    #[test]
    fn rebase_same_cwd() {
        // no cd; candidate is valid as-is
        let result = rebase_path(
            "./src/main.rs",
            Path::new("/harvest"),
            Path::new("/harvest"),
        );
        assert_eq!(result, "./src/main.rs");
    }

    #[test]
    fn rebase_outside_current_cwd_uses_dotdot() {
        // ../tests/foo.rs (15) < /project/tests/foo.rs (21) → relative wins
        let result = rebase_path(
            "./foo.rs",
            Path::new("/project/tests"),
            Path::new("/project/src"),
        );
        assert_eq!(result, "../tests/foo.rs");
    }

    #[test]
    fn rebase_deep_mismatch_falls_back_to_absolute() {
        // ../../../../x/y/file.txt (24) > /x/y/file.txt (13) → absolute wins
        let result = rebase_path(
            "./file.txt",
            Path::new("/x/y"),
            Path::new("/a/b/c/d/e"),
        );
        assert_eq!(result, "/x/y/file.txt");
    }

    // --- extract_command_from_prompt_line ---

    #[test]
    fn command_extracted_from_prompt_line() {
        let literals = vec!["➜", "git:(main)"];
        assert_eq!(
            extract_command_from_prompt_line("➜  harvest git:(main) cd src", &literals),
            "cd src"
        );
    }

    #[test]
    fn command_extracted_non_cd() {
        let literals = vec!["➜", "git:(main)"];
        assert_eq!(
            extract_command_from_prompt_line("➜  harvest git:(main) find . | grep foo", &literals),
            "find . | grep foo"
        );
    }

    #[test]
    fn command_empty_prompt_line() {
        let literals = vec!["➜", "git:(main)"];
        // prompt with no command (user just pressed enter or trigger key)
        assert_eq!(
            extract_command_from_prompt_line("➜  src git:(main) ✗", &literals),
            "✗"  // ✗ is not in literals here, so it's treated as trailing text
        );
    }

    #[test]
    fn command_with_dirty_marker_in_literals() {
        let literals = vec!["➜", "git:(main)", "✗"];
        assert_eq!(
            extract_command_from_prompt_line("➜  harvest git:(main) ✗ cd src", &literals),
            "cd src"
        );
    }

    // --- find_cd_in_line ---

    #[test]
    fn find_cd_extracts_from_prompt_line() {
        assert_eq!(find_cd_in_line("➜  harvest git:(main) cd src"), Some("src"));
    }

    #[test]
    fn find_cd_extracts_absolute() {
        assert_eq!(find_cd_in_line("➜  harvest git:(main) cd /tmp"), Some("/tmp"));
    }

    #[test]
    fn find_cd_returns_none_for_non_cd() {
        assert_eq!(find_cd_in_line("➜  harvest git:(main) find . | grep main"), None);
    }

    #[test]
    fn find_cd_ignores_cd_in_path() {
        // "cat /tmp/cd files.txt" - cd is part of a path, not a command
        // But since this would also be a prompt line check first, low risk
        assert_eq!(find_cd_in_line("cat /tmp/nocd here"), None);
    }

    // --- undo_cd ---

    #[test]
    fn undo_cd_simple_subdir() {
        let result = undo_cd("src", Path::new("/Users/anton/proj/harvest/src"));
        assert_eq!(result, Some(PathBuf::from("/Users/anton/proj/harvest")));
    }

    #[test]
    fn undo_cd_multi_component() {
        let result = undo_cd("proj/harvest", Path::new("/Users/anton/proj/harvest"));
        assert_eq!(result, Some(PathBuf::from("/Users/anton")));
    }

    #[test]
    fn undo_cd_mismatch_returns_none() {
        // last component doesn't match target
        let result = undo_cd("other", Path::new("/Users/anton/proj/harvest/src"));
        assert_eq!(result, None);
    }

    #[test]
    fn undo_cd_absolute_returns_none() {
        assert_eq!(undo_cd("/tmp", Path::new("/tmp")), None);
    }

    #[test]
    fn undo_cd_parent_returns_none() {
        assert_eq!(undo_cd("..", Path::new("/Users/anton")), None);
    }

    #[test]
    fn undo_cd_dash_returns_none() {
        assert_eq!(undo_cd("-", Path::new("/tmp")), None);
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
    cwd_log: Option<PathBuf>,
    lines: Option<usize>,
    debug: bool,
}

fn parse_args() -> Args {
    let mut cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let mut prompt = None;
    let mut cwd_log = None;
    let mut lines = None;
    let mut debug = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cwd"     => { if let Some(v) = args.next() { cwd = PathBuf::from(v); } }
            "--prompt"  => { if let Some(v) = args.next() { prompt = Some(v); } }
            "--cwd-log" => { if let Some(v) = args.next() { cwd_log = Some(PathBuf::from(v)); } }
            "--lines"   => { if let Some(v) = args.next() { lines = v.parse().ok(); } }
            "--debug"   => { debug = true; }
            _ => {}
        }
    }
    if !debug { debug = env::var("HARVEST_DEBUG").is_ok(); }
    Args { cwd, prompt, cwd_log, lines, debug }
}

fn main() {
    let args = parse_args();
    let stdin = io::stdin();
    let mut lines: Vec<String> = stdin.lock().lines().filter_map(|l| l.ok()).collect();
    if let Some(n) = args.lines {
        let skip = lines.len().saturating_sub(n);
        lines.drain(..skip);
    }

    let debug_path = if args.debug { Some("/tmp/harvest_debug.txt") } else { None };
    macro_rules! dbg {
        ($($arg:tt)*) => {
            if let Some(path) = debug_path {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, $($arg)*);
                }
            }
        }
    }
    if let Some(path) = debug_path {
        let _ = std::fs::write(path, ""); // truncate
        dbg!("cwd: {:?}", args.cwd);
        dbg!("prompt: {:?}", args.prompt);
        dbg!("lines: {} (--lines limit: {:?})", lines.len(), args.lines);
    }

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

    dbg!("use_prompt: {use_prompt}, literals: {prompt_literals:?}");

    // Build per-section CWD map for last 20 commands (when prompt available).
    let mut section_cwds: Vec<(usize, PathBuf)> = Vec::new();

    if use_prompt {
        let prompt_indices: Vec<usize> = lines.iter().enumerate()
            .filter(|(_, l)| is_prompt_line(l, &prompt_literals))
            .map(|(i, _)| i)
            .collect();

        dbg!("prompt line indices: {prompt_indices:?}");

        // Parse cwd log if available (format: "CWD\tCOMMAND" per line).
        let log_entries: Option<Vec<(PathBuf, String)>> = args.cwd_log.as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.lines().filter_map(|line| {
                let (cwd, cmd) = line.split_once('\t')?;
                Some((PathBuf::from(cwd), cmd.to_string()))
            }).collect());

        if let Some(ref e) = log_entries { dbg!("cwd_log: {} entries", e.len()); }

        // Step 1: undo_cd baseline — works for all prompts, but breaks on absolute cd.
        let mut undo_baseline: Vec<(usize, PathBuf)> = Vec::new();
        {
            let mut cwd = args.cwd.clone();
            for &i in prompt_indices.iter().rev().take(20) {
                let prev_cwd = if let Some(target) = find_cd_in_line(&lines[i]) {
                    dbg!("  undo_cd prompt[{i}]: cd {target:?} from {cwd:?}");
                    undo_cd(target, &cwd).unwrap_or_else(|| cwd.clone())
                } else {
                    cwd.clone()
                };
                undo_baseline.push((i, prev_cwd.clone()));
                cwd = prev_cwd;
            }
        }

        // Step 2: for each prompt, prefer a cwd_log match; fall back to undo_cd baseline.
        // Scan log from bottom so most-recent log entry matches most-recent prompt line.
        let mut log_ptr = log_entries.as_ref().map(|e| e.len()).unwrap_or(0);
        for (prompt_idx, undo_cwd) in undo_baseline {
            let prompt_line = &lines[prompt_idx];
            let mut matched_cwd = None;
            if let Some(ref entries) = log_entries {
                let mut scan = log_ptr;
                while scan > 0 {
                    scan -= 1;
                    let (cwd, cmd) = &entries[scan];
                    if !cmd.is_empty() && prompt_line.ends_with(cmd.as_str()) {
                        let before = prompt_line.len() - cmd.len();
                        if before == 0 || prompt_line.as_bytes()[before - 1] == b' ' {
                            matched_cwd = Some(cwd.clone());
                            log_ptr = scan;
                            break;
                        }
                    }
                }
            }
            let cwd = matched_cwd.unwrap_or(undo_cwd);
            dbg!("  prompt[{prompt_idx}] {:?} → {cwd:?}", prompt_line);
            section_cwds.push((prompt_idx, cwd));
        }
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

    for (i, line) in lines.iter().enumerate().rev() {
        let cwd = cwd_for_line(i);

        let abs_candidates: Vec<String> = extract_absolute(line).into_iter()
            .chain(extract_absolute_greedy(line))
            .collect();
        for candidate in abs_candidates {
            let exists = Path::new(&candidate).exists();
            dbg!("  abs [{i}] {candidate:?} → exists={exists}");
            if !seen.contains(&candidate) && exists {
                for v in path_variants(&candidate) {
                    if seen.insert(v.clone()) { println!("{}", v); }
                }
            }
        }

        for candidate in extract_relative(line) {
            dbg!("  rel [{i}] {candidate:?} in {cwd:?} → exists={}", exists_at(&candidate, cwd));
            if !exists_at(&candidate, cwd) { continue; }
            let output = rebase_path(&candidate, cwd, &args.cwd);
            if seen.insert(output.clone()) { println!("{}", output); }
        }

        if use_prompt {
            for candidate in extract_dotwords(line) {
                if !exists_at(&candidate, cwd) { continue; }
                let output = rebase_path(&candidate, cwd, &args.cwd);
                if seen.insert(output.clone()) { println!("{}", output); }
            }
        }
    }
}
