use std::collections::HashSet;
use std::env;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

// ── Filesystem abstraction ────────────────────────────────────────────────────

trait Filesystem {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

struct RealFs;

impl Filesystem for RealFs {
    fn exists(&self, path: &Path) -> bool { path.exists() }
    fn is_dir(&self, path: &Path) -> bool { path.is_dir() }
}

/// In-memory filesystem for tests.
/// Declare files; all parent directories are inferred automatically.
/// Paths are normalised on lookup, so `..` and `.` components work correctly.
struct MemFs {
    files: HashSet<PathBuf>,
    dirs:  HashSet<PathBuf>,
}

impl MemFs {
    fn new(file_paths: &[&str]) -> Self {
        let mut files = HashSet::new();
        let mut dirs  = HashSet::new();
        dirs.insert(PathBuf::from("/"));
        for s in file_paths {
            let p = PathBuf::from(s);
            files.insert(p.clone());
            let mut cur = p;
            loop {
                match cur.parent() {
                    Some(parent) if parent != cur => {
                        let parent = parent.to_path_buf();
                        if !dirs.insert(parent.clone()) { break; } // already added
                        cur = parent;
                    }
                    _ => break,
                }
            }
        }
        Self { files, dirs }
    }
}

impl Filesystem for MemFs {
    fn exists(&self, path: &Path) -> bool {
        let p = normalize_path(path);
        self.files.contains(&p) || self.dirs.contains(&p)
    }
    fn is_dir(&self, path: &Path) -> bool {
        let p = normalize_path(path);
        self.dirs.contains(&p)
    }
}

// ── Text utilities ────────────────────────────────────────────────────────────

// Tokenize a line respecting backslash-escaped spaces (e.g. foo\ bar -> "foo bar")
fn tokenize(line: &str) -> Vec<String> {
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

fn trim_path_suffix(s: &str) -> &str {
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
fn extract_absolute(line: &str) -> Vec<String> {
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
fn extract_absolute_greedy(line: &str, fs: &dyn Filesystem) -> Vec<String> {
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

// ── Path math ────────────────────────────────────────────────────────────────

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

fn exists_at(candidate: &str, cwd: &Path, fs: &dyn Filesystem) -> bool {
    let p = if Path::new(candidate).is_absolute() {
        normalize_path(Path::new(candidate))
    } else {
        normalize_path(&cwd.join(candidate))
    };
    fs.exists(&p)
}

// ── Prompt parsing ────────────────────────────────────────────────────────────

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

fn parse_prompt_pattern(prompt: &str) -> String {
    let mut out = String::new();
    let mut chars = prompt.chars().peekable();
    while let Some(c) = chars.next() {
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
                loop {
                    match chars.next() {
                        None | Some('%') => { chars.next(); break; }
                        _ => {}
                    }
                }
            }
            Some('(') => {
                let content = collect_paren_content(&mut chars);
                let parts = split_top_colons(&content);
                if parts.len() >= 2 { out += &parse_prompt_pattern(parts[1]); }
            }
            Some('1') => {
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
            Some(_) => {}
        }
    }
    out.trim().to_string()
}

fn is_prompt_line(line: &str, prompt_literals: &[&str]) -> bool {
    !prompt_literals.is_empty() && prompt_literals.iter().all(|lit| line.contains(lit))
}

fn extract_command_from_prompt_line<'a>(line: &'a str, prompt_literals: &[&str]) -> &'a str {
    let last_end = prompt_literals.iter()
        .filter_map(|lit| line.rfind(lit).map(|pos| pos + lit.len()))
        .max()
        .unwrap_or(0);
    line[last_end..].trim()
}

fn find_cd_in_line(line: &str) -> Option<&str> {
    let mut search = line;
    while let Some(pos) = search.rfind(" cd ") {
        let rest = search[pos + 4..].trim();
        if !rest.is_empty() {
            let target = rest.split(|c: char| c.is_whitespace() || c == '|' || c == ';').next()?;
            if !target.is_empty() { return Some(target); }
        }
        search = &search[..pos];
    }
    None
}

// ── CD tracking ───────────────────────────────────────────────────────────────

fn parse_cd(cmd: &str, current_cwd: &Path, fs: &dyn Filesystem) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if cmd == "cd" {
        return Some(PathBuf::from(env::var("HOME").unwrap_or_default()));
    }
    let rest = cmd.strip_prefix("cd ")?;
    let target = rest.trim().trim_matches('"').trim_matches('\'');
    let path = if target.starts_with("~/") {
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
    let path = normalize_path(&path);
    if fs.exists(&path) { Some(path) } else { None }
}

fn undo_cd(target: &str, result_cwd: &Path) -> Option<PathBuf> {
    let target = target.trim().trim_matches('"').trim_matches('\'');
    if target.is_empty() || target == "-" || target == "~"
        || target.starts_with("~/") || Path::new(target).is_absolute()
    {
        return None;
    }
    let target_path = Path::new(target);
    if target_path.components().any(|c| c == std::path::Component::ParentDir) {
        return None;
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

// ── CWD candidate extraction ──────────────────────────────────────────────────

fn resolve_env_token(token: &str) -> Option<String> {
    let name = token.strip_prefix('$')?;
    let name = if let Some(inner) = name.strip_prefix('{') {
        inner.trim_end_matches('}')
    } else {
        name
    };
    if name.is_empty() { return None; }
    env::var(name).ok()
}

/// Extract directory candidates from a command string.
/// Handles absolute paths, relative paths, `~`, `$VAR`, `$VAR/rest`, `KEY=/path`, `KEY=$VAR`.
fn extract_dir_candidates(cmd: &str, base_cwd: &Path, fs: &dyn Filesystem) -> Vec<PathBuf> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<PathBuf> = Vec::new();

    let mut push = |raw: PathBuf| {
        let p = normalize_path(&raw);
        if fs.is_dir(&p) && seen.insert(p.clone()) {
            out.push(p);
        }
    };

    for token in tokenize(cmd) {
        if token.starts_with('$') {
            let (var_tok, suffix) = match token.find('/') {
                Some(i) => (&token[..i], &token[i..]),
                None    => (token.as_str(), ""),
            };
            if let Some(val) = resolve_env_token(var_tok) {
                let full = format!("{val}{suffix}");
                let p = PathBuf::from(full);
                push(if p.is_absolute() { p } else { base_cwd.join(p) });
            }
        } else if token == "~" {
            if let Ok(home) = env::var("HOME") { push(PathBuf::from(home)); }
        } else if let Some(rest) = token.strip_prefix("~/") {
            if let Ok(home) = env::var("HOME") { push(PathBuf::from(format!("{home}/{rest}"))); }
        } else if token.starts_with('/') {
            push(PathBuf::from(&token));
        } else if token.contains('/') || token == ".." || token == "." {
            push(base_cwd.join(&token));
        }

        if let Some(eq) = token.find('=') {
            let after = &token[eq + 1..];
            if after.starts_with('/') {
                push(PathBuf::from(after));
            } else if after.starts_with('$') {
                let (var_tok, suffix) = match after.find('/') {
                    Some(i) => (&after[..i], &after[i..]),
                    None    => (after, ""),
                };
                if let Some(val) = resolve_env_token(var_tok) {
                    let full = format!("{val}{suffix}");
                    let p = PathBuf::from(full);
                    push(if p.is_absolute() { p } else { base_cwd.join(p) });
                }
            } else if after == "~" {
                if let Ok(home) = env::var("HOME") { push(PathBuf::from(home)); }
            } else if let Some(rest) = after.strip_prefix("~/") {
                if let Ok(home) = env::var("HOME") { push(PathBuf::from(format!("{home}/{rest}"))); }
            }
        }
    }
    out
}

/// Pick the candidate CWD that resolves the most relative paths.
/// Ties go to the first candidate (always the tracked/logged CWD).
fn best_candidate(candidates: &[PathBuf], rel_paths: &[String], fs: &dyn Filesystem) -> PathBuf {
    assert!(!candidates.is_empty());
    candidates.iter()
        .enumerate()
        .max_by_key(|(i, cwd)| {
            let count = rel_paths.iter().filter(|p| exists_at(p, cwd, fs)).count();
            (count, usize::MAX - i)
        })
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| candidates[0].clone())
}

// ── Core pipeline ─────────────────────────────────────────────────────────────

fn run_harvest(
    lines: Vec<String>,
    cwd: PathBuf,
    prompt: Option<String>,
    cwd_log: Vec<(PathBuf, String)>,
    fs: &dyn Filesystem,
    debug: bool,
) -> Vec<String> {
    let debug_path: Option<&str> = if debug { Some("/tmp/harvest_debug.txt") } else { None };
    macro_rules! dbg {
        ($($arg:tt)*) => {
            if let Some(path) = debug_path {
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    let _ = writeln!(f, $($arg)*);
                }
            }
        }
    }

    dbg!("cwd: {cwd:?}");
    dbg!("prompt: {prompt:?}");
    dbg!("lines: {}", lines.len());

    let prompt_literals_storage: Vec<String> = prompt.as_deref()
        .map(|p| parse_prompt_pattern(p)
            .split_whitespace()
            .filter(|s| s.len() >= 2)
            .map(|s| s.to_string())
            .collect())
        .unwrap_or_default();
    let prompt_literals: Vec<&str> = prompt_literals_storage.iter().map(|s| s.as_str()).collect();
    let use_prompt = prompt.is_some() && !prompt_literals.is_empty();

    dbg!("use_prompt: {use_prompt}, literals: {prompt_literals:?}");

    let mut section_cwds: Vec<(usize, Vec<PathBuf>)> = Vec::new();

    if use_prompt {
        let prompt_indices: Vec<usize> = lines.iter().enumerate()
            .filter(|(_, l)| is_prompt_line(l, &prompt_literals))
            .map(|(i, _)| i)
            .collect();

        dbg!("prompt line indices: {prompt_indices:?}");
        dbg!("cwd_log: {} entries", cwd_log.len());

        // Step 1: undo_cd baseline
        let mut undo_baseline: Vec<(usize, PathBuf)> = Vec::new();
        {
            let mut cur = cwd.clone();
            for &i in prompt_indices.iter().rev().take(20) {
                let prev = if let Some(target) = find_cd_in_line(&lines[i]) {
                    dbg!("  undo_cd prompt[{i}]: cd {target:?} from {cur:?}");
                    undo_cd(target, &cur).unwrap_or_else(|| cur.clone())
                } else {
                    cur.clone()
                };
                undo_baseline.push((i, prev.clone()));
                cur = prev;
            }
        }

        // Step 2: prefer cwd_log match per prompt; fall back to undo_cd baseline
        let mut log_ptr = cwd_log.len();
        for (prompt_idx, undo_cwd) in undo_baseline {
            let prompt_line = &lines[prompt_idx];
            let mut matched_cwd = None;
            let mut matched_cmd: Option<String> = None;

            let mut scan = log_ptr;
            while scan > 0 {
                scan -= 1;
                let (entry_cwd, cmd) = &cwd_log[scan];
                if !cmd.is_empty() && prompt_line.ends_with(cmd.as_str()) {
                    let before = prompt_line.len() - cmd.len();
                    if before == 0 || prompt_line.as_bytes()[before - 1] == b' ' {
                        matched_cwd = Some(entry_cwd.clone());
                        matched_cmd = Some(cmd.clone());
                        log_ptr = scan;
                        break;
                    }
                }
            }

            let tracked_cwd = matched_cwd.unwrap_or(undo_cwd);
            let cmd_str: &str = matched_cmd.as_deref().unwrap_or_else(||
                extract_command_from_prompt_line(prompt_line, &prompt_literals));

            let mut candidates = vec![tracked_cwd.clone()];
            for dir in extract_dir_candidates(cmd_str, &tracked_cwd, fs) {
                if !candidates.contains(&dir) { candidates.push(dir); }
            }

            dbg!("  prompt[{prompt_idx}] {prompt_line:?} → tracked={tracked_cwd:?}, \
                  {} extra candidates", candidates.len() - 1);
            section_cwds.push((prompt_idx, candidates));
        }
    }

    // Resolve each section to a single best CWD
    let resolved_cwds: Vec<(usize, PathBuf)> = section_cwds.iter().enumerate()
        .map(|(k, (start_idx, candidates))| {
            let end_idx = if k == 0 { lines.len() } else { section_cwds[k - 1].0 };
            let rel_paths: Vec<String> = lines[*start_idx..end_idx].iter()
                .flat_map(|l| extract_relative(l).into_iter().chain(extract_dotwords(l)))
                .collect();
            let best = best_candidate(candidates, &rel_paths, fs);
            dbg!("  section[{start_idx}..{end_idx}] best_cwd={best:?} \
                  ({} candidates, {} rel paths)", candidates.len(), rel_paths.len());
            (*start_idx, best)
        })
        .collect();

    let cwd_for_line = |line_idx: usize| -> &Path {
        for (section_start, section_cwd) in &resolved_cwds {
            if line_idx >= *section_start { return section_cwd.as_path(); }
        }
        cwd.as_path()
    };

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (i, line) in lines.iter().enumerate().rev() {
        let line_cwd = cwd_for_line(i);

        let abs_candidates: Vec<String> = extract_absolute(line).into_iter()
            .chain(extract_absolute_greedy(line, fs))
            .collect();
        for candidate in abs_candidates.into_iter().rev() {
            let exists = fs.exists(Path::new(&candidate));
            dbg!("  abs [{i}] {candidate:?} → exists={exists}");
            if !seen.contains(&candidate) && exists {
                for v in path_variants(&candidate).into_iter().rev() {
                    if seen.insert(v.clone()) { out.push(v); }
                }
            }
        }

        for candidate in extract_relative(line).into_iter().rev() {
            let ex = exists_at(&candidate, line_cwd, fs);
            dbg!("  rel [{i}] {candidate:?} in {line_cwd:?} → exists={ex}");
            if !ex { continue; }
            let output = rebase_path(&candidate, line_cwd, &cwd);
            if seen.insert(output.clone()) { out.push(output); }
        }

        if use_prompt {
            for candidate in extract_dotwords(line).into_iter().rev() {
                if !exists_at(&candidate, line_cwd, fs) { continue; }
                let output = rebase_path(&candidate, line_cwd, &cwd);
                if seen.insert(output.clone()) { out.push(output); }
            }
        }
    }

    out
}

// ── Unit tests ────────────────────────────────────────────────────────────────

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
        assert_eq!(tokenize(r"foo\nbar"), vec![r"foo\nbar"]);
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
        assert!(!pattern.contains("$("), "pattern: {pattern:?}");
        assert!(pattern.contains('➜'), "pattern: {pattern:?}");
    }

    #[test]
    fn prompt_pattern_strips_color_groups() {
        assert_eq!(parse_prompt_pattern("%{\\e[32m%}hello%{\\e[0m%}"), "hello");
    }

    #[test]
    fn prompt_pattern_keeps_n_braces() {
        assert_eq!(parse_prompt_pattern("%1{➜%}"), "➜");
    }

    #[test]
    fn prompt_pattern_strips_conditionals() {
        assert_eq!(parse_prompt_pattern("%(?:yes:no) rest"), "yes rest");
    }

    #[test]
    fn prompt_pattern_strips_percent_codes() {
        assert_eq!(parse_prompt_pattern("user %n at %m in %~"), "user  at  in");
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
        assert!(!is_prompt_line("➜  harvest", &literals));
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

    #[test]
    fn variants_with_spaces() {
        assert_eq!(path_variants("/foo/bar baz"), vec!["/foo/bar baz"]);
    }

    // Helper used by a few tests below
    fn emit_candidates(
        candidates: Vec<String>,
        seen: &mut HashSet<String>,
        cwd: &Path,
        fs: &dyn Filesystem,
    ) -> Vec<String> {
        let mut out = Vec::new();
        for candidate in candidates {
            if !seen.contains(&candidate) && exists_at(&candidate, cwd, fs) {
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
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"), &RealFs);
        assert_eq!(result, vec!["/tmp"]);
    }

    #[test]
    fn no_space_path_deduped_on_second_occurrence() {
        let mut seen = HashSet::new();
        emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"), &RealFs);
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"), &RealFs);
        assert!(result.is_empty());
    }

    #[test]
    fn space_path_emits_both_variants() {
        let mut seen = HashSet::new();
        let result = emit_candidates(vec!["/tmp".to_string()], &mut seen, Path::new("/"), &RealFs);
        assert_eq!(result.len(), 1);
        assert!(seen.contains("/tmp"));
    }

    // --- extract_absolute_greedy ---

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
        assert_eq!(
            rebase_path("./src/main.rs", Path::new("/harvest"), Path::new("/harvest/src")),
            "./main.rs"
        );
    }

    #[test]
    fn rebase_same_cwd() {
        assert_eq!(
            rebase_path("./src/main.rs", Path::new("/harvest"), Path::new("/harvest")),
            "./src/main.rs"
        );
    }

    #[test]
    fn rebase_outside_current_cwd_uses_dotdot() {
        assert_eq!(
            rebase_path("./foo.rs", Path::new("/project/tests"), Path::new("/project/src")),
            "../tests/foo.rs"
        );
    }

    #[test]
    fn rebase_deep_mismatch_falls_back_to_absolute() {
        assert_eq!(
            rebase_path("./file.txt", Path::new("/x/y"), Path::new("/a/b/c/d/e")),
            "/x/y/file.txt"
        );
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

    // --- undo_cd ---

    #[test]
    fn undo_cd_simple_subdir() {
        assert_eq!(
            undo_cd("src", Path::new("/Users/anton/proj/harvest/src")),
            Some(PathBuf::from("/Users/anton/proj/harvest"))
        );
    }

    #[test]
    fn undo_cd_multi_component() {
        assert_eq!(
            undo_cd("proj/harvest", Path::new("/Users/anton/proj/harvest")),
            Some(PathBuf::from("/Users/anton"))
        );
    }

    #[test]
    fn undo_cd_mismatch_returns_none() {
        assert_eq!(undo_cd("other", Path::new("/Users/anton/proj/harvest/src")), None);
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
        assert_eq!(
            parse_cd("cd /tmp", Path::new("/"), &RealFs),
            Some(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn parse_cd_tilde() {
        let home = env::var("HOME").unwrap_or_default();
        assert_eq!(
            parse_cd("cd ~", Path::new("/tmp"), &RealFs),
            Some(PathBuf::from(&home))
        );
    }

    #[test]
    fn parse_cd_dash_returns_none() {
        assert_eq!(parse_cd("cd -", Path::new("/tmp"), &RealFs), None);
    }

    #[test]
    fn parse_cd_no_cd_returns_none() {
        assert_eq!(parse_cd("ls -la", Path::new("/tmp"), &RealFs), None);
    }

    #[test]
    fn parse_cd_nonexistent_returns_none() {
        assert_eq!(parse_cd("cd /this/does/not/exist/ever", Path::new("/tmp"), &RealFs), None);
    }

    #[test]
    fn parse_cd_quoted() {
        assert_eq!(
            parse_cd("cd \"/tmp\"", Path::new("/"), &RealFs),
            Some(PathBuf::from("/tmp"))
        );
    }

    // parse_cd with MemFs
    #[test]
    fn parse_cd_memfs() {
        let fs = MemFs::new(&["/proj/yankrich/src/main.rs"]);
        assert_eq!(
            parse_cd("cd /proj/yankrich", Path::new("/"), &fs),
            Some(PathBuf::from("/proj/yankrich"))
        );
        assert_eq!(
            parse_cd("cd /proj/yankrich/src", Path::new("/proj/yankrich"), &fs),
            Some(PathBuf::from("/proj/yankrich/src"))
        );
        assert_eq!(parse_cd("cd /proj/other", Path::new("/"), &fs), None);
    }

    // --- resolve_env_token ---

    #[test]
    fn resolve_env_token_dollar_var() {
        let home = env::var("HOME").unwrap_or_default();
        if !home.is_empty() { assert_eq!(resolve_env_token("$HOME"), Some(home)); }
    }

    #[test]
    fn resolve_env_token_braces() {
        let home = env::var("HOME").unwrap_or_default();
        if !home.is_empty() { assert_eq!(resolve_env_token("${HOME}"), Some(home)); }
    }

    #[test]
    fn resolve_env_token_unknown_returns_none() {
        assert_eq!(resolve_env_token("$HARVEST_TOTALLY_UNKNOWN_VAR_XYZ123"), None);
    }

    #[test]
    fn resolve_env_token_no_dollar_returns_none() {
        assert_eq!(resolve_env_token("HOME"), None);
    }

    #[test]
    fn resolve_env_token_bare_dollar_returns_none() {
        assert_eq!(resolve_env_token("$"), None);
    }

    // --- extract_dir_candidates ---

    #[test]
    fn dir_candidates_absolute_dir() {
        let c = extract_dir_candidates("find /tmp -name '*.rs'", Path::new("/"), &RealFs);
        assert!(c.contains(&PathBuf::from("/tmp")), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_absolute_file_excluded() {
        let c = extract_dir_candidates("/etc/hosts", Path::new("/"), &RealFs);
        assert!(!c.contains(&PathBuf::from("/etc/hosts")), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_dotdot() {
        let c = extract_dir_candidates("cd ..", Path::new("/tmp"), &RealFs);
        assert!(c.iter().any(|p| p == Path::new("/")), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_subshell_pattern() {
        let c = extract_dir_candidates("cd .. && find . | grep main", Path::new("/tmp"), &RealFs);
        assert!(c.iter().any(|p| p == Path::new("/")), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_inline_assignment_abs() {
        let c = extract_dir_candidates("MYDIR=/tmp vim file.txt", Path::new("/"), &RealFs);
        assert!(c.contains(&PathBuf::from("/tmp")), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_dedup() {
        let c = extract_dir_candidates("/tmp /tmp", Path::new("/"), &RealFs);
        assert_eq!(c.iter().filter(|p| *p == Path::new("/tmp")).count(), 1);
    }

    #[test]
    fn dir_candidates_no_paths_returns_empty() {
        let c = extract_dir_candidates("echo hello world", Path::new("/tmp"), &RealFs);
        assert!(c.is_empty(), "got: {c:?}");
    }

    #[test]
    fn dir_candidates_memfs() {
        let fs = MemFs::new(&["/proj/yankrich/src/main.rs"]);
        let c = extract_dir_candidates("cd ..", Path::new("/proj/yankrich"), &fs);
        assert!(c.contains(&PathBuf::from("/proj")), "got: {c:?}");
        // /tmp is not in MemFs → not a candidate
        let c2 = extract_dir_candidates("find /tmp", Path::new("/"), &fs);
        assert!(!c2.contains(&PathBuf::from("/tmp")), "got: {c2:?}");
    }

    #[test]
    fn dir_candidates_escaped_space_absolute() {
        if Path::new("/Library/Application Support").is_dir() {
            let c = extract_dir_candidates(
                r"cd /Library/Application\ Support", Path::new("/"), &RealFs,
            );
            assert!(c.contains(&PathBuf::from("/Library/Application Support")), "got: {c:?}");
        }
    }

    // --- best_candidate ---

    #[test]
    fn best_candidate_picks_most_matches() {
        let candidates = vec![PathBuf::from("/"), PathBuf::from("/tmp")];
        let rel_paths = vec!["./tmp".to_string(), "./etc".to_string()];
        assert_eq!(best_candidate(&candidates, &rel_paths, &RealFs), PathBuf::from("/"));
    }

    #[test]
    fn best_candidate_tie_goes_to_first() {
        let candidates = vec![PathBuf::from("/tmp"), PathBuf::from("/")];
        assert_eq!(best_candidate(&candidates, &[], &RealFs), PathBuf::from("/tmp"));
    }

    #[test]
    fn best_candidate_memfs() {
        let fs = MemFs::new(&[
            "/proj/yankrich/src/ansi.rs",
            "/proj/yankrich/src/main.rs",
        ]);
        // From /proj/yankrich, "./src/ansi.rs" exists; from /proj it doesn't
        let candidates = vec![
            PathBuf::from("/proj"),
            PathBuf::from("/proj/yankrich"),
        ];
        let rel_paths = vec!["./src/ansi.rs".to_string(), "./src/main.rs".to_string()];
        assert_eq!(
            best_candidate(&candidates, &rel_paths, &fs),
            PathBuf::from("/proj/yankrich"),
            "yankrich should win — both paths resolve there"
        );
    }

    // --- MemFs ---

    #[test]
    fn memfs_file_exists() {
        let fs = MemFs::new(&["/proj/src/main.rs"]);
        assert!(fs.exists(Path::new("/proj/src/main.rs")));
        assert!(!fs.exists(Path::new("/proj/src/other.rs")));
    }

    #[test]
    fn memfs_parent_dirs_inferred() {
        let fs = MemFs::new(&["/proj/src/main.rs"]);
        assert!(fs.exists(Path::new("/proj/src")));
        assert!(fs.exists(Path::new("/proj")));
        assert!(fs.exists(Path::new("/")));
    }

    #[test]
    fn memfs_is_dir() {
        let fs = MemFs::new(&["/proj/src/main.rs"]);
        assert!(fs.is_dir(Path::new("/proj/src")));
        assert!(!fs.is_dir(Path::new("/proj/src/main.rs")));
    }

    #[test]
    fn memfs_normalize_on_lookup() {
        let fs = MemFs::new(&["/proj/src/main.rs"]);
        assert!(fs.exists(Path::new("/proj/./src/main.rs")));
        assert!(fs.exists(Path::new("/proj/other/../src/main.rs")));
    }
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod integration {
    use super::*;

    // The typical oh-my-zsh prompt format
    const PROMPT: &str = r"%(?:%{%}%1{➜%} :%{%}%1{➜%} ) %{%}%c%{%} $(git_prompt_info)";

    struct Scenario {
        fs: MemFs,
    }

    impl Scenario {
        fn new(files: &[&str]) -> Self {
            Self { fs: MemFs::new(files) }
        }

        fn run(
            &self,
            scrollback: &[&str],
            cwd: &str,
            prompt: Option<&str>,
            cwd_log: &[(&str, &str)],
        ) -> Vec<String> {
            let lines = scrollback.iter().map(|s| s.to_string()).collect();
            let log = cwd_log.iter()
                .map(|(c, cmd)| (PathBuf::from(*c), cmd.to_string()))
                .collect();
            run_harvest(lines, PathBuf::from(cwd), prompt.map(str::to_string), log, &self.fs, false)
        }
    }

    // Build a prompt line matching the typical zsh theme (no git info)
    fn pl(dir: &str) -> String {
        format!("➜  {dir} ")
    }

    // Build a prompt line with git branch
    fn plg(dir: &str, branch: &str) -> String {
        format!("➜  {dir} git:({branch}) ✗ ")
    }

    #[test]
    fn basic_relative_paths_no_cd() {
        let s = Scenario::new(&[
            "/proj/yankrich/src/ansi.rs",
            "/proj/yankrich/src/main.rs",
        ]);
        let result = s.run(
            &[
                &plg("yankrich", "main"),
                "./src/ansi.rs",
                "./src/main.rs",
                &format!("{}find . | grep rs$", plg("yankrich", "main")),
            ],
            "/proj/yankrich",
            Some(PROMPT),
            &[("/proj/yankrich", "find . | grep rs$")],
        );
        assert!(result.iter().any(|p| p == "./src/ansi.rs"), "got: {result:?}");
        assert!(result.iter().any(|p| p == "./src/main.rs"), "got: {result:?}");
    }

    #[test]
    fn relative_paths_rebased_after_cd() {
        // Reproduces the reported bug: find output in yankrich/ seen from proj/ after cd ..
        let s = Scenario::new(&[
            "/proj/yankrich/src/ansi.rs",
            "/proj/yankrich/src/main.rs",
            "/proj/yankrich/src/blocks.rs",
            "/proj/yankrich/src/rtf.rs",
        ]);
        let result = s.run(
            &[
                &format!("{}find . | grep rs$", plg("yankrich", "main")),
                "./src/ansi.rs",
                "./src/main.rs",
                "./src/blocks.rs",
                "./src/rtf.rs",
                &format!("{}cd ..", plg("yankrich", "main")),
                &pl("proj"),
            ],
            "/proj",
            Some(PROMPT),
            &[
                ("/proj/yankrich", "find . | grep rs$"),
                ("/proj", "cd .."),
            ],
        );
        assert!(
            result.iter().any(|p| p == "./yankrich/src/ansi.rs"),
            "expected ./yankrich/src/ansi.rs, got: {result:?}"
        );
        assert!(
            result.iter().any(|p| p == "./yankrich/src/main.rs"),
            "got: {result:?}"
        );
    }

    #[test]
    fn absolute_paths_extracted() {
        let s = Scenario::new(&["/proj/harvest/src/main.rs"]);
        let result = s.run(
            &["error at /proj/harvest/src/main.rs:42"],
            "/proj",
            None,
            &[],
        );
        assert!(
            result.contains(&"/proj/harvest/src/main.rs".to_string()),
            "got: {result:?}"
        );
    }

    #[test]
    fn subshell_cd_resolved_via_candidates() {
        // (cd .. && find . | grep rs$) run from /proj/yankrich
        // preexec records CWD=/proj/yankrich but paths are relative to /proj
        let s = Scenario::new(&[
            "/proj/src/lib.rs",
            "/proj/src/main.rs",
        ]);
        let result = s.run(
            &[
                &format!("{}(cd .. && find . | grep rs$)", plg("yankrich", "main")),
                "./src/lib.rs",
                "./src/main.rs",
                &pl("proj"),
            ],
            "/proj",
            Some(PROMPT),
            &[
                ("/proj/yankrich", "(cd .. && find . | grep rs$)"),
                ("/proj", ""),
            ],
        );
        // Candidates: [/proj/yankrich (tracked), /proj (from `..`), /proj/yankrich (from `.`)]
        // /proj wins since both paths resolve there
        assert!(
            result.iter().any(|p| p == "./src/lib.rs" || p == "/proj/src/lib.rs"),
            "got: {result:?}"
        );
    }

    #[test]
    fn no_prompt_no_relative_dotwords() {
        // Without prompt tracking, dotwords should not appear
        let s = Scenario::new(&["/proj/src/main.rs"]);
        let result = s.run(
            &["edit main.rs and done"],
            "/proj/src",
            None, // no prompt
            &[],
        );
        assert!(!result.contains(&"main.rs".to_string()), "got: {result:?}");
    }
}

// ── CLI ───────────────────────────────────────────────────────────────────────

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

    if args.debug {
        let _ = std::fs::write("/tmp/harvest_debug.txt", "");
    }

    let cwd_log: Vec<(PathBuf, String)> = args.cwd_log.as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().filter_map(|line| {
            let (cwd, cmd) = line.split_once('\t')?;
            Some((PathBuf::from(cwd), cmd.to_string()))
        }).collect())
        .unwrap_or_default();

    for path in run_harvest(lines, args.cwd, args.prompt, cwd_log, &RealFs, args.debug) {
        println!("{path}");
    }
}
