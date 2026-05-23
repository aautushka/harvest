mod fs;
mod path;
mod parse;
mod prompt;
mod cd;

use std::collections::HashSet;
use std::env;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use fs::{Filesystem, RealFs};
use path::{rebase_path, exists_at, path_variants};
use parse::{extract_absolute, extract_relative, extract_dotwords, extract_absolute_greedy};
use prompt::{parse_prompt_pattern, is_prompt_line, find_cd_in_line,
             extract_command_from_prompt_line, prompt_line_dirname};
use cd::{undo_cd, extract_dir_candidates, best_candidate};

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
                    undo_cd(target, &cur)
                        .or_else(|| {
                            // Can't reverse algebraically (e.g. cd .., cd /abs).
                            // Try the dirname shown in the prompt as the pre-cd location.
                            prompt_line_dirname(&lines[i])
                                .map(|name| cur.join(name))
                                .filter(|p| fs.is_dir(p))
                        })
                        .unwrap_or_else(|| cur.clone())
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

// ── Scenario tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod scenarios;

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
    let cwd_log = args.cwd_log.as_ref()
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
