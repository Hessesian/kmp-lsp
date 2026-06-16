use tower_lsp::lsp_types::{FormattingOptions, Position, Range, TextEdit};

pub(crate) fn compute_on_type_formatting(
    lines: &[String],
    position: Position,
    ch: &str,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    match ch {
        "\n" => on_newline(lines, position, options),
        _ => None,
    }
}

fn on_newline(
    lines: &[String],
    position: Position,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    let cursor_line = position.line as usize;
    if cursor_line == 0 {
        return None;
    }

    let prev_line = lines.get(cursor_line - 1)?;
    let prev_trimmed = prev_line.trim_end();
    let indent = leading_whitespace(prev_line);
    let tab = if options.insert_spaces {
        " ".repeat(options.tab_size as usize)
    } else {
        "\t".to_string()
    };

    if prev_trimmed.ends_with('{') && !is_in_string_context(prev_trimmed) {
        return brace_newline(lines, cursor_line, indent, &tab);
    }

    if let Some(prefix) = comment_continuation_prefix(prev_line.trim()) {
        return comment_newline(lines, cursor_line, indent, prefix);
    }

    None
}

fn brace_newline(
    lines: &[String],
    cursor_line: usize,
    indent: &str,
    tab: &str,
) -> Option<Vec<TextEdit>> {
    let inner_indent = format!("{indent}{tab}");
    let cur_line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
    let cur_trimmed = cur_line.trim_start();
    let cur_indent_len = cur_line.len() - cur_trimmed.len();

    // Auto-pair inserted `}` on this line — split it into a properly indented block.
    if let Some(after_brace) = cur_trimmed.strip_prefix('}') {
        let after_close = after_brace.trim_start();
        let closing = format!("{indent}}}");
        let new_text = if after_close.is_empty() {
            format!("{inner_indent}\n{closing}")
        } else {
            format!("{inner_indent}\n{closing} {after_close}")
        };
        return Some(vec![text_edit(
            cursor_line as u32,
            0,
            cursor_line as u32,
            cur_line.len() as u32,
            new_text,
        )]);
    }

    // Cursor is on a blank line. `}` may have landed on the next line without
    // indentation (Helix auto-indent path: blank inner line + unindented `}`).
    if cur_trimmed.is_empty() {
        if let Some(next_line) = lines.get(cursor_line + 1) {
            let next_trimmed = next_line.trim_start();
            if next_trimmed.starts_with('}') {
                let next_indent_len = next_line.len() - next_trimmed.len();
                let mut edits = Vec::new();
                if cur_indent_len != inner_indent.len() {
                    edits.push(text_edit(
                        cursor_line as u32,
                        0,
                        cursor_line as u32,
                        cur_indent_len as u32,
                        inner_indent,
                    ));
                }
                if next_indent_len != indent.len() {
                    edits.push(text_edit(
                        (cursor_line + 1) as u32,
                        0,
                        (cursor_line + 1) as u32,
                        next_indent_len as u32,
                        indent.to_string(),
                    ));
                }
                return if edits.is_empty() { None } else { Some(edits) };
            }
        }
    }

    // No closing brace visible — fix cursor line indentation if wrong.
    if cur_indent_len != inner_indent.len() {
        return Some(vec![text_edit(
            cursor_line as u32,
            0,
            cursor_line as u32,
            cur_indent_len as u32,
            inner_indent,
        )]);
    }

    None
}

fn comment_newline(
    lines: &[String],
    cursor_line: usize,
    indent: &str,
    prefix: &str,
) -> Option<Vec<TextEdit>> {
    let cur_line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
    // Skip if editor already inserted the right prefix (e.g. editor has its own comment continuation).
    if cur_line.trim_start().starts_with(prefix) {
        return None;
    }
    let cur_indent_len = cur_line.len() - cur_line.trim_start().len();
    Some(vec![text_edit(
        cursor_line as u32,
        0,
        cursor_line as u32,
        cur_indent_len as u32,
        format!("{indent}{prefix} "),
    )])
}

/// Returns the comment prefix to continue onto the next line, if the current
/// (trimmed) line is inside a comment that should be continued.
fn comment_continuation_prefix(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("///") {
        return Some("///");
    }
    if trimmed.starts_with("//!") {
        return Some("//!");
    }
    if trimmed.starts_with("/**") {
        return Some(" *");
    }
    // Inside an existing block-comment body (` * text` or bare `*`)
    if trimmed.starts_with("* ") || trimmed == "*" {
        return Some("*");
    }
    // Plain `//` line comments — only when the whole line is a comment, not an
    // end-of-line comment tacked onto code (those shouldn't be continued).
    if trimmed.starts_with("// ") || trimmed == "//" {
        return Some("//");
    }
    None
}

/// Heuristic: returns true if the `{` at end of the line is inside a string
/// literal (odd number of unescaped `"` before it).
fn is_in_string_context(line: &str) -> bool {
    let mut in_string = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => in_string = !in_string,
            '\\' if in_string => {
                chars.next();
            }
            _ => {}
        }
    }
    in_string
}

fn leading_whitespace(s: &str) -> &str {
    let trimmed = s.trim_start_matches([' ', '\t']);
    &s[..s.len() - trimmed.len()]
}

