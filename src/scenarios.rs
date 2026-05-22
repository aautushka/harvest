// Scenario tests: full pipeline with a mocked filesystem.
// Each test declares the files that exist, feeds in a scrollback + cwd_log,
// and asserts on the output of run_harvest.
use super::*;

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

// Prompt line helpers matching the oh-my-zsh theme
fn pl(dir: &str) -> String { format!("➜  {dir} ") }
fn plg(dir: &str, branch: &str) -> String { format!("➜  {dir} git:({branch}) ✗ ") }
fn plgc(dir: &str, branch: &str, cmd: &str) -> String { format!("➜  {dir} git:({branch}) ✗ {cmd}") }

// ── Basic cases ───────────────────────────────────────────────────────────────

#[test]
fn relative_paths_same_dir() {
    let s = Scenario::new(&["/proj/src/main.rs", "/proj/src/lib.rs"]);
    let result = s.run(
        &[
            &plgc("proj", "main", "find . | grep rs$"),
            "./src/main.rs",
            "./src/lib.rs",
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[("/proj", "find . | grep rs$")],
    );
    assert!(result.iter().any(|p| p == "./src/main.rs"), "got: {result:?}");
    assert!(result.iter().any(|p| p == "./src/lib.rs"), "got: {result:?}");
}

#[test]
fn absolute_paths_no_prompt() {
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &["error at /proj/src/main.rs:42"],
        "/other",
        None,
        &[],
    );
    assert!(result.contains(&"/proj/src/main.rs".to_string()), "got: {result:?}");
}

#[test]
fn no_prompt_means_no_dotwords() {
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(&["edit main.rs"], "/proj/src", None, &[]);
    assert!(!result.contains(&"main.rs".to_string()), "got: {result:?}");
}

#[test]
fn dotwords_with_prompt_active() {
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &[
            &plgc("src", "main", "cargo build"),
            "   Compiling main.rs",
            &pl("src"),
        ],
        "/proj/src",
        Some(PROMPT),
        &[("/proj/src", "cargo build")],
    );
    assert!(result.iter().any(|p| p == "main.rs" || p == "./main.rs"), "got: {result:?}");
}

// ── CWD tracking ─────────────────────────────────────────────────────────────

