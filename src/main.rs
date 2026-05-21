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

fn trim_punctuation(s: &str) -> &str {
    s.trim_end_matches(|c| matches!(c, '.' | ',' | ':' | ';'))
}

// Extract absolute paths (start with /)
fn extract_absolute(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| t.starts_with('/') && t.len() > 1)
        .map(|t| trim_punctuation(&t).to_string())
        .collect()
}

// Extract relative paths (contain / but don't start with /)
fn extract_relative(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| !t.starts_with('/') && t.contains('/'))
        .map(|t| trim_punctuation(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

// Extract single words containing a dot (likely filenames: foo.rs, config.yaml)
fn extract_dotwords(line: &str) -> Vec<String> {
    tokenize(line)
        .into_iter()
        .filter(|t| !t.starts_with('/') && !t.contains('/') && t.contains('.') && !t.starts_with("http"))
        .map(|t| trim_punctuation(&t).to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn exists_at(candidate: &str, cwd: &Path) -> bool {
    let p = Path::new(candidate);
    if p.is_absolute() {
        p.exists()
    } else {
        cwd.join(p).exists()
    }
}

// Parse a ZSH prompt string, return a stripped version usable for line matching.
// Strips %{...%} color groups, simplifies %(?:A:B) to just A, removes other % codes.
fn parse_prompt_pattern(prompt: &str) -> String {
    let mut out = String::new();
    let mut chars = prompt.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => break,
            Some('{') => {
                // %{...%} — zero-width, skip until %}
                loop {
                    match chars.next() {
                        None | Some('%') => { chars.next(); break; } // consume }
                        _ => {}
                    }
                }
            }
            Some('(') => {
                // %(cond:true:false) — consume whole expression, emit nothing
                // Just skip until matching )
                let mut depth = 1;
                while let Some(ch) = chars.next() {
                    if ch == '(' { depth += 1; }
                    if ch == ')' { depth -= 1; if depth == 0 { break; } }
                }
            }
            Some('?') => {
                // %(?:A:B) ternary — skip
                // already consumed '?', look for surrounding parens handled above
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
            Some(_) => {} // skip other % codes (%c, %~, %n, etc.)
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

        // Absolute paths
        for candidate in extract_absolute(line) {
            if seen.insert(candidate.clone()) && Path::new(&candidate).exists() {
                println!("{}", candidate);
            }
        }

        // Relative paths (contain /)
        for candidate in extract_relative(line) {
            if seen.insert(candidate.clone()) && exists_at(&candidate, cwd) {
                println!("{}", candidate);
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
