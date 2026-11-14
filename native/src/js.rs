//! JavaScript string behavior that Rust's standard library doesn't share.
//! The code being ported is written against JavaScript's definitions, and the
//! output has to match it character for character.

/// `\s` in a JavaScript regular expression, and what `String.prototype.trim`
/// removes: ECMAScript's `WhiteSpace` and `LineTerminator`. Rust's
/// `char::is_whitespace` differs on U+0085 (not here) and U+FEFF (here).
pub fn is_whitespace(char: char) -> bool {
    matches!(
        char,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_whitespace)
}

/// `String.prototype.trimStart`.
pub fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_whitespace)
}

/// `String.prototype.trimEnd`.
pub fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_whitespace)
}

/// `String.prototype.indexOf(needle, from)`, in bytes. `from` must be a
/// character boundary; past the end, only an empty needle is found.
pub fn index_of(text: &str, needle: &str, from: usize) -> Option<usize> {
    let from = from.min(text.len());
    text.get(from..)?.find(needle).map(|found| found + from)
}

/// `@vue/shared`'s `camelize`: `/-(\w)/g`, the letter after each hyphen in
/// upper case. `\w` is ASCII only.
pub fn camelize(text: &str) -> String {
    let mut camelized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        match chars.peek() {
            Some(&next) if char == '-' && (next.is_ascii_alphanumeric() || next == '_') => {
                camelized.push(next.to_ascii_uppercase());
                chars.next();
            }
            _ => camelized.push(char),
        }
    }
    camelized
}

#[cfg(test)]
mod tests {
    use super::{camelize, index_of, trim};

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(trim("\u{feff} a\u{a0}\n"), "a");
        assert_eq!(trim("\u{85}a\u{85}"), "\u{85}a\u{85}");
    }

    #[test]
    fn index_of_from() {
        assert_eq!(index_of("a in a", "a", 1), Some(5));
        assert_eq!(index_of("abc", "", 9), Some(3));
        assert_eq!(index_of("abc", "d", 0), None);
    }

    #[test]
    fn camelizes() {
        assert_eq!(camelize("foo-bar-baz"), "fooBarBaz");
        assert_eq!(camelize("a--b"), "a-B");
        assert_eq!(camelize("-a-"), "A-");
        assert_eq!(camelize("x-1-_y-é"), "x1_y-é");
    }
}
