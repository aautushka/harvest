use std::collections::HashSet;
use std::path::{Path, PathBuf, Component};

pub(crate) trait Filesystem {
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

pub(crate) struct RealFs;

impl Filesystem for RealFs {
    fn exists(&self, path: &Path) -> bool { path.exists() }
    fn is_dir(&self, path: &Path) -> bool { path.is_dir() }
}

/// In-memory filesystem for tests.
/// Declare files; all parent directories are inferred automatically.
/// Paths are normalised on lookup, so `..` and `.` components work correctly.
pub(crate) struct MemFs {
    pub(crate) files: HashSet<PathBuf>,
    pub(crate) dirs:  HashSet<PathBuf>,
}

impl MemFs {
    pub(crate) fn new(file_paths: &[&str]) -> Self {
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
                        if !dirs.insert(parent.clone()) { break; }
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

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component> = Vec::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match components.last() {
                Some(Component::RootDir) | Some(Component::Prefix(_)) | None => {
                    components.push(c)
                }
                _ => { components.pop(); }
            },
            c => components.push(c),
        }
    }
    if components.is_empty() { PathBuf::from(".") } else { components.iter().collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_removes_curdir() {
        assert_eq!(normalize_path(Path::new("/a/b/./c")), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn normalize_removes_parentdir() {
        assert_eq!(normalize_path(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
    }

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
