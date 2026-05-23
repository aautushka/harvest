pub(crate) fn collect_paren_content<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> String {
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

pub(crate) fn parse_prompt_pattern(prompt: &str) -> String {
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

pub(crate) fn is_prompt_line(line: &str, prompt_literals: &[&str]) -> bool {
    !prompt_literals.is_empty() && prompt_literals.iter().all(|lit| line.contains(lit))
}

pub(crate) fn extract_command_from_prompt_line<'a>(
    line: &'a str,
    prompt_literals: &[&str],
) -> &'a str {
    let last_end = prompt_literals.iter()
        .filter_map(|lit| line.rfind(lit).map(|pos| pos + lit.len()))
        .max()
        .unwrap_or(0);
    line[last_end..].trim()
}

pub(crate) fn find_cd_in_line(line: &str) -> Option<&str> {
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

// Extract the directory basename rendered in a prompt line (the %c slot).
// Prompt lines look like: "➜  dirname [git info] [cmd]"
pub(crate) fn prompt_line_dirname(line: &str) -> Option<&str> {
    let i = line.find("➜")?;
    let rest = line[i + "➜".len()..].trim_start_matches(' ');
    let end = rest.find(|c: char| c == ' ' || c == '/').unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() { None } else { Some(name) }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
