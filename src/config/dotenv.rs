//! Minimal `.env` parser.
//!
//! The process environment is never modified (that would need `unsafe`);
//! callers look keys up in the returned map after checking the real
//! environment.
//!
//! Supported syntax: `KEY=value`, an optional `export ` prefix, blank lines,
//! `#` comment lines, matching single or double quotes around the value
//! (no escape sequences), and ` #` inline comments after unquoted values.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::error::ConfigError;

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub reason: &'static str,
}

pub fn parse(text: &str) -> Result<HashMap<String, String>, ParseError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut vars = HashMap::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);

        let Some((key, value)) = line.split_once('=') else {
            return Err(ParseError {
                line: line_no,
                reason: "expected KEY=value",
            });
        };
        let key = key.trim();
        if !is_valid_key(key) {
            return Err(ParseError {
                line: line_no,
                reason: "invalid variable name",
            });
        }
        vars.insert(key.to_owned(), parse_value(value));
    }

    Ok(vars)
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_value(raw: &str) -> String {
    let raw = raw.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = raw
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    match raw.find(" #") {
        Some(pos) => raw.get(..pos).unwrap_or(raw).trim_end().to_owned(),
        None => raw.to_owned(),
    }
}

/// Reads the first existing file among `candidates`. A missing file is not an
/// error; no file at all yields an empty map.
pub fn load_first(
    candidates: &[PathBuf],
) -> Result<(HashMap<String, String>, Option<PathBuf>), ConfigError> {
    for path in candidates {
        match read(path) {
            Ok(Some(vars)) => return Ok((vars, Some(path.clone()))),
            Ok(None) => continue,
            Err(err) => return Err(err),
        }
    }
    Ok((HashMap::new(), None))
}

fn read(path: &Path) -> Result<Option<HashMap<String, String>>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    parse(&text).map(Some).map_err(|e| ConfigError::Dotenv {
        path: path.to_owned(),
        line: e.line,
        reason: e.reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(m: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
        m.get(k).map(String::as_str)
    }

    #[test]
    fn parses_plain_pairs_comments_and_blank_lines() {
        let m = parse("# comment\n\nA=1\n  B = two words \n").unwrap();
        assert_eq!(get(&m, "A"), Some("1"));
        assert_eq!(get(&m, "B"), Some("two words"));
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn supports_export_prefix_and_quotes() {
        let m = parse("export A=\"x y\"\nB='z # not a comment'\nC=\"\"\n").unwrap();
        assert_eq!(get(&m, "A"), Some("x y"));
        assert_eq!(get(&m, "B"), Some("z # not a comment"));
        assert_eq!(get(&m, "C"), Some(""));
    }

    #[test]
    fn strips_inline_comment_only_when_preceded_by_space() {
        let m = parse("A=abc # note\nB=abc#keep\n").unwrap();
        assert_eq!(get(&m, "A"), Some("abc"));
        assert_eq!(get(&m, "B"), Some("abc#keep"));
    }

    #[test]
    fn value_may_contain_equals_sign() {
        let m = parse("KEY=a=b==\n").unwrap();
        assert_eq!(get(&m, "KEY"), Some("a=b=="));
    }

    #[test]
    fn handles_bom_and_crlf() {
        let m = parse("\u{feff}A=1\r\nB=2\r\n").unwrap();
        assert_eq!(get(&m, "A"), Some("1"));
        assert_eq!(get(&m, "B"), Some("2"));
    }

    #[test]
    fn last_duplicate_wins() {
        let m = parse("A=1\nA=2\n").unwrap();
        assert_eq!(get(&m, "A"), Some("2"));
    }

    #[test]
    fn rejects_malformed_lines_without_echoing_content() {
        let err = parse("A=1\nsecret-without-equals\n").unwrap_err();
        assert_eq!(err.line, 2);
        assert!(!err.reason.contains("secret"));

        assert_eq!(parse("1BAD=x").unwrap_err().line, 1);
        assert_eq!(parse("=x").unwrap_err().reason, "invalid variable name");
        assert_eq!(parse("A B=x").unwrap_err().reason, "invalid variable name");
    }

    #[test]
    fn missing_files_are_skipped() {
        let (vars, path) = load_first(&[PathBuf::from("/definitely/not/here/.env")]).unwrap();
        assert!(vars.is_empty());
        assert!(path.is_none());
    }

    #[test]
    fn loads_first_existing_file() {
        let dir =
            std::env::temp_dir().join(format!("stream-spectre-dotenv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        std::fs::write(&file, "A=from-file\n").unwrap();

        let (vars, path) = load_first(&[dir.join("missing"), file.clone()]).unwrap();
        assert_eq!(get(&vars, "A"), Some("from-file"));
        assert_eq!(path, Some(file));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
