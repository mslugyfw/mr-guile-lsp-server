//! Source-text analysis helpers: extracting the symbol under the cursor.
//!
//! Used by completion/hover/definition/signature handlers to turn a cursor
//! `Position` into the Scheme identifier (and its range) that the user is
//! editing or inspecting.

use crate::position::{offset_to_position, position_to_offset};
use tower_lsp::lsp_types::{Position, Range};

/// A character is part of a Scheme symbol if it is not whitespace and not a
/// delimiter: `( ) [ ] { } " ; ' ` ,`
fn is_symbol_char(b: u8) -> bool {
    if b.is_ascii_whitespace() {
        return false;
    }
    !matches!(
        b,
        b'(' | b')' | b'[' | b']' | b'{' | b'}' | b'"' | b';' | b'\'' | b'`' | b','
    )
}

/// The byte offset of the start of the symbol containing `offset` (scan left).
fn symbol_start(bytes: &[u8], offset: usize) -> usize {
    let mut start = offset.min(bytes.len());
    while start > 0 && is_symbol_char(bytes[start - 1]) {
        start -= 1;
    }
    start
}

/// The byte offset just past the symbol containing `offset` (scan right).
fn symbol_end(bytes: &[u8], offset: usize) -> usize {
    let mut end = offset.min(bytes.len());
    while end < bytes.len() && is_symbol_char(bytes[end]) {
        end += 1;
    }
    end
}

/// Return the full symbol (and its range) that contains `pos`, or None if the
/// cursor is not on a symbol character.
pub fn symbol_at(text: &str, pos: &Position) -> Option<(String, Range)> {
    let bytes = text.as_bytes();
    let offset = position_to_offset(text, pos);
    if offset >= bytes.len() || !is_symbol_char(bytes[offset]) {
        return None;
    }
    let start = symbol_start(bytes, offset);
    let end = symbol_end(bytes, offset);
    if start >= end {
        return None;
    }
    let sym = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
    let range = Range {
        start: offset_to_position(text, start),
        end: offset_to_position(text, end),
    };
    Some((sym, range))
}

/// Return the symbol prefix being typed up to (but not past) `pos`, plus the
/// range from the symbol's start to `pos`. Used for completion.
pub fn symbol_prefix_at(text: &str, pos: &Position) -> Option<(String, Range)> {
    let bytes = text.as_bytes();
    let offset = position_to_offset(text, pos);
    let start = symbol_start(bytes, offset);
    if start >= offset {
        return None;
    }
    let prefix = std::str::from_utf8(&bytes[start..offset]).ok()?.to_string();
    let range = Range {
        start: offset_to_position(text, start),
        end: Position {
            line: pos.line,
            character: pos.character,
        },
    };
    Some((prefix, range))
}

/// For signature help: find the function being called around `pos` by scanning
/// back to the nearest unmatched `(` and taking the symbol just before it.
/// Returns the called symbol name. e.g. in `(foo a b|`, returns "foo".
pub fn called_symbol_before(text: &str, pos: &Position) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = position_to_offset(text, pos);
    // Walk back skipping whitespace, tracking paren depth; find the `(` that
    // opens the current call (depth 0 relative to start).
    let mut depth = 0i32;
    if i > bytes.len() {
        i = bytes.len();
    }
    while i > 0 {
        i -= 1;
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            continue;
        }
        if b == b')' {
            depth += 1;
            continue;
        }
        if b == b'(' {
            if depth == 0 {
                // This `(` opens the current call. The called symbol is right after it.
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && is_symbol_char(bytes[j]) {
                    j += 1;
                }
                if j > start {
                    return std::str::from_utf8(&bytes[start..j])
                        .ok()
                        .map(|s| s.to_string());
                }
                return None;
            }
            depth -= 1;
            continue;
        }
        // any other char inside the arg list keeps scanning (depth unchanged)
        if depth > 0 {
            continue;
        }
        // at depth 0 inside the call, keep scanning back over args
    }
    None
}

/// Definition-form keywords whose body starts with the defined name.
const DEFINE_HEADS: &[&str] = &[
    "define",
    "define*",
    "define-public",
    "define-record-type",
    "define-syntax",
    "define-macro",
    "define-method",
    "define-generic",
    "define-class",
    "define-accessors",
];

