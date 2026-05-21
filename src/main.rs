use std::collections::HashSet;
use std::io::{self, BufRead};
use std::path::Path;

trait Extractor {
    fn extract(&self, line: &str) -> Vec<String>;
}

struct AbsolutePathExtractor;

impl Extractor for AbsolutePathExtractor {
    fn extract(&self, line: &str) -> Vec<String> {
        let mut results = Vec::new();
        let bytes = line.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] == b'/' {
                let start = i;
                let end = line[start..]
                    .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ')' | ']' | '>'))
                    .map(|j| start + j)
                    .unwrap_or(len);

                let candidate = line[start..end].trim_end_matches(|c| matches!(c, '.' | ',' | ':' | ';'));
                if candidate.len() > 1 {
                    results.push(candidate.to_string());
                }
                i = end;
            } else {
                i += 1;
            }
        }

        results
    }
}

fn run(extractors: &[Box<dyn Extractor>]) {
    let stdin = io::stdin();
    let lines: Vec<String> = stdin.lock().lines().filter_map(|l| l.ok()).collect();

    let mut seen = HashSet::new();

    for line in lines.iter().rev() {
        for extractor in extractors {
            for candidate in extractor.extract(line) {
                if seen.contains(&candidate) {
                    continue;
                }
                if Path::new(&candidate).exists() {
                    println!("{}", candidate);
                    seen.insert(candidate);
                }
            }
        }
    }
}

fn main() {
    let extractors: Vec<Box<dyn Extractor>> = vec![Box::new(AbsolutePathExtractor)];
    run(&extractors);
}
