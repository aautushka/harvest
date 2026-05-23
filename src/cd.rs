use std::collections::HashSet;
use std::env;
use std::path::{Path, PathBuf};
use crate::fs::{Filesystem, normalize_path};
use crate::path::exists_at;
use crate::parse::tokenize;

pub(crate) fn resolve_env_token(token: &str) -> Option<String> {
    let name = token.strip_prefix('$')?;
    let name = if let Some(inner) = name.strip_prefix('{') {
        inner.trim_end_matches('}')
    } else {
        name
    };
    if name.is_empty() { return None; }
    env::var(name).ok()
}

pub(crate) fn parse_cd(cmd: &str, current_cwd: &Path, fs: &dyn Filesystem) -> Option<PathBuf> {
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

pub(crate) fn undo_cd(target: &str, result_cwd: &Path) -> Option<PathBuf> {
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

/// Extract directory candidates from a command string.
/// Handles absolute paths, relative paths, `~`, `$VAR`, `$VAR/rest`, `KEY=/path`, `KEY=$VAR`.
pub(crate) fn extract_dir_candidates(
    cmd: &str,
    base_cwd: &Path,
    fs: &dyn Filesystem,
) -> Vec<PathBuf> {
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

// Pick the candidate CWD that resolves the most relative paths.
// Assumes exactly one real CWD per command — we vote on it but can't determine
// more than one (e.g. "cd A && cmd; cd B && cmd2" in a single command string).
// Early exit when a candidate resolves every path (perfect score).
pub(crate) fn best_candidate(
    candidates: &[PathBuf],
    rel_paths: &[String],
    fs: &dyn Filesystem,
) -> PathBuf {
    assert!(!candidates.is_empty());
    let perfect = rel_paths.len();
    let mut best_cwd = &candidates[0];
    let mut best_score = 0usize;
    for cwd in candidates {
        let score = rel_paths.iter().filter(|p| exists_at(p, cwd, fs)).count();
        if score == perfect {
            return cwd.clone();
        }
        if score > best_score {
            best_score = score;
            best_cwd = cwd;
        }
    }
    best_cwd.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::fs::{MemFs, RealFs};

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

    #[test]
    fn best_candidate_early_exit_on_perfect_match() {
        let fs = MemFs::new(&[
            "/proj/a/foo.rs",
            "/proj/a/bar.rs",
            "/proj/b/foo.rs",
        ]);
        let candidates = vec![
            PathBuf::from("/proj/a"), // perfect: resolves all 2 rel_paths
            PathBuf::from("/proj/b"), // partial: only foo.rs
        ];
        let rel_paths = vec!["./foo.rs".to_string(), "./bar.rs".to_string()];
        assert_eq!(best_candidate(&candidates, &rel_paths, &fs), PathBuf::from("/proj/a"));
    }
}
