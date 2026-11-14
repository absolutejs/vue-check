//! What Vue's language tools read from a `<style>` block: a port of the
//! `vue-style-css` plugin (`plugins/vue-style-css.ts`, 3.3.14).
//!
//! The original scans the CSS with regular expressions, several of them with
//! lookbehinds. Each one is a function here, named after it.

use crate::js;

/// A piece of a style block, and where it is in the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleText {
    pub text: String,
    pub offset: usize,
}

/// `compileSFCStyle`'s result, without its imports (see `scan`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleScan {
    /// The expressions of `v-bind(...)`.
    pub bindings: Vec<StyleText>,
    /// The class names of the selectors, each with its dot.
    pub class_names: Vec<StyleText>,
    /// Set when the original's result can't be reproduced: why.
    pub unsupported: Option<&'static str>,
}

/// Blanks a range, keeping every offset after it: one space per byte. (The
/// original writes one space per UTF-16 unit, which keeps its offsets.)
fn blank(css: &mut [u8], start: usize, end: usize) {
    if let Some(range) = css.get_mut(start..end) {
        range.fill(b' ');
    }
}

/// `commentRE`: `/(?<=\/\*)[\s\S]*?(?=\*\/)|(?<=\/\/)[\s\S]*?(?=\n)/g`, each
/// match blanked. What is after a `/*` up to the next `*/`, and what is after
/// a `//` up to the end of its line, wherever the `//` is.
fn blank_comments(css: &str) -> String {
    let mut blanked = css.as_bytes().to_vec();
    let bytes = css.as_bytes();
    let find = |needle: &[u8], from: usize| {
        bytes
            .get(from..)
            .and_then(|rest| {
                rest.windows(needle.len())
                    .position(|window| window == needle)
            })
            .map(|found| found + from)
    };
    let mut position = 0;
    while position <= bytes.len() {
        let before = bytes
            .get(position.saturating_sub(2)..position)
            .unwrap_or(&[]);
        let end = match before {
            b"/*" => find(b"*/", position),
            b"//" => find(b"\n", position),
            _ => None,
        };
        match end {
            // An empty match moves on by one character, like any other.
            Some(end) if end > position => {
                blank(&mut blanked, position, end);
                position = end;
            }
            _ => position += 1,
        }
    }
    // Only whole characters were blanked: a match ends before an ASCII one.
    String::from_utf8(blanked).unwrap_or_else(|_| css.to_owned())
}

/// `fragmentRE`: `/(?<={)[^{]*(?=(?<!\\);)/g`, each match blanked. After a
/// `{`, everything up to the last `;` before the next `{` that has no
/// backslash before it: the declarations, which have no class names.
fn blank_fragments(css: &str) -> String {
    let mut blanked = css.as_bytes().to_vec();
    let bytes = css.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        if position == 0 || bytes.get(position - 1) != Some(&b'{') {
            position += 1;
            continue;
        }
        let run_end = bytes
            .get(position..)
            .and_then(|rest| rest.iter().position(|&byte| byte == b'{'))
            .map_or(bytes.len(), |found| found + position);
        let last_semicolon = (position..run_end).rev().find(|&index| {
            bytes.get(index) == Some(&b';') && (index == 0 || bytes.get(index - 1) != Some(&b'\\'))
        });
        match last_semicolon {
            Some(end) if end > position => {
                blank(&mut blanked, position, end);
                position = end;
            }
            _ => position += 1,
        }
    }
    String::from_utf8(blanked).unwrap_or_else(|_| css.to_owned())
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `cssClassNameRE`: `/\.[a-z_][-\w]*(?=[\s.,+~>:#)[{])/gi`.
fn class_names(css: &str) -> Vec<StyleText> {
    let bytes = css.as_bytes();
    let mut found = Vec::new();
    let mut position = 0;
    while let Some(&byte) = bytes.get(position) {
        let starts = byte == b'.'
            && bytes
                .get(position + 1)
                .is_some_and(|&next| next.is_ascii_alphabetic() || next == b'_');
        if !starts {
            position += 1;
            continue;
        }
        let end = bytes
            .get(position + 2..)
            .and_then(|rest| {
                rest.iter()
                    .position(|&byte| !(is_word(byte) || byte == b'-'))
            })
            .map_or(bytes.len(), |run| position + 2 + run);
        // What follows the name has to end a selector's class.
        let follows = css.get(end..).and_then(|rest| rest.chars().next());
        if follows.is_some_and(|char| js::is_whitespace(char) || ".,+~>:#)[{".contains(char)) {
            found.push(StyleText {
                text: css.get(position..end).unwrap_or("").to_owned(),
                offset: position,
            });
            position = end;
        } else {
            position += 1;
        }
    }
    found
}

