//! TypeScript's scanner (`scanner.ts`, `createScanner`), as far as
//! `preProcessFile` drives it: the standard language variant, trivia skipped,
//! no error reporting. Every token ends where TypeScript's ends, and a string,
//! a template or an identifier has TypeScript's value.
//!
//! The original counts UTF-16 code units and this counts UTF-8 bytes. Where
//! the original looks a fixed number of units ahead it is looking for ASCII,
//! which is as many bytes as units.

use std::borrow::Cow;

use super::tables::{IDENTIFIER_PART, IDENTIFIER_START, KEYWORDS};

/// The keywords `preProcessFile` tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyword {
    As,
    Declare,
    Export,
    From,
    Import,
    Module,
    Require,
    Type,
    Other,
}

/// `SyntaxKind`, with the tokens `preProcessFile` never asks about as `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    EndOfFile,
    Unknown,
    NonTextFileMarker,
    StringLiteral,
    NoSubstitutionTemplateLiteral,
    TemplateHead,
    TemplateMiddle,
    TemplateTail,
    OpenBrace,
    CloseBrace,
    OpenParen,
    OpenBracket,
    CloseBracket,
    Dot,
    Comma,
    Asterisk,
    Equals,
    Identifier,
    PrivateIdentifier,
    Keyword(Keyword),
    Other,
}

/// `tokenValue`.
#[derive(Clone, Debug)]
pub enum Value<'a> {
    /// The token has no value anything reads.
    None,
    Text(Cow<'a, str>),
    /// The value has half a surrogate pair (`'\uD800'`), which a JavaScript
    /// string can hold and a Rust one can't.
    LoneSurrogate,
}

/// What an escape sequence stands for.
enum Escape {
    /// A UTF-16 code unit or a code point.
    Code(u32),
    /// Nothing (a line continuation).
    Empty,
    /// The text of the escape itself, from this offset to the position.
    Raw(usize),
}

fn in_ranges(code: u32, ranges: &[u32]) -> bool {
    let index = ranges.partition_point(|&bound| bound < code);
    // An odd index is inside a range; an even one is a range's start, or between two.
    index % 2 == 1 || ranges.get(index) == Some(&code)
}

fn is_ascii_letter(code: u32) -> bool {
    (0x41..=0x5a).contains(&code) || (0x61..=0x7a).contains(&code)
}

/// `isIdentifierStart`, for the latest language version.
fn is_identifier_start(code: u32) -> bool {
    is_ascii_letter(code)
        || code == 0x24
        || code == 0x5f
        || (code > 127 && in_ranges(code, IDENTIFIER_START))
}

/// `isIdentifierPart`, for the latest language version.
fn is_identifier_part(code: u32) -> bool {
    is_ascii_letter(code)
        || (0x30..=0x39).contains(&code)
        || code == 0x24
        || code == 0x5f
        || (code > 127 && in_ranges(code, IDENTIFIER_PART))
}