/// One top-level definition found while scanning.
pub struct DefineInfo {
    pub name: String,
    pub kind: tower_lsp::lsp_types::SymbolKind,
    /// Range covering the whole defining form (from `(` to matching `)`).
    pub range: tower_lsp::lsp_types::Range,
    /// Tighter range around just the defined name.
    pub selection_range: tower_lsp::lsp_types::Range,
}

/// Given `bytes` and the index of an opening `(`, return the index of its
/// matching `)`, skipping string literals and line comments. None if unbalanced.
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Scan `text` for top-level `(define…)` forms and return one entry each.
pub fn scan_defines(text: &str) -> Vec<DefineInfo> {
    use tower_lsp::lsp_types::{Range, SymbolKind};

    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                // skip string literal
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                continue;
            }
            b';' => {
                // line comment
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' => {
                depth += 1;
                if depth == 1 {
                    if let Some((head, head_end)) = read_token(bytes, i + 1) {
                        if DEFINE_HEADS.contains(&head.as_str()) {
                            let mut j = skip_ws(bytes, head_end);
                            let is_callable = head_end_leads_to_paren(bytes, head_end);
                            if is_callable {
                                j = skip_ws(bytes, j + 1);
                            }
                            if let Some((name, name_end)) = read_token(bytes, j) {
                                let name_start = name_end - name.len();
                                let kind = if is_callable {
                                    SymbolKind::FUNCTION
                                } else {
                                    SymbolKind::VARIABLE
                                };
                                // Full form = from this '(' to its matching ')'.
                                let form_end = matching_paren(bytes, i)
                                    .map(|close| close + 1)
                                    .unwrap_or(name_end);
                                out.push(DefineInfo {
                                    name: name.clone(),
                                    kind,
                                    range: Range {
                                        start: offset_to_position(text, i),
                                        end: offset_to_position(text, form_end),
                                    },
                                    selection_range: Range {
                                        start: offset_to_position(text, name_start),
                                        end: offset_to_position(text, name_end),
                                    },
                                });
                            }
                        }
                    }
                }
            }
            b')' if depth > 0 => {
                depth -= 1;
            }
            b')' => {}

            _ => {}
        }
        i += 1;
    }
    out
}

fn head_end_leads_to_paren(bytes: &[u8], head_end: usize) -> bool {
    let j = skip_ws(bytes, head_end);
    j < bytes.len() && bytes[j] == b'('
}

/// Find every occurrence of `symbol` as a whole identifier token in `text`,
/// skipping string literals and line comments. Used for goto-references.
pub fn find_references_in_text(text: &str, symbol: &str) -> Vec<Range> {
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                // skip string literal
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b';' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            _ if is_symbol_char(b) => {
                let start = i;
                while i < bytes.len() && is_symbol_char(bytes[i]) {
                    i += 1;
                }
                if text.get(start..i) == Some(symbol) {
                    refs.push(Range {
                        start: offset_to_position(text, start),
                        end: offset_to_position(text, i),
                    });
                }
            }
            _ => i += 1,
        }
    }
    refs
}

