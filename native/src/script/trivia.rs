//! TypeScript's rules for the whitespace and comments around a node, which
//! `@vue/language-core` reads through `ts.getLeadingCommentRanges` and a
//! node's `pos`. Ported from TypeScript's `scanner.ts`.

use oxc_ast::Comment;

use super::at;

/// A comment before a node, as `ts.CommentRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentRange {
    pub start: usize,
    pub end: usize,
    /// `/* */`, as opposed to `//`.
    pub is_multi_line: bool,
}

/// `isLineBreak`.
fn is_line_break(char: char) -> bool {
    matches!(char, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `isWhiteSpaceLike`: whitespace on one line, or a line break.
fn is_white_space_like(char: char) -> bool {
    matches!(
        char,
        ' ' | '\t' | '\u{b}' | '\u{c}' | '\u{a0}' | '\u{85}' | '\u{1680}' | '\u{2000}'
            ..='\u{200b}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    ) || is_line_break(char)
}

fn char_at(text: &str, position: usize) -> Option<char> {
    text.get(position..)?.chars().next()
}

/// `ts.getLeadingCommentRanges(text, pos)`: the comments between `position`
/// and the next token that belong to that token. A comment on the line
/// `position` is on belongs to what came before it, unless `position` is the
/// start of the text.
pub fn leading_comment_ranges(text: &str, position: usize) -> Vec<CommentRange> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut position = position;
    let mut collecting = false;
    if position == 0 {
        collecting = true;
        // A shebang line isn't a comment.
        if text.starts_with("#!") {
            position = text.find(is_line_break).unwrap_or(text.len());
        }
    }
    while let Some(char) = char_at(text, position) {
        match char {
            '\r' | '\n' => {
                position += 1;
                if char == '\r' && bytes.get(position) == Some(&b'\n') {
                    position += 1;
                }
                collecting = true;
            }
            '\t' | '\u{b}' | '\u{c}' | ' ' => position += 1,
            '/' => {
                let next = bytes.get(position + 1).copied();
                if next != Some(b'/') && next != Some(b'*') {
                    break;
                }
                let start = position;
                position += 2;
                if next == Some(b'/') {
                    while let Some(char) = char_at(text, position) {
                        if is_line_break(char) {
                            break;
                        }
                        position += char.len_utf8();
                    }
                } else {
                    // To the `*/`, or the end of the text.
                    position = text
                        .get(position..)
                        .and_then(|rest| rest.find("*/"))
                        .map_or(text.len(), |end| position + end + 2);
                }
                if collecting {
                    ranges.push(CommentRange {
                        start,
                        end: position,
                        is_multi_line: next == Some(b'*'),
                    });
                }
            }
            other if !other.is_ascii() && is_white_space_like(other) => {
                position += other.len_utf8();
            }
            _ => break,
        }
    }
    ranges
}

/// A node's `pos`: where the whitespace and comments before it start, which
/// is where the token before it ends. `start` is where the node's own first
/// token starts.
pub fn full_start(text: &str, comments: &[Comment], start: usize) -> usize {
    let mut position = start;
    loop {
        let before = text.get(..position).unwrap_or("");
        let trimmed = before.trim_end_matches(is_white_space_like).len();
        // The comments are in source order; one can only end here.
        let comment = comments
            .binary_search_by_key(&trimmed, |comment| at(comment.span.end))
            .ok()
            .and_then(|index| comments.get(index));
        match comment {
            Some(comment) => position = at(comment.span.start),
            None => return trimmed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::leading_comment_ranges;

    fn ranges(text: &str, position: usize) -> Vec<&str> {
        leading_comment_ranges(text, position)
            .iter()
            .map(|range| text.get(range.start..range.end).unwrap_or(""))
            .collect()
    }

    #[test]
    fn comments_before_a_token() {
        assert_eq!(ranges("// a\n/* b */ x", 0), ["// a", "/* b */"]);
        assert_eq!(ranges("/* a */ x", 0), ["/* a */"]);
        assert_eq!(ranges("#!/bin/x\n// a\nx", 0), ["// a"]);
        assert_eq!(ranges("x", 0), [""; 0]);
    }

    #[test]
    fn a_comment_on_the_previous_tokens_line_is_not_leading() {
        let text = "a; /* a's */ // also a's\n/* b's */ b";
        assert_eq!(ranges(text, 2), ["/* b's */"]);
        assert_eq!(ranges("/* unterminated", 0), ["/* unterminated"]);
    }
}