/// `isWhiteSpaceSingleLine`.
fn is_white_space_single_line(char: char) -> bool {
    matches!(
        char,
        ' ' | '\t'
            | '\u{b}'
            | '\u{c}'
            | '\u{a0}'
            | '\u{85}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200b}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// `isLineBreak`.
fn is_line_break(char: char) -> bool {
    matches!(char, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn hex_value(byte: u8) -> Option<u32> {
    char::from(byte).to_digit(16)
}

fn push_code(units: &mut Vec<u16>, code: u32) {
    match char::from_u32(code) {
        Some(char) => {
            let mut buffer = [0; 2];
            units.extend_from_slice(char.encode_utf16(&mut buffer));
        }
        // Half a surrogate pair: a code unit of its own.
        None => units.push(u16::try_from(code).unwrap_or(0xfffd)),
    }
}

#[derive(Clone)]
pub struct Scanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    token_start: usize,
    token: Token,
    value: Value<'a>,
}

impl<'a> Scanner<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            token_start: 0,
            token: Token::Unknown,
            value: Value::None,
        }
    }

    /// `getToken`.
    pub fn token(&self) -> Token {
        self.token
    }

    /// `getTokenValue`.
    pub fn value(&self) -> &Value<'a> {
        &self.value
    }

    fn byte(&self, at: usize) -> Option<u8> {
        self.bytes.get(at).copied()
    }

    fn is_byte(&self, at: usize, byte: u8) -> bool {
        self.byte(at) == Some(byte)
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text.get(at..)?.chars().next()
    }

    fn slice(&self, start: usize, end: usize) -> &'a str {
        self.text.get(start..end).unwrap_or("")
    }

    /// The length of the line break at an offset, if one is there.
    fn line_break_at(&self, at: usize) -> Option<usize> {
        self.char_at(at)
            .filter(|&char| is_line_break(char))
            .map(char::len_utf8)
    }

    fn is_digit_at(&self, at: usize) -> bool {
        self.byte(at).is_some_and(|byte| byte.is_ascii_digit())
    }

    fn set(&mut self, token: Token) -> Token {
        self.token = token;
        token
    }

    /// A token of `length` bytes.
    fn punctuation(&mut self, length: usize, token: Token) -> Token {
        self.pos += length;
        self.set(token)
    }

    /// `lookAhead(() => scan())`: the token after this one, without moving.
    pub fn look_ahead(&self) -> Token {
        self.clone().scan()
    }

    /// `scanNumberFragment`.
    fn scan_number_fragment(&mut self) {
        while self
            .byte(self.pos)
            .is_some_and(|byte| byte == b'_' || byte.is_ascii_digit())
        {
            self.pos += 1;
        }
    }

    /// `scanNumber`.
    fn scan_number(&mut self) {
        let mut leading_zero = false;
        if self.is_byte(self.pos, b'0') {
            self.pos += 1;
            if self.is_byte(self.pos, b'_') {
                self.pos -= 1;
                self.scan_number_fragment();
            } else {
                let start = self.pos;
                let mut octal = true;
                while let Some(digit) = self.byte(self.pos).filter(u8::is_ascii_digit) {
                    octal &= digit <= b'7';
                    self.pos += 1;
                }
                if !octal {
                    leading_zero = true;
                } else if self.pos > start {
                    // An octal literal (`017`) ends here.
                    return;
                }
            }
        } else {
            self.scan_number_fragment();
        }
        let mut fraction_or_exponent = false;
        if self.is_byte(self.pos, b'.') {
            self.pos += 1;
            self.scan_number_fragment();
            fraction_or_exponent = true;
        }
        if self.is_byte(self.pos, b'E') || self.is_byte(self.pos, b'e') {
            self.pos += 1;
            if self.is_byte(self.pos, b'+') || self.is_byte(self.pos, b'-') {
                self.pos += 1;
            }
            self.scan_number_fragment();
            fraction_or_exponent = true;
        }
        if leading_zero {
            return;
        }
        if !fraction_or_exponent {
            self.check_big_int_suffix();
        }
        self.check_for_identifier_start_after_numeric_literal();
    }

    /// `checkBigIntSuffix`.
    fn check_big_int_suffix(&mut self) {
        if self.is_byte(self.pos, b'n') {
            self.pos += 1;
        }
    }

    /// `checkForIdentifierStartAfterNumericLiteral`: an `n` on its own after
    /// the number is taken with it; anything else is left.
    fn check_for_identifier_start_after_numeric_literal(&mut self) {
        if !self
            .char_at(self.pos)
            .is_some_and(|char| is_identifier_start(u32::from(char)))
        {
            return;
        }
        let start = self.pos;
        let parts = self.scan_identifier_parts();
        if !(parts.encode_utf16().count() == 1 && self.is_byte(start, b'n')) {
            self.pos = start;
        }
    }

    /// `scanMinimumNumberOfHexDigits(1, canHaveSeparators)`: the run of hex
    /// digits here, as its value. `None` when there is none; a value beyond a
    /// code point is `u32::MAX`.
    fn scan_hex_digits(&mut self, separators: bool) -> Option<u32> {
        let mut value: Option<u32> = None;
        while let Some(byte) = self.byte(self.pos) {
            if separators && byte == b'_' {
                self.pos += 1;
                continue;
            }
            let Some(digit) = hex_value(byte) else {
                break;
            };
            value = Some(
                value
                    .unwrap_or(0)
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(digit))
                    .unwrap_or(u32::MAX),
            );
            self.pos += 1;
        }
        value
    }

    /// `scanBinaryOrOctalDigits`.
    fn scan_binary_or_octal_digits(&mut self, base: u8) {
        while let Some(byte) = self.byte(self.pos) {
            if byte != b'_' && !(byte.is_ascii_digit() && byte - b'0' < base) {
                break;
            }
            self.pos += 1;
        }
    }

    /// The value of a string or a template: the text itself when nothing in
    /// it was an escape, or the code units collected.
    fn cooked(&self, units: Option<Vec<u16>>, start: usize, end: usize) -> Value<'a> {
        match units {
            None => Value::Text(Cow::Borrowed(self.slice(start, end))),
            Some(units) => String::from_utf16(&units)
                .map_or(Value::LoneSurrogate, |text| Value::Text(Cow::Owned(text))),
        }
    }

    /// `scanString`.
    fn scan_string(&mut self) {
        let quote = self.byte(self.pos);
        self.pos += 1;
        let start = self.pos;
        let mut units: Vec<u16> = Vec::new();
        let mut escaped = false;
        let mut segment = start;
        let mut closed = false;
        while let Some(byte) = self.byte(self.pos) {
            if Some(byte) == quote {
                closed = true;
                break;
            }
            if byte == b'\\' {
                units.extend(self.slice(segment, self.pos).encode_utf16());
                self.scan_escape_sequence(true, &mut units);
                escaped = true;
                segment = self.pos;
                continue;
            }
            if byte == b'\n' || byte == b'\r' {
                break;
            }
            self.pos += 1;
        }
        self.value = if escaped {
            units.extend(self.slice(segment, self.pos).encode_utf16());
            self.cooked(Some(units), start, self.pos)
        } else {
            self.cooked(None, start, self.pos)
        };
        if closed {
            self.pos += 1;
        }
    }

    /// `scanTemplateAndSetTokenValue`.
    fn scan_template(&mut self, report_invalid_escapes: bool) -> Token {
        let started_with_backtick = self.is_byte(self.pos, b'`');
        self.pos += 1;
        let start = self.pos;
        let mut units: Vec<u16> = Vec::new();
        let mut escaped = false;
        let mut segment = start;
        let token;
        let end;
        loop {
            let Some(byte) = self.byte(self.pos) else {
                end = self.pos;
                token = if started_with_backtick {
                    Token::NoSubstitutionTemplateLiteral
                } else {
                    Token::TemplateTail
                };
                break;
            };
            if byte == b'`' {
                end = self.pos;
                self.pos += 1;
                token = if started_with_backtick {
                    Token::NoSubstitutionTemplateLiteral
                } else {
                    Token::TemplateTail
                };
                break;
            }
            if byte == b'$' && self.is_byte(self.pos + 1, b'{') {
                end = self.pos;
                self.pos += 2;
                token = if started_with_backtick {
                    Token::TemplateHead
                } else {
                    Token::TemplateMiddle
                };
                break;
            }
            if byte == b'\\' {
                units.extend(self.slice(segment, self.pos).encode_utf16());
                self.scan_escape_sequence(report_invalid_escapes, &mut units);
                escaped = true;
                segment = self.pos;
                continue;
            }
            if byte == b'\r' {
                units.extend(self.slice(segment, self.pos).encode_utf16());
                self.pos += 1;
                if self.is_byte(self.pos, b'\n') {
                    self.pos += 1;
                }
                units.push(u16::from(b'\n'));
                escaped = true;
                segment = self.pos;
                continue;
            }
            self.pos += 1;
        }
        self.value = if escaped {
            units.extend(self.slice(segment, end).encode_utf16());
            self.cooked(Some(units), start, end)
        } else {
            self.cooked(None, start, end)
        };
        token
    }

    /// `scanEscapeSequence`, for a string or a template: what the escape at
    /// the position stands for goes to `units`. `report` is whether an
    /// invalid escape would be reported, which decides what an octal one is.
    fn scan_escape_sequence(&mut self, report: bool, units: &mut Vec<u16>) {
        match self.escape_sequence(report) {
            Escape::Code(code) => push_code(units, code),
            Escape::Empty => {}
            Escape::Raw(from) => units.extend(self.slice(from, self.pos).encode_utf16()),
        }
    }

    fn escape_sequence(&mut self, report: bool) -> Escape {
        let start = self.pos;
        self.pos += 1;
        let Some(char) = self.char_at(self.pos) else {
            return Escape::Empty;
        };
        // The original takes one code unit; the other half of a pair, when
        // there is one, would follow as text, so both are taken here.
        self.pos += char.len_utf8();
        match char {
            '0'..='7' => {
                if char == '0' && !self.is_digit_at(self.pos) {
                    return Escape::Code(0);
                }
                let is_octal_at = |scanner: &Self| {
                    scanner
                        .byte(scanner.pos)
                        .is_some_and(|byte| (b'0'..=b'7').contains(&byte))
                };
                if char <= '3' && is_octal_at(self) {
                    self.pos += 1;
                }
                if is_octal_at(self) {
                    self.pos += 1;
                }
                if report {
                    let code = u32::from_str_radix(self.slice(start + 1, self.pos), 8).unwrap_or(0);
                    return Escape::Code(code);
                }
                Escape::Raw(start)
            }
            '8' | '9' => {
                if report {
                    Escape::Code(u32::from(char))
                } else {
                    Escape::Raw(start)
                }
            }
            'b' => Escape::Code(8),
            't' => Escape::Code(9),
            'n' => Escape::Code(10),
            'v' => Escape::Code(11),
            'f' => Escape::Code(12),
            'r' => Escape::Code(13),
            'u' => {
                if self.is_byte(self.pos, b'{') {
                    self.pos -= 2;
                    return self.scan_extended_unicode_escape();
                }
                self.fixed_hex_escape(start, 4)
            }
            'x' => self.fixed_hex_escape(start, 2),
            '\r' => {
                if self.is_byte(self.pos, b'\n') {
                    self.pos += 1;
                }
                Escape::Empty
            }
            '\n' | '\u{2028}' | '\u{2029}' => Escape::Empty,
            other => Escape::Code(u32::from(other)),
        }
    }

    /// `\uXXXX` and `\xXX`: exactly `count` hex digits, or the escape's own
    /// text up to the first character that isn't one.
    fn fixed_hex_escape(&mut self, start: usize, count: usize) -> Escape {
        let mut code = 0;
        for _ in 0..count {
            let Some(digit) = self.byte(self.pos).and_then(hex_value) else {
                return Escape::Raw(start);
            };
            code = code * 16 + digit;
            self.pos += 1;
        }
        Escape::Code(code)
    }

    /// `scanExtendedUnicodeEscape`: `\u{…}` at the position.
    fn scan_extended_unicode_escape(&mut self) -> Escape {
        let start = self.pos;
        self.pos += 3;
        let value = self.scan_hex_digits(false);
        let mut invalid = value.is_none_or(|value| value > 0x10_ffff);
        if self.is_byte(self.pos, b'}') {
            self.pos += 1;
        } else {
            invalid = true;
        }
        match value {
            Some(value) if !invalid => Escape::Code(value),
            _ => Escape::Raw(start),
        }
    }

    /// `peekExtendedUnicodeEscape`: the code point of the `\u{…}` at the
    /// position, whether or not it is closed.
    fn peek_extended_unicode_escape(&self) -> Option<u32> {
        if !(self.is_byte(self.pos + 1, b'u') && self.is_byte(self.pos + 2, b'{')) {
            return None;
        }
        let mut ahead = self.clone();
        ahead.pos += 3;
        ahead.scan_hex_digits(false)
    }

    /// `peekUnicodeEscape`: the code unit of the `\uXXXX` at the position.
    fn peek_unicode_escape(&self) -> Option<u32> {
        if !self.is_byte(self.pos + 1, b'u') {
            return None;
        }
        let mut code = 0;
        for offset in 2..6 {
            code = code * 16 + self.byte(self.pos + offset).and_then(hex_value)?;
        }
        Some(code)
    }

    /// The text an escape in an identifier stands for.
    fn escape_text(&self, escape: &Escape) -> String {
        match *escape {
            Escape::Code(code) => char::from_u32(code).map(String::from).unwrap_or_default(),
            Escape::Empty => String::new(),
            Escape::Raw(from) => self.slice(from, self.pos).to_owned(),
        }
    }

    /// `scanIdentifierParts`.
    fn scan_identifier_parts(&mut self) -> String {
        let mut result = String::new();
        let mut start = self.pos;
        while let Some(char) = self.char_at(self.pos) {
            if is_identifier_part(u32::from(char)) {
                self.pos += char.len_utf8();
            } else if char == '\\' {
                if self
                    .peek_extended_unicode_escape()
                    .is_some_and(is_identifier_part)
                {
                    // As the original: the text since the last escape is dropped.
                    let escape = self.scan_extended_unicode_escape();
                    result.push_str(&self.escape_text(&escape));
                    start = self.pos;
                    continue;
                }
                let Some(code) = self.peek_unicode_escape().filter(|&code| is_identifier_part(code))
                else {
                    break;
                };
                result.push_str(self.slice(start, self.pos));
                result.push_str(&self.escape_text(&Escape::Code(code)));
                self.pos += 6;
                start = self.pos;
            } else {
                break;
            }
        }
        result.push_str(self.slice(start, self.pos));
        result
    }

    /// `getIdentifierToken`.
    fn identifier_token(&mut self, value: Cow<'a, str>) -> Token {
        let token = if (2..=12).contains(&value.len())
            && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && KEYWORDS.binary_search(&value.as_ref()).is_ok()
        {
            Token::Keyword(match value.as_ref() {
                "as" => Keyword::As,
                "declare" => Keyword::Declare,
                "export" => Keyword::Export,
                "from" => Keyword::From,
                "import" => Keyword::Import,
                "module" => Keyword::Module,
                "require" => Keyword::Require,
                "type" => Keyword::Type,
                _ => Keyword::Other,
            })
        } else {
            Token::Identifier
        };
        self.value = Value::Text(value);
        self.set(token)
    }

    /// `scanIdentifier`: the identifier that starts with this character, if
    /// it can start one.
    fn scan_identifier(&mut self, first: char) -> Option<Token> {
        if !is_identifier_start(u32::from(first)) {
            return None;
        }
        self.pos += first.len_utf8();
        // The last character looked at, as the original's `ch`.
        let mut last = first;
        while let Some(char) = self.char_at(self.pos) {
            last = char;
            if !is_identifier_part(u32::from(char)) {
                break;
            }
            self.pos += char.len_utf8();
        }
        let mut value = Cow::Borrowed(self.slice(self.token_start, self.pos));
        if last == '\\' {
            value = Cow::Owned(value.into_owned() + &self.scan_identifier_parts());
        }
        Some(self.identifier_token(value))
    }

    /// An identifier that starts with an escape (`ab`), after `prefix`.
    fn scan_escaped_identifier(&mut self, prefix: &str) -> Option<String> {
        if self
            .peek_extended_unicode_escape()
            .is_some_and(is_identifier_start)
        {
            let escape = self.scan_extended_unicode_escape();
            return Some(format!(
                "{prefix}{}{}",
                self.escape_text(&escape),
                self.scan_identifier_parts()
            ));
        }
        let code = self.peek_unicode_escape().filter(|&code| is_identifier_start(code))?;
        self.pos += 6;
        Some(format!(
            "{prefix}{}{}",
            self.escape_text(&Escape::Code(code)),
            self.scan_identifier_parts()
        ))
    }

    /// `isConflictMarkerTrivia`.
    fn is_conflict_marker(&self, at: usize) -> bool {
        const LENGTH: usize = 7;
        let line_start = at == 0
            || self
                .text
                .get(..at)
                .is_some_and(|before| before.ends_with(is_line_break));
        if !line_start || at + LENGTH >= self.bytes.len() {
            return false;
        }
        let Some(byte) = self.byte(at) else {
            return false;
        };
        (0..LENGTH).all(|offset| self.is_byte(at + offset, byte))
            && (byte == b'=' || self.is_byte(at + LENGTH, b' '))
    }

    /// `scanConflictMarkerTrivia`.
    fn scan_conflict_marker(&mut self) {
        let Some(marker) = self.byte(self.pos) else {
            return;
        };
        if marker == b'<' || marker == b'>' {
            while self.pos < self.bytes.len() && self.line_break_at(self.pos).is_none() {
                self.pos += 1;
            }
        } else {
            while let Some(byte) = self.byte(self.pos) {
                if (byte == b'=' || byte == b'>') && byte != marker && self.is_conflict_marker(self.pos)
                {
                    break;
                }
                self.pos += 1;
            }
        }
    }

    /// `reScanTemplateToken(false)`.
    pub fn rescan_template_token(&mut self) -> Token {
        self.pos = self.token_start;
        let token = self.scan_template(true);
        self.set(token)
    }

    /// `scan`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per character, as the original's switch"
    )]
    pub fn scan(&mut self) -> Token {
        loop {
            self.token_start = self.pos;
            let Some(char) = self.char_at(self.pos) else {
                return self.set(Token::EndOfFile);
            };
            if self.pos == 0 && self.text.starts_with("#!") {
                // The shebang line, to its line break.
                self.pos = self.text.find(is_line_break).unwrap_or(self.text.len());
                continue;
            }
            let next = self.byte(self.pos + 1);
            let after = self.byte(self.pos + 2);
            match char {
                '\n' | '\r' | '\t' | '\u{b}' | '\u{c}' | ' ' => self.pos += 1,
                '!' => {
                    return match (next, after) {
                        (Some(b'='), Some(b'=')) => self.punctuation(3, Token::Other),
                        (Some(b'='), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Other),
                    };
                }
                '"' | '\'' => {
                    self.scan_string();
                    return self.set(Token::StringLiteral);
                }
                '`' => {
                    let token = self.scan_template(false);
                    return self.set(token);
                }
                '%' | '^' => {
                    return self.punctuation(if next == Some(b'=') { 2 } else { 1 }, Token::Other);
                }
                '&' | '|' => {
                    if char == '|' && self.is_conflict_marker(self.pos) {
                        self.scan_conflict_marker();
                        continue;
                    }
                    let same = u8::try_from(char).ok();
                    return match (next, after) {
                        (first, Some(b'=')) if first == same => self.punctuation(3, Token::Other),
                        (first, _) if first == same => self.punctuation(2, Token::Other),
                        (Some(b'='), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Other),
                    };
                }
                '(' => return self.punctuation(1, Token::OpenParen),
                '*' => {
                    return match (next, after) {
                        (Some(b'*'), Some(b'=')) => self.punctuation(3, Token::Other),
                        (Some(b'=' | b'*'), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Asterisk),
                    };
                }
                '+' | '-' => {
                    let same = u8::try_from(char).ok();
                    return self.punctuation(
                        if next == same || next == Some(b'=') { 2 } else { 1 },
                        Token::Other,
                    );
                }
                ',' => return self.punctuation(1, Token::Comma),
                '.' => {
                    if self.is_digit_at(self.pos + 1) {
                        self.scan_number();
                        return self.set(Token::Other);
                    }
                    if next == Some(b'.') && after == Some(b'.') {
                        return self.punctuation(3, Token::Other);
                    }
                    return self.punctuation(1, Token::Dot);
                }
                '/' => {
                    if next == Some(b'/') {
                        self.pos += 2;
                        while self.pos < self.bytes.len() && self.line_break_at(self.pos).is_none() {
                            self.pos += 1;
                        }
                        continue;
                    }
                    if next == Some(b'*') {
                        self.pos += 2;
                        self.pos = self
                            .text
                            .get(self.pos..)
                            .and_then(|rest| rest.find("*/"))
                            .map_or(self.bytes.len(), |end| self.pos + end + 2);
                        continue;
                    }
                    return self.punctuation(if next == Some(b'=') { 2 } else { 1 }, Token::Other);
                }
                '0' if self.pos + 2 < self.bytes.len()
                    && matches!(next, Some(b'X' | b'x' | b'B' | b'b' | b'O' | b'o')) =>
                {
                    self.pos += 2;
                    match next {
                        Some(b'X' | b'x') => {
                            self.scan_hex_digits(true);
                        }
                        Some(b'B' | b'b') => self.scan_binary_or_octal_digits(2),
                        _ => self.scan_binary_or_octal_digits(8),
                    }
                    self.check_big_int_suffix();
                    return self.set(Token::Other);
                }
                '0'..='9' => {
                    self.scan_number();
                    return self.set(Token::Other);
                }
                ':' | ';' | ')' | '~' | '@' => return self.punctuation(1, Token::Other),
                '<' => {
                    if self.is_conflict_marker(self.pos) {
                        self.scan_conflict_marker();
                        continue;
                    }
                    return match (next, after) {
                        (Some(b'<'), Some(b'=')) => self.punctuation(3, Token::Other),
                        (Some(b'<' | b'='), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Other),
                    };
                }
                '=' => {
                    if self.is_conflict_marker(self.pos) {
                        self.scan_conflict_marker();
                        continue;
                    }
                    return match (next, after) {
                        (Some(b'='), Some(b'=')) => self.punctuation(3, Token::Other),
                        (Some(b'=' | b'>'), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Equals),
                    };
                }
                '>' => {
                    if self.is_conflict_marker(self.pos) {
                        self.scan_conflict_marker();
                        continue;
                    }
                    return self.punctuation(1, Token::Other);
                }
                '?' => {
                    if next == Some(b'.') && !self.is_digit_at(self.pos + 2) {
                        return self.punctuation(2, Token::Other);
                    }
                    return match (next, after) {
                        (Some(b'?'), Some(b'=')) => self.punctuation(3, Token::Other),
                        (Some(b'?'), _) => self.punctuation(2, Token::Other),
                        _ => self.punctuation(1, Token::Other),
                    };
                }
                '[' => return self.punctuation(1, Token::OpenBracket),
                ']' => return self.punctuation(1, Token::CloseBracket),
                '{' => return self.punctuation(1, Token::OpenBrace),
                '}' => return self.punctuation(1, Token::CloseBrace),
                '\\' => {
                    if let Some(value) = self.scan_escaped_identifier("") {
                        return self.identifier_token(Cow::Owned(value));
                    }
                    return self.punctuation(1, Token::Unknown);
                }
                '#' => {
                    if self.pos != 0 && next == Some(b'!') {
                        return self.punctuation(1, Token::Unknown);
                    }
                    if next == Some(b'\\') {
                        self.pos += 1;
                        if let Some(value) = self.scan_escaped_identifier("#") {
                            self.value = Value::Text(Cow::Owned(value));
                            return self.set(Token::PrivateIdentifier);
                        }
                        self.pos -= 1;
                    }
                    match self.char_at(self.pos + 1) {
                        Some(first) if is_identifier_start(u32::from(first)) => {
                            self.pos += 1;
                            self.scan_identifier(first);
                        }
                        _ => self.pos += 1,
                    }
                    return self.set(Token::PrivateIdentifier);
                }
                '\u{fffd}' => {
                    self.pos = self.bytes.len();
                    return self.set(Token::NonTextFileMarker);
                }
                other => {
                    if let Some(token) = self.scan_identifier(other) {
                        return token;
                    }
                    self.pos += other.len_utf8();
                    if !is_white_space_single_line(other) && !is_line_break(other) {
                        return self.set(Token::Unknown);
                    }
                }
            }
        }
    }
}