/// Find the (Range of the) definition of `symbol` in `text`, by scanning
/// top-level-ish `(define…)` forms. Returns the range covering the whole
/// defining form's head `(define-name` opening, precise enough for goto.
pub fn find_definition_in_text(text: &str, symbol: &str) -> Option<Range> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            // Read the head token after '('.
            if let Some((head, head_end)) = read_token(bytes, i + 1) {
                if DEFINE_HEADS.contains(&head.as_str()) {
                    // The defined name is the next token, possibly after '('.
                    let mut j = skip_ws(bytes, head_end);
                    if j < bytes.len() && bytes[j] == b'(' {
                        j = skip_ws(bytes, j + 1);
                    }
                    if let Some((name, name_end)) = read_token(bytes, j) {
                        if name == symbol {
                            let name_start = name_end - name.len();
                            let range = Range {
                                start: offset_to_position(text, i),
                                end: offset_to_position(text, name_start + name.len()),
                            };
                            return Some(range);
                        }
                    }
                }
            }
        }
        i += 1;
    }
    None
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Read a token starting at `i`; return (token, end_offset_past_token).
fn read_token(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    let start = i;
    let mut end = i;
    while end < bytes.len() && is_symbol_char(bytes[end]) {
        end += 1;
    }
    if end == start {
        return None;
    }
    let tok = std::str::from_utf8(&bytes[start..end]).ok()?.to_string();
    Some((tok, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, ch: u32) -> Position {
        Position {
            line,
            character: ch,
        }
    }

    #[test]
    fn extracts_symbol_under_cursor() {
        // "(display hello)" — cursor on the 'h' of hello (line 0, char 9).
        let text = "(display hello)";
        let (sym, range) = symbol_at(text, &pos(0, 9)).expect("symbol");
        assert_eq!(sym, "hello");
        assert_eq!(range.start, pos(0, 9));
        assert_eq!(range.end, pos(0, 14));
    }

    #[test]
    fn extracts_symbol_with_special_chars() {
        // my-func? should be one symbol.
        let text = "(call my-func?)";
        let (sym, _) = symbol_at(text, &pos(0, 8)).expect("symbol");
        assert_eq!(sym, "my-func?");
    }

    #[test]
    fn returns_none_in_whitespace() {
        let text = "(display  hello)";
        assert!(symbol_at(text, &pos(0, 8)).is_none());
    }

    #[test]
    fn extracts_partial_prefix_for_completion() {
        // "(dis" with cursor at end (col 4) -> prefix "dis".
        let text = "(disp";
        let (prefix, range) = symbol_prefix_at(text, &pos(0, 5)).expect("prefix");
        assert_eq!(prefix, "disp");
        assert_eq!(range.start, pos(0, 1));
        assert_eq!(range.end, pos(0, 5));
    }

    #[test]
    fn prefix_returns_none_at_word_boundary() {
        // Cursor right after "(" — no prefix typed yet.
        assert!(symbol_prefix_at("(display", &pos(0, 1)).is_none());
    }

    #[test]
    fn called_symbol_found_for_signature_help() {
        // Cursor right after the '(' of a call -> the called symbol is "foo".
        let text = "(foo a b)";
        assert_eq!(called_symbol_before(text, &pos(0, 4)), Some("foo".into()));
        // Cursor inside args, after nested close -> still "foo".
        let text2 = "(foo (bar 1) 2)";
        assert_eq!(called_symbol_before(text2, &pos(0, 13)), Some("foo".into()));
    }

    #[test]
    fn called_symbol_none_at_top_level() {
        // Not inside any call.
        assert!(called_symbol_before("hello world", &pos(0, 5)).is_none());
    }

    #[test]
    fn finds_function_definition() {
        let text = "(define (foo x)\n  (* x 2))\n(define bar 1)";
        let range = find_definition_in_text(text, "foo").expect("found foo");
        assert_eq!(range.start, pos(0, 0));
        // "(define (foo" — "foo" occupies cols 9..12; end is just past it.
        assert_eq!(range.end, pos(0, 12));
    }

    #[test]
    fn finds_value_definition() {
        let text = "(define (foo x) x)\n(define bar 1)";
        let range = find_definition_in_text(text, "bar").expect("found bar");
        assert_eq!(range.start, pos(1, 0));
    }

    #[test]
    fn finds_star_define() {
        let text = "(define* (baz #:key a)\n  a)";
        let range = find_definition_in_text(text, "baz").expect("found baz");
        assert_eq!(range.start, pos(0, 0));
    }

    #[test]
    fn definition_not_found_returns_none() {
        assert!(find_definition_in_text("(define (foo) 1)", "missing").is_none());
    }

    #[test]
    fn finds_define_syntax_macro() {
        let text = "(define-syntax my-when\n  (syntax-rules () ((_ b) b)))\n(my-when 1)";
        let range = find_definition_in_text(text, "my-when").expect("macro def found");
        assert_eq!(range.start, pos(0, 0));
    }

    #[test]
    fn finds_define_macro_old_form() {
        let text = "(define-macro (my-old x) x)\n(my-old 1)";
        let range = find_definition_in_text(text, "my-old").expect("macro def found");
        assert_eq!(range.start, pos(0, 0));
    }

    #[test]
    fn finds_all_references_to_symbol() {
        // foo appears as the definition name and as a call -> 2 references.
        let text = "(define (foo x) (foo x))";
        let refs = find_references_in_text(text, "foo");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn references_respect_word_boundaries() {
        // `foo` must not match inside `foobar` or `foo?`.
        let text = "(foo foobar foo?)";
        let refs = find_references_in_text(text, "foo");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn references_skip_strings_and_comments() {
        // `foo` in a comment and a string literal must not count.
        let text = "(foo)\n; foo here\n\"foo\"";
        let refs = find_references_in_text(text, "foo");
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn references_match_ranges_are_correct() {
        let text = "(bar) (bar)";
        let refs = find_references_in_text(text, "bar");
        assert_eq!(refs.len(), 2);
        // first `bar` at chars 1..4, second at chars 7..10.
        assert_eq!(refs[0].start, pos(0, 1));
        assert_eq!(refs[1].start, pos(0, 7));
    }

    #[test]
    fn scans_top_level_defines() {
        use tower_lsp::lsp_types::SymbolKind;
        let text = "(define (foo x)\n  (* x 2))\n\n(define bar 1)";
        let defs = scan_defines(text);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "foo");
        assert_eq!(defs[0].kind, SymbolKind::FUNCTION);
        assert_eq!(defs[1].name, "bar");
        assert_eq!(defs[1].kind, SymbolKind::VARIABLE);
    }

    #[test]
    fn scan_skips_nested_and_string_and_comment_defines() {
        let text =
            "(define (foo)\n  \"(define fake)\"\n  ; (define commented)\n  1)\n(define real 2)";
        let defs = scan_defines(text);
        let names: Vec<_> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["foo", "real"]);
    }

    #[test]
    fn document_symbol_range_covers_the_whole_form() {
        // `range` must span from '(' to its matching ')', not just the head.
        let text = "(define (foo x)\n  (* x 2))\n(define bar 1)";
        let defs = scan_defines(text);
        assert_eq!(defs.len(), 2);
        // foo form spans to the outer ')' (line 1, char 9); range.end is past it.
        assert_eq!(defs[0].range.start, pos(0, 0));
        assert_eq!(defs[0].range.end, pos(1, 10));
        // selection_range is just the name "foo" (starts at col 9).
        assert_eq!(defs[0].selection_range.start, pos(0, 9));
        // bar form: "(define bar 1)" on line 2.
        assert_eq!(defs[1].range.start, pos(2, 0));
        assert_eq!(defs[1].range.end, pos(2, 14));
    }

    #[test]
    fn symbol_prefix_at_extracts_typed_prefix() {
        // Cursor right after typing "my" inside "(my": prefix should be "my".
        let text = "(define (foo x)\n  (* x 2))\n(my\n";
        // line 2 = "(my", cursor at char 3 (just after 'y')
        let (prefix, _range) = symbol_prefix_at(text, &pos(2, 3)).expect("prefix");
        assert_eq!(prefix, "my");
    }

    #[test]
    fn symbol_prefix_at_returns_none_on_whitespace() {
        // Cursor on whitespace/empty line yields no prefix (no completion there).
        let text = "(define (foo x)\n  x)\n";
        // line 0 char 0 is '(', not a symbol char -> None
        assert!(symbol_prefix_at(text, &pos(0, 0)).is_none());
        // line 1 is "  x)" -> leading spaces at char 1 is whitespace -> None
        assert!(symbol_prefix_at(text, &pos(1, 1)).is_none());
    }

    #[test]
    fn symbol_prefix_at_full_symbol_when_cursor_at_end() {
        // Cursor at the end of a whole symbol returns the whole symbol.
        let text = "(display\n";
        // "display" spans chars 1..8; cursor at char 8 (right after 'y')
        let (prefix, _range) = symbol_prefix_at(text, &pos(0, 8)).expect("prefix");
        assert_eq!(prefix, "display");
    }
}