#[test]
fn relative_paths_rebased_after_cd() {
    // The originally reported bug: find in yankrich, then cd .., trigger from proj
    let s = Scenario::new(&[
        "/proj/yankrich/src/ansi.rs",
        "/proj/yankrich/src/main.rs",
        "/proj/yankrich/src/blocks.rs",
    ]);
    let result = s.run(
        &[
            &plgc("yankrich", "main", "find . | grep rs$"),
            "./src/ansi.rs",
            "./src/main.rs",
            "./src/blocks.rs",
            &plgc("yankrich", "main", "cd .."),
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[
            ("/proj/yankrich", "find . | grep rs$"),
            ("/proj/yankrich", "cd .."),
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
fn undo_cd_fallback_when_no_log() {
    // No cwd_log at all — undo_cd must reconstruct the section CWD from prompt lines
    let s = Scenario::new(&["/proj/yankrich/src/main.rs"]);
    let result = s.run(
        &[
            &plgc("yankrich", "main", "find . | grep rs$"),
            "./src/main.rs",
            &plgc("yankrich", "main", "cd .."),
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[], // empty log
    );
    assert!(
        result.iter().any(|p| p == "./yankrich/src/main.rs"),
        "undo_cd should reconstruct yankrich CWD, got: {result:?}"
    );
}

#[test]
fn undo_cd_fallback_when_log_entry_missing() {
    // Log exists but the find command has no entry (e.g. ran before harvest was set up)
    let s = Scenario::new(&["/proj/yankrich/src/main.rs"]);
    let result = s.run(
        &[
            &plgc("yankrich", "main", "find . | grep rs$"),
            "./src/main.rs",
            &plgc("yankrich", "main", "cd .."),
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[("/proj/yankrich", "cd ..")], // only cd is logged, not find
    );
    assert!(
        result.iter().any(|p| p == "./yankrich/src/main.rs"),
        "undo_cd should fill in for missing find entry, got: {result:?}"
    );
}

#[test]
fn absolute_cd_loses_tracking_falls_back_to_args_cwd() {
    // cd /absolute — undo_cd can't reverse it; the section before it uses args.cwd
    let s = Scenario::new(&[
        "/proj/src/main.rs",
        "/other/src/lib.rs",
    ]);
    let result = s.run(
        &[
            &plgc("other", "main", "find . | grep rs$"),
            "./src/lib.rs",
            &plgc("other", "main", "cd /proj"),
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[
            ("/other", "find . | grep rs$"),
            ("/proj", "cd /proj"),
        ],
    );
    // /other/src/lib.rs should still resolve because cwd_log match gives /other
    assert!(
        result.iter().any(|p| p.contains("lib.rs")),
        "got: {result:?}"
    );
}

#[test]
fn cycling_same_locations_matches_correct_entry() {
    // User cycles A → B → A → B. Log has duplicate commands.
    // Most recent find (from B, second visit) should be used.
    let s = Scenario::new(&[
        "/proj/a/foo.rs",
        "/proj/b/bar.rs",
    ]);
    let result = s.run(
        &[
            // older visit to a
            &plgc("a", "main", "find . | grep rs$"),
            "./foo.rs",
            &plgc("a", "main", "cd ../b"),
            // newer visit to b
            &plgc("b", "main", "find . | grep rs$"),
            "./bar.rs",
            &plgc("b", "main", "cd ../a"),
            &pl("a"),
        ],
        "/proj/a",
        Some(PROMPT),
        &[
            ("/proj/a", "find . | grep rs$"),   // first visit
            ("/proj/a", "cd ../b"),
            ("/proj/b", "find . | grep rs$"),   // second visit
            ("/proj/b", "cd ../a"),
        ],
    );
    // bar.rs from the most recent find (in b) should appear as ../b/bar.rs
    assert!(
        result.iter().any(|p| p.contains("bar.rs")),
        "got: {result:?}"
    );
    // foo.rs from the older find (in a) should also appear
    assert!(
        result.iter().any(|p| p.contains("foo.rs")),
        "got: {result:?}"
    );
}

#[test]
fn most_recent_paths_appear_before_older() {
    // find in b (more recent) → bar.rs should come before foo.rs from a (older)
    let s = Scenario::new(&["/proj/a/foo.rs", "/proj/b/bar.rs"]);
    let result = s.run(
        &[
            &plgc("a", "main", "find . | grep rs$"),
            "./foo.rs",
            &plgc("a", "main", "cd ../b"),
            &plgc("b", "main", "find . | grep rs$"),
            "./bar.rs",
            &pl("b"),
        ],
        "/proj/b",
        Some(PROMPT),
        &[
            ("/proj/a", "find . | grep rs$"),
            ("/proj/a", "cd ../b"),
            ("/proj/b", "find . | grep rs$"),
        ],
    );
    let bar_pos = result.iter().position(|p| p.contains("bar.rs"));
    let foo_pos = result.iter().position(|p| p.contains("foo.rs"));
    assert!(bar_pos.is_some() && foo_pos.is_some(), "both files missing, got: {result:?}");
    assert!(
        bar_pos.unwrap() < foo_pos.unwrap(),
        "bar.rs (recent) should precede foo.rs (older), got: {result:?}"
    );
}

#[test]
fn deduplication_across_sections() {
    // main.rs appears in two different find outputs; should only appear once
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &[
            &plgc("proj", "main", "find . | grep rs$"),
            "./src/main.rs",
            &plgc("proj", "main", "find . | grep rs$"),
            "./src/main.rs",
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[
            ("/proj", "find . | grep rs$"),
            ("/proj", "find . | grep rs$"),
        ],
    );
    let count = result.iter().filter(|p| p.contains("main.rs")).count();
    assert_eq!(count, 1, "main.rs should appear exactly once, got: {result:?}");
}

// ── Subshell / candidate voting ───────────────────────────────────────────────

#[test]
fn subshell_cd_resolved_via_candidates() {
    // (cd .. && find . | grep rs$) run from /proj/sub
    // preexec records /proj/sub but paths are relative to /proj
    let s = Scenario::new(&["/proj/src/main.rs", "/proj/src/lib.rs"]);
    let result = s.run(
        &[
            &plgc("sub", "main", "(cd .. && find . | grep rs$)"),
            "./src/main.rs",
            "./src/lib.rs",
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[
            ("/proj/sub", "(cd .. && find . | grep rs$)"),
            ("/proj", ""),
        ],
    );
    // /proj is a candidate (from `..` in command); both paths exist there
    assert!(
        result.iter().any(|p| p.contains("main.rs")),
        "got: {result:?}"
    );
    assert!(
        result.iter().any(|p| p.contains("lib.rs")),
        "got: {result:?}"
    );
}

#[test]
fn find_with_relative_root_already_works() {
    // find ../../ | grep rs$ — output paths start with ../../, self-describing
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &[
            &plgc("a/b", "main", "find ../../ | grep rs$"),
            "../../src/main.rs",
            &pl("a/b"),
        ],
        "/proj/a/b",
        Some(PROMPT),
        &[("/proj/a/b", "find ../../ | grep rs$")],
    );
    // ../../src/main.rs from /proj/a/b = /proj/src/main.rs → exists → shown
    assert!(
        result.iter().any(|p| p.contains("main.rs")),
        "got: {result:?}"
    );
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[test]
fn empty_scrollback_returns_nothing() {
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(&[], "/proj", Some(PROMPT), &[]);
    assert!(result.is_empty());
}

#[test]
fn paths_not_in_fs_are_excluded() {
    // Output mentions /proj/src/ghost.rs which doesn't exist in our MemFs
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &["/proj/src/ghost.rs", "/proj/src/main.rs"],
        "/proj",
        None,
        &[],
    );
    assert!(result.contains(&"/proj/src/main.rs".to_string()), "got: {result:?}");
    assert!(!result.contains(&"/proj/src/ghost.rs".to_string()), "got: {result:?}");
}

#[test]
fn multiple_dirs_in_scrollback_no_prompt() {
    // Without prompt there's no CWD tracking; only absolute paths or paths
    // that resolve against args.cwd are shown
    let s = Scenario::new(&[
        "/proj/src/main.rs",
        "/proj/src/lib.rs",
    ]);
    let result = s.run(
        &["/proj/src/main.rs", "/proj/src/lib.rs"],
        "/proj",
        None,
        &[],
    );
    assert!(result.contains(&"/proj/src/main.rs".to_string()), "got: {result:?}");
    assert!(result.contains(&"/proj/src/lib.rs".to_string()), "got: {result:?}");
}

#[test]
fn embedded_path_in_compiler_output() {
    // CMake/GCC style: "error:/proj/src/main.rs:42"
    let s = Scenario::new(&["/proj/src/main.rs"]);
    let result = s.run(
        &["error:/proj/src/main.rs:42: something failed"],
        "/proj",
        None,
        &[],
    );
    assert!(result.contains(&"/proj/src/main.rs".to_string()), "got: {result:?}");
}

#[test]
fn deep_cd_chain_tracked_by_log() {
    // cd a, cd b, cd c — each step logged, find run in c
    let s = Scenario::new(&["/proj/a/b/c/deep.rs"]);
    let result = s.run(
        &[
            &plgc("c", "main", "find . | grep rs$"),
            "./deep.rs",
            &plgc("c", "main", "cd ../../.."),
            &pl("proj"),
        ],
        "/proj",
        Some(PROMPT),
        &[
            ("/proj/a/b/c", "find . | grep rs$"),
            ("/proj/a/b/c", "cd ../../.."),
        ],
    );
    assert!(
        result.iter().any(|p| p.contains("deep.rs")),
        "got: {result:?}"
    );
}

#[test]
fn final_prompt_with_git_info_still_terminates_section() {
    // Final prompt shows git info (user stays in git repo, no cd).
    // Uses plg() — must still be recognised as a prompt line.
    let s = Scenario::new(&["/proj/src/main.rs", "/proj/src/lib.rs"]);
    let result = s.run(
        &[
            &plgc("proj", "main", "find . | grep rs$"),
            "./src/main.rs",
            "./src/lib.rs",
            &plg("proj", "main"),
        ],
        "/proj",
        Some(PROMPT),
        &[("/proj", "find . | grep rs$")],
    );
    assert!(result.iter().any(|p| p == "./src/main.rs"), "got: {result:?}");
    assert!(result.iter().any(|p| p == "./src/lib.rs"), "got: {result:?}");
}

#[test]
fn envvar_in_command_used_as_candidate() {
    // HARVEST_TEST_DIR=/proj/src is set; command uses $HARVEST_TEST_DIR as find root.
    // extract_dir_candidates should resolve the var and find /proj/src as candidate.
    unsafe { std::env::set_var("HARVEST_TEST_DIR", "/proj/src") };
    let s = Scenario::new(&["/proj/src/main.rs", "/proj/src/lib.rs"]);
    let result = s.run(
        &[
            &plgc("proj", "main", "find $HARVEST_TEST_DIR | grep rs$"),
            "/proj/src/main.rs",
            "/proj/src/lib.rs",
            &plg("proj", "main"),
        ],
        "/proj",
        Some(PROMPT),
        &[("/proj", "find $HARVEST_TEST_DIR | grep rs$")],
    );
    assert!(result.iter().any(|p| p.contains("main.rs")), "got: {result:?}");
    assert!(result.iter().any(|p| p.contains("lib.rs")), "got: {result:?}");
}

#[test]
fn relative_dotdot_sibling_path() {
    // Compiler output: ../other/file.rs from cwd /proj/src means /proj/other/file.rs
    let s = Scenario::new(&["/proj/other/file.rs"]);
    let result = s.run(
        &[
            &plgc("src", "main", "cargo build"),
            "../other/file.rs:10: error",
            &plg("src", "main"),
        ],
        "/proj/src",
        Some(PROMPT),
        &[("/proj/src", "cargo build")],
    );
    assert!(
        result.iter().any(|p| p.contains("file.rs")),
        "got: {result:?}"
    );
}
