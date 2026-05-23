use std::collections::HashSet;
use std::path::{Path, PathBuf};
use crate::fs::{Filesystem, normalize_path};

pub(crate) fn make_relative(from_dir: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to:   Vec<_> = to.components().collect();
    let common = from.iter().zip(to.iter()).take_while(|(a, b)| a == b).count();
    let mut result = PathBuf::new();
    for _ in 0..(from.len() - common) { result.push(".."); }
    for c in &to[common..] { result.push(c); }
    if result.as_os_str().is_empty() { result.push("."); }
    result
}

pub(crate) fn rebase_path(candidate: &str, section_cwd: &Path, current_cwd: &Path) -> String {
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

pub(crate) fn exists_at(candidate: &str, cwd: &Path, fs: &dyn Filesystem) -> bool {
    let p = if Path::new(candidate).is_absolute() {
        normalize_path(Path::new(candidate))
    } else {
        normalize_path(&cwd.join(candidate))
    };
    fs.exists(&p)
}

pub(crate) fn path_variants(path: &str) -> Vec<String> {
    vec![path.to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::fs::{Filesystem, RealFs};

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

    #[test]
    fn variants_no_spaces() {
        assert_eq!(path_variants("/foo/bar"), vec!["/foo/bar"]);
    }

    #[test]
    fn variants_with_spaces() {
        assert_eq!(path_variants("/foo/bar baz"), vec!["/foo/bar baz"]);
    }

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
}