fn text_edit(
    start_line: u32,
    start_char: u32,
    end_line: u32,
    end_char: u32,
    new_text: String,
) -> TextEdit {
    TextEdit {
        range: Range::new(
            Position::new(start_line, start_char),
            Position::new(end_line, end_char),
        ),
        new_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(spaces: bool, tab_size: u32) -> FormattingOptions {
        FormattingOptions {
            tab_size,
            insert_spaces: spaces,
            ..Default::default()
        }
    }

    // Split by `\n` (not str::lines) so trailing empty lines are preserved,
    // matching what the indexer sees after a newline is inserted.
    fn lines(src: &str) -> Vec<String> {
        src.split('\n').map(str::to_owned).collect()
    }

    fn fmt(src: &str, cursor_line: u32) -> Option<Vec<TextEdit>> {
        compute_on_type_formatting(
            &lines(src),
            Position::new(cursor_line, 0),
            "\n",
            &opts(true, 4),
        )
    }

    fn apply(src: &str, edits: Vec<TextEdit>) -> String {
        let mut ls = lines(src);
        for edit in edits {
            let sl = edit.range.start.line as usize;
            let sc = edit.range.start.character as usize;
            let el = edit.range.end.line as usize;
            let ec = edit.range.end.character as usize;
            assert_eq!(sl, el, "multi-line edits not supported in test helper");
            // Extend with empty lines if the edit targets a line not yet in ls.
            while ls.len() <= sl {
                ls.push(String::new());
            }
            let line = ls[sl].clone();
            ls[sl] = format!("{}{}{}", &line[..sc], edit.new_text, &line[ec..]);
        }
        ls.join("\n")
    }

    #[test]
    fn brace_newline_fixes_wrong_indent() {
        // Editor auto-indented by 2 but we want 4.
        let src = "fun foo() {\n  ";
        let edits = fmt(src, 1).expect("should produce edits");
        let result = apply(src, edits);
        assert_eq!(result, "fun foo() {\n    ");
    }

    #[test]
    fn brace_newline_no_op_when_correct() {
        let src = "fun foo() {\n    ";
        assert!(
            fmt(src, 1).is_none(),
            "indent already correct — should be no-op"
        );
    }

    #[test]
    fn brace_newline_splits_autopair_close() {
        // Helix or VS Code auto-pair: `}` landed on cursor line.
        let src = "fun foo() {\n}";
        let edits = fmt(src, 1).expect("should split brace pair");
        let result = apply(src, edits);
        assert_eq!(result, "fun foo() {\n    \n}");
    }

    #[test]
    fn brace_newline_splits_autopair_close_nested() {
        let src = "    if (x) {\n    }";
        let edits = fmt(src, 1).expect("should split");
        let result = apply(src, edits);
        assert_eq!(result, "    if (x) {\n        \n    }");
    }

    #[test]
    fn brace_in_string_is_ignored() {
        // The `{` is inside a string — should not trigger brace assist.
        let src = "    val s = \"{\"\n";
        assert!(fmt(src, 1).is_none());
    }

    #[test]
    fn doc_comment_triple_slash_continued() {
        let src = "/// Hello\n";
        let edits = fmt(src, 1).expect("should continue doc comment");
        let result = apply(src, edits);
        assert_eq!(result, "/// Hello\n/// ");
    }

    #[test]
    fn doc_comment_bang_continued() {
        let src = "//! Module doc\n";
        let edits = fmt(src, 1).expect("should continue //! comment");
        let result = apply(src, edits);
        assert_eq!(result, "//! Module doc\n//! ");
    }

    #[test]
    fn line_comment_continued() {
        let src = "    // some comment\n    ";
        let edits = fmt(src, 1).expect("should continue line comment");
        let result = apply(src, edits);
        assert_eq!(result, "    // some comment\n    // ");
    }

    #[test]
    fn block_comment_star_continued() {
        let src = "    /**\n    ";
        let edits = fmt(src, 1).expect("should continue block comment");
        let result = apply(src, edits);
        assert_eq!(result, "    /**\n     * ");
    }

    #[test]
    fn block_comment_body_continued() {
        let src = "    * some text\n    ";
        let edits = fmt(src, 1).expect("should continue block comment body");
        let result = apply(src, edits);
        assert_eq!(result, "    * some text\n    * ");
    }

    #[test]
    fn brace_newline_helix_blank_line_with_close_below() {
        // Helix auto-indent path: cursor lands on a blank inner-indented line,
        // `}` moves to the next line with no indentation.
        let src = "    viewModelScope.launch {\n        \n}";
        let edits = fmt(src, 1).expect("should fix } indentation");
        let result = apply(src, edits);
        // cursor line already has correct inner indent → only } is moved
        assert_eq!(result, "    viewModelScope.launch {\n        \n    }");
    }

    #[test]
    fn brace_newline_helix_blank_line_wrong_indent_and_close_below() {
        // Both cursor line (wrong indent) and `}` below need fixing.
        let src = "    viewModelScope.launch {\n\n}";
        let edits = fmt(src, 1).expect("should fix both lines");
        let result = apply(src, edits);
        assert_eq!(result, "    viewModelScope.launch {\n        \n    }");
    }

    #[test]
    fn comment_already_continued_is_noop() {
        // Editor already inserted the prefix — we should not double-insert.
        let src = "/// Hello\n/// ";
        assert!(fmt(src, 1).is_none());
    }

    #[test]
    fn plain_line_no_action() {
        let src = "val x = 1\n    ";
        assert!(fmt(src, 1).is_none());
    }

    #[test]
    fn tab_indentation() {
        // Editor gave no auto-indent (empty new line). We should insert one tab.
        let src = "fun foo() {\n";
        let src_lines = lines(src);
        let edits =
            compute_on_type_formatting(&src_lines, Position::new(1, 0), "\n", &opts(false, 4));
        let edits = edits.expect("should produce edits");
        let result = apply(src, edits);
        assert_eq!(result, "fun foo() {\n\t");
    }
}
