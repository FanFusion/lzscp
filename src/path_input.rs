use std::path::PathBuf;

use percent_encoding::percent_decode_str;

/// Parse a bracketed-paste / drag-drop payload into one or more local paths.
///
/// Handles the formatting variants produced by iTerm2, Terminal.app, Ghostty,
/// kitty, WezTerm, Alacritty, GNOME Terminal, Windows Terminal, etc.:
/// - Outer quoting: `""`, `''`
/// - Escaped spaces: `foo\ bar`
/// - URL form: `file:///…` with percent-encoded UTF-8
/// - Multi-path: `\n`-separated or shell-style whitespace-separated
/// - `~` / `$HOME` expansion
///
/// Does not touch the filesystem for existence checks — callers decide.
pub fn parse_paste(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return vec![];
    }

    // 1) Multi-line input = one path per non-empty line.
    if trimmed.contains('\n') {
        return trimmed
            .split('\n')
            .map(|s| s.trim().trim_end_matches('\r'))
            .filter(|s| !s.is_empty())
            .flat_map(normalize_one_line)
            .collect();
    }

    // 2) Single line: could still contain multiple shell-quoted tokens.
    normalize_one_line(trimmed)
}

fn normalize_one_line(line: &str) -> Vec<String> {
    // Fast path: single surrounding pair of quotes — treat as one path.
    if let Some(inner) = strip_outer_quotes(line) {
        return vec![normalize_single(inner)];
    }

    // Try shell-style split. This respects nested quoting and escaped spaces.
    if let Some(tokens) = shlex::split(line) {
        if tokens.len() >= 2 {
            return tokens.into_iter().map(|t| normalize_single(&t)).collect();
        }
        if let Some(tok) = tokens.into_iter().next() {
            return vec![normalize_single(&tok)];
        }
    }

    // Fallback: treat whole line as one path, manually unescape.
    vec![normalize_single(line)]
}

fn strip_outer_quotes(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return Some(&s[1..s.len() - 1]);
        }
    }
    None
}

fn normalize_single(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    // Strip any residual outer quotes (shlex removes them; safety net).
    if let Some(inner) = strip_outer_quotes(&s) {
        s = inner.to_string();
    }

    // file:// URL form → decode.
    if let Some(rest) = s.strip_prefix("file://") {
        // drop leading host component if present — usually empty for file://
        let (_host, path) = split_file_uri_host(rest);
        s = percent_decode_str(path)
            .decode_utf8_lossy()
            .to_string();
    }

    // Manual unescape for backslash-escaped spaces and backslashes (for inputs
    // that did not round-trip through shlex).
    s = unescape_backslash(&s);

    // ~ and $HOME expansion.
    s = shellexpand::full(&s)
        .map(|v| v.into_owned())
        .unwrap_or(s);

    s
}

fn split_file_uri_host(rest: &str) -> (&str, &str) {
    // `file://host/path` → ("host", "/path")
    // `file:///path`     → ("", "/path")
    if let Some(idx) = rest.find('/') {
        (&rest[..idx], &rest[idx..])
    } else {
        ("", rest)
    }
}

fn unescape_backslash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                // `\ ` → ` `, `\\` → `\`, `\"` → `"`, `\'` → `'`, `\t` → tab
                match next {
                    ' ' | '\\' | '"' | '\'' | '(' | ')' | '&' | ';' | '$' | '`' => {
                        out.push(next);
                        chars.next();
                        continue;
                    }
                    _ => {
                        // keep backslash + next as-is (e.g. Windows paths `C:\Users\x`)
                    }
                }
            }
        }
        out.push(c);
    }
    out
}

pub fn path_exists(p: &str) -> bool {
    PathBuf::from(p).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_path() {
        assert_eq!(parse_paste("/tmp/file.png"), vec!["/tmp/file.png"]);
    }

    #[test]
    fn unquotes_double_quotes() {
        assert_eq!(
            parse_paste("\"/tmp/my file.png\""),
            vec!["/tmp/my file.png"]
        );
    }

    #[test]
    fn unquotes_single_quotes() {
        assert_eq!(
            parse_paste("'/tmp/中文 文件.png'"),
            vec!["/tmp/中文 文件.png"]
        );
    }

    #[test]
    fn unescapes_space() {
        assert_eq!(parse_paste("/tmp/my\\ file.png"), vec!["/tmp/my file.png"]);
    }

    #[test]
    fn chinese_path_no_quotes() {
        assert_eq!(parse_paste("/tmp/截图.png"), vec!["/tmp/截图.png"]);
    }

    #[test]
    fn chinese_with_space_escaped() {
        assert_eq!(
            parse_paste("/tmp/截图\\ 1.png"),
            vec!["/tmp/截图 1.png"]
        );
    }

    #[test]
    fn emoji_path() {
        assert_eq!(parse_paste("/tmp/🎉.png"), vec!["/tmp/🎉.png"]);
    }

    #[test]
    fn file_uri_url_encoded_chinese() {
        assert_eq!(
            parse_paste("file:///tmp/%E6%B5%8B%E8%AF%95.png"),
            vec!["/tmp/测试.png"]
        );
    }

    #[test]
    fn file_uri_with_space_encoded() {
        assert_eq!(
            parse_paste("file:///tmp/my%20file.png"),
            vec!["/tmp/my file.png"]
        );
    }

    #[test]
    fn multi_path_newline() {
        let out = parse_paste("/tmp/a.png\n/tmp/b.png");
        assert_eq!(out, vec!["/tmp/a.png", "/tmp/b.png"]);
    }

    #[test]
    fn multi_path_shell_split() {
        let out = parse_paste("/tmp/foo\\ bar.png /tmp/baz.png");
        assert_eq!(out, vec!["/tmp/foo bar.png", "/tmp/baz.png"]);
    }

    #[test]
    fn multi_path_quoted_and_unquoted() {
        let out = parse_paste("\"/tmp/a b.png\" /tmp/c.png");
        assert_eq!(out, vec!["/tmp/a b.png", "/tmp/c.png"]);
    }

    #[test]
    fn tilde_expansion() {
        let out = parse_paste("~/foo.png");
        assert!(!out[0].starts_with("~"));
        assert!(out[0].ends_with("/foo.png"));
    }

    #[test]
    fn windows_style_preserved() {
        // Outer quotes stripped; internal backslashes preserved because the
        // chars after them aren't in the unescape whitelist.
        let out = parse_paste("\"C:\\Users\\x\\a.png\"");
        assert_eq!(out, vec!["C:\\Users\\x\\a.png"]);
    }

    #[test]
    fn whitespace_only_returns_empty() {
        assert!(parse_paste("   \n  ").is_empty());
    }

    #[test]
    fn trims_trailing_cr() {
        let out = parse_paste("/tmp/a.png\r\n/tmp/b.png\r\n");
        assert_eq!(out, vec!["/tmp/a.png", "/tmp/b.png"]);
    }
}
