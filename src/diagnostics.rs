//! Parse Guile compiler output into LSP diagnostics.
//!
//! `compile-file` writes lines like
//!   `;;; /path/file.scm:2:2: warning: possibly unbound variable \`foo'`
//! to the current warning port. We turn each into a [`RawDiag`] (0-based line
//! and column) that the backend attaches to the open document's URI/range.

use crate::position::{offset_to_position, position_to_offset};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// A single parsed diagnostic, before it is attached to a document Range.
#[derive(Debug, Clone, PartialEq)]
pub struct RawDiag {
    /// 0-based line.
    pub line: u32,
    /// 0-based column.
    pub col: u32,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

/// Parse a blob of Guile compiler output into zero or more diagnostics.
/// Lines that do not match the `path:line:col: severity: msg` shape are
/// ignored (e.g. the ";;; compiling" / ";;; compiled" noise).
pub fn parse_compile_output(raw: &str) -> Vec<RawDiag> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if let Some(d) = parse_line(line) {
            out.push(d);
        }
    }
    out
}

/// Parse a Guile compile *error* message (a single string of shape
/// `path:line:col: message`, with no severity keyword) into one ERROR diag.
pub fn parse_error_output(raw: &str) -> Option<RawDiag> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    // Split off the path (may contain a drive-letter colon, e.g. `C:/...`).
    let (_path, rest) = split_path_prefix(s)?;
    // rest = ":<line>:<col>: <message>"
    let parts: Vec<&str> = rest.splitn(4, ':').collect();
    if parts.len() < 4 {
        return None;
    }
    // parts[0] is empty (the leading colon after the path).
    let line_num = parts[1].trim().parse::<u32>().ok()?;
    let col_num = parts[2].trim().parse::<u32>().ok()?;
    Some(RawDiag {
        line: line_num.saturating_sub(1),
        col: col_num.saturating_sub(1),
        severity: DiagnosticSeverity::ERROR,
        message: parts[3].trim().to_string(),
    })
}

/// Parse one Guile compile *warning* line into a diagnostic.
///
/// Line shape: `<path>:<line>:<col>: <severity>: <message...>`
/// where `<path>` may itself contain colons — notably a Windows drive letter
/// (`C:/...`) whose `:` must not be counted as a field separator. We therefore
/// locate the `:line:col:` suffix (two consecutive numeric fields) rather than
/// naively splitting on every colon.
fn parse_line(line: &str) -> Option<RawDiag> {
    // Strip leading ";;;" comment markers and surrounding whitespace.
    let mut s = line.trim_start();
    while s.starts_with(';') {
        s = &s[1..];
    }
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }

    // Split off the leading path, tolerating drive-letter colons (C:, D:).
    // The remainder always begins with `:line:col: ...`.
    let (_path, rest) = split_path_prefix(s)?;
    // rest now looks like ":<line>:<col>: <severity>: <message...>"
    let parts: Vec<&str> = rest.splitn(4, ':').collect();
    if parts.len() < 4 {
        return None;
    }
    // parts[0] is empty (the leading colon after the path).
    let line_num = parts[1].trim().parse::<u32>().ok()?;
    let col_num = parts[2].trim().parse::<u32>().ok()?;
    // parts[3] = "<severity>: <message...>" — split severity from message.
    let (severity_field, message) = parts[3].split_once(':')?;
    let severity = match severity_field.trim() {
        "warning" => DiagnosticSeverity::WARNING,
        "error" => DiagnosticSeverity::ERROR,
        _ => return None,
    };
    let message = message.trim().to_string();
    Some(RawDiag {
        // Guile is 1-based; LSP is 0-based.
        line: line_num.saturating_sub(1),
        col: col_num.saturating_sub(1),
        severity,
        message,
    })
}

/// Split `s` into `(path, ":line:col: ...")`.
///
/// A Guile source path may contain a colon (a Windows drive letter `C:`), so we
/// can't split on the first `:`. Instead we find the `:line:col:` suffix: the
/// first colon whose following two `:`-separated fields are both numbers.
fn split_path_prefix(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' && is_line_col_suffix(&s[i + 1..]) {
            return Some((&s[..i], &s[i..]));
        }
    }
    None
}

/// True if `s` starts with `<digits>:<digits>:` (the line and column fields).
fn is_line_col_suffix(s: &str) -> bool {
    let b = s.as_bytes();
    let mut j = 0;
    let n1 = take_digits(b, j);
    if n1 == 0 || j + n1 >= b.len() || b[j + n1] != b':' {
        return false;
    }
    j += n1 + 1;
    let n2 = take_digits(b, j);
    n2 > 0 && j + n2 < b.len() && b[j + n2] == b':'
}