/// `lexBinding`: where the `)` of a `v-bind(` is, past the parentheses and
/// strings inside it.
fn lex_binding(css: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    let mut depth = 0_usize;
    for (index, char) in css.get(start..)?.char_indices() {
        match (quote, char) {
            (None, '\'' | '"') => quote = Some(char),
            (None, '(') => depth += 1,
            (None, ')') => match depth.checked_sub(1) {
                Some(less) => depth = less,
                None => return Some(start + index),
            },
            (Some(open), close) if open == close => quote = None,
            _ => {}
        }
    }
    None
}

/// `trimQuotes(text, offset, false)`: an expression in quotes, without them.
fn trim_quotes(text: &str, offset: usize) -> StyleText {
    let mut start = 0;
    let mut end = text.len();
    if text.contains('"') || text.contains('\'') {
        let trimmed = js::trim_start(text);
        start = text.len() - trimmed.len();
        end = start + js::trim_end(trimmed).len();
        if trimmed.is_empty() {
            // The original's end runs past the start here; the text is empty.
            end = start;
        }
    }
    let inner = text.get(start..end).unwrap_or("");
    let quoted =
        |quote: char| inner.len() >= 2 && inner.starts_with(quote) && inner.ends_with(quote);
    if quoted('"') || quoted('\'') {
        start += 1;
        end -= 1;
    } else if inner == "\"" || inner == "'" {
        // One quote is both the first and the last character.
        start += 1;
        end = start;
    }
    StyleText {
        text: text.get(start..end).unwrap_or("").to_owned(),
        offset: offset + start,
    }
}

/// `compileSFCStyle`: a style block's `v-bind()` expressions and class names.
pub fn scan(css: &str) -> StyleScan {
    let mut result = StyleScan::default();
    let without_comments = blank_comments(css);

    // `cssBindingRE`: `/\bv-bind\s*\(/g`
    let bytes = without_comments.as_bytes();
    let mut search = 0;
    while let Some(found) = without_comments
        .get(search..)
        .and_then(|rest| rest.find("v-bind"))
    {
        let at = search + found;
        search = at + "v-bind".len();
        if at > 0 && bytes.get(at - 1).is_some_and(|&before| is_word(before)) {
            continue;
        }
        let after = without_comments.get(search..).unwrap_or("");
        let Some(rest) = js::trim_start(after).strip_prefix('(') else {
            continue;
        };
        let start = without_comments.len() - rest.len();
        search = start;
        let Some(end) = lex_binding(&without_comments, start) else {
            continue;
        };
        let text = without_comments.get(start..end).unwrap_or("");
        // A comment inside the expression was blanked one space per byte,
        // where the original has one per UTF-16 unit.
        if css.get(start..end) != Some(text) && !css.get(start..end).is_some_and(str::is_ascii) {
            result.unsupported =
                Some("a v-bind() in a style has a comment with a character outside ASCII in it");
        }
        result.bindings.push(trim_quotes(text, start));
    }

    result.class_names = class_names(&blank_fragments(&without_comments));
    result
}

#[cfg(test)]
mod tests {
    use super::scan;

    fn names(css: &str) -> Vec<(String, usize)> {
        scan(css)
            .class_names
            .into_iter()
            .map(|name| (name.text, name.offset))
            .collect()
    }

    #[test]
    fn class_names_of_selectors() {
        assert_eq!(names(".a { color: red; }"), [(".a".to_owned(), 0)]);
        assert_eq!(names(".a.b-c, .d_e > .f:hover {}").len(), 4);
        // A declaration's `.5em` and a comment's `.x` aren't class names.
        assert_eq!(names(".a { margin: .5em; } /* .x { */ .b{}").len(), 2);
        // A name at the very end has nothing after it to end it.
        assert_eq!(names(".a"), []);
    }

    #[test]
    fn bindings() {
        let found =
            scan(".a { color: v-bind(color); width: v-bind('size.width'); top: v-bind( fn(1) ) }");
        let texts: Vec<_> = found
            .bindings
            .iter()
            .map(|binding| binding.text.as_str())
            .collect();
        assert_eq!(texts, ["color", "size.width", " fn(1) "]);
        assert_eq!(
            found.bindings.first().map(|binding| binding.offset),
            Some(19)
        );
        assert_eq!(scan("/* v-bind(a) */ .b { x: xv-bind(c) }").bindings, []);
    }
}