/// Count leading ASCII digits of `b` at offset `from`.
fn take_digits(b: &[u8], from: usize) -> usize {
    let mut n = 0;
    while from + n < b.len() && b[from + n].is_ascii_digit() {
        n += 1;
    }
    n
}

/// Build LSP diagnostics for a document from Guile's raw compiler warning
/// output plus an optional compile-error message. Each diagnostic's range
/// spans the identifier at the reported (0-based) line/column.
pub fn build_diagnostics(text: &str, warnings: &str, error: Option<&str>) -> Vec<Diagnostic> {
    let mut raws = parse_compile_output(warnings);
    if let Some(e) = error.and_then(parse_error_output) {
        raws.push(e);
    }
    raws.into_iter()
        .map(|rd| {
            let start = Position {
                line: rd.line,
                character: rd.col,
            };
            let end_offset = identifier_end_offset(text, rd.line, rd.col);
            let end = offset_to_position(text, end_offset);
            Diagnostic {
                range: Range { start, end },
                severity: Some(rd.severity),
                source: Some("guile".to_string()),
                message: rd.message,
                ..Default::default()
            }
        })
        .collect()
}

/// Byte offset just past the identifier that starts at `(line, col)` in `text`.
/// Identifier characters are anything that is not whitespace or a Scheme
/// delimiter `(` `)` `"` `;`.
fn identifier_end_offset(text: &str, line: u32, col: u32) -> usize {
    let start = position_to_offset(
        text,
        &Position {
            line,
            character: col,
        },
    );
    let bytes = text.as_bytes();
    let mut end = start;
    while end < bytes.len() {
        let b = bytes[end];
        if b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'"' | b';') {
            break;
        }
        end += 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unbound_variable_warning() {
        let raw = ";;; /tmp/x.scm:2:2: warning: possibly unbound variable `foo'\n";
        let diags = parse_compile_output(raw);
        assert_eq!(diags.len(), 1);
        assert_eq!(
            diags[0],
            RawDiag {
                line: 1,
                col: 1,
                severity: DiagnosticSeverity::WARNING,
                message: "possibly unbound variable `foo'".to_string(),
            }
        );
    }

    #[test]
    fn parses_multiple_lines() {
        let raw = "\
;;; /x.scm:1:1: warning: a
;;; /x.scm:3:8: warning: b
;;; /x.scm:5:0: error: c
";
        let diags = parse_compile_output(raw);
        assert_eq!(diags.len(), 3);
        assert_eq!(diags[0].line, 0);
        assert_eq!(diags[1].line, 2);
        assert_eq!(diags[1].col, 7);
        assert_eq!(diags[2].severity, DiagnosticSeverity::ERROR);
        assert_eq!(diags[2].line, 4);
    }

    #[test]
    fn ignores_non_diagnostic_lines() {
        let raw = "\
;;; compiling /x.scm
;;; compiled /home/u/.cache/.../x.scm.go
;;; /x.scm:1:1: warning: real warning
";
        let diags = parse_compile_output(raw);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].message, "real warning");
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_compile_output("").is_empty());
    }

    #[test]
    fn build_diagnostics_ranges_span_the_identifier() {
        let text = "(define (broken)\n  undefined-symbol-here)\n";
        // Guile reports col 3 (1-based) on line 2 (1-based) -> 0-based (1, 2).
        let raw = ";;; /x.scm:2:3: warning: possibly unbound variable `undefined-symbol-here'\n";
        let diags = build_diagnostics(text, raw, None);
        assert_eq!(diags.len(), 1);
        let d = &diags[0];
        assert_eq!(d.range.start.line, 1);
        assert_eq!(d.range.start.character, 2);
        // The identifier `undefined-symbol-here` (21 chars) starts at col 2.
        assert_eq!(d.range.end.line, 1);
        assert_eq!(d.range.end.character, 2 + 21);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(d.source.as_deref(), Some("guile"));
        assert!(d.message.contains("unbound"));
    }

    #[test]
    fn build_diagnostics_empty_when_no_warnings() {
        let diags = build_diagnostics("(display 1)", "", None);
        assert!(diags.is_empty());
    }

    #[test]
    fn parse_error_output_handles_unbalanced_parens() {
        let raw = "/x.scm:3:1: unexpected end of input while searching for: )";
        let d = parse_error_output(raw).expect("parsed");
        assert_eq!(d.line, 2);
        assert_eq!(d.col, 0);
        assert_eq!(d.severity, DiagnosticSeverity::ERROR);
        assert!(d.message.starts_with("unexpected end of input"));
    }

    #[test]
    fn build_diagnostics_includes_compile_error() {
        let text = "(define (f)\n  (display \"hi\")\n";
        let err = "/x.scm:3:1: unexpected end of input while searching for: )";
        let diags = build_diagnostics(text, "", Some(err));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].range.start.line, 2);
    }
}
