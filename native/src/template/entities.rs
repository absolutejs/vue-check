//! HTML entity decoding, as Vue's template parser does it: a port of the
//! `entities` package's `EntityDecoder` (7.0.1, `src/decode.ts`), over the
//! same lookup table.
//!
//! The original walks a JavaScript string by UTF-16 code unit. This walks the
//! UTF-8 bytes instead: every character an entity can contain is ASCII, and a
//! byte of a longer character (0x80 and up) fails each comparison exactly as
//! its UTF-16 unit would, so the two take the same path and count the same
//! number of consumed characters.
//!
//! entities is Copyright (c) Felix Böhm, BSD-2-Clause.

use super::entities_data::HTML_DECODE_TREE;

const VALUE_LENGTH: u16 = 0b1100_0000_0000_0000;
const FLAG13: u16 = 0b0010_0000_0000_0000;
const BRANCH_LENGTH: u16 = 0b0001_1111_1000_0000;
const JUMP_TABLE: u16 = 0b0000_0000_0111_1111;
const TO_LOWER_BIT: u8 = 0b10_0000;

/// A read past the end of the table is `undefined` in the original, which
/// every bit operation there treats as 0.
fn tree(index: usize) -> u16 {
    HTML_DECODE_TREE.get(index).copied().unwrap_or(0)
}

fn value_length(node: u16) -> usize {
    usize::from((node & VALUE_LENGTH) >> 14)
}

fn is_hexadecimal_letter(code: u8) -> bool {
    matches!(code, b'A'..=b'F' | b'a'..=b'f')
}

fn is_entity_in_attribute_invalid_end(code: u8) -> bool {
    code == b'=' || code.is_ascii_alphanumeric()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecoderState {
    EntityStart,
    NumericStart,
    NumericDecimal,
    NumericHex,
    NamedEntity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodingMode {
    /// Entities can end without a semicolon.
    Legacy,
    /// In an attribute, an entity without a semicolon isn't one when a letter,
    /// digit or `=` follows.
    Attribute,
}

/// What the decoder produced for one entity: its text, and how many
/// characters of input it stands for (the `&` included).
#[derive(Debug, Default)]
pub struct Decoded {
    units: Vec<u16>,
    pub consumed: usize,
}

impl Decoded {
    fn emit(&mut self, unit: u16, consumed: usize) {
        self.units.push(unit);
        self.consumed = consumed;
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The decoded text. The table stores a character outside the basic plane
    /// as its two UTF-16 halves, which become one character here.
    pub fn text(&self) -> String {
        String::from_utf16_lossy(&self.units)
    }

    pub fn clear(&mut self) {
        self.units.clear();
        self.consumed = 0;
    }
}

/// How far a `write` got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    /// The input ended inside the entity (the original's `-1`).
    Incomplete,
    /// Not an entity (the original's `0`).
    Nothing,
    /// An entity of this many characters was emitted.
    Entity,
}

#[derive(Debug)]
pub struct EntityDecoder {
    state: DecoderState,
    consumed: usize,
    /// The number so far (numeric entity) or the table index of the last
    /// match (named entity), as the original's one field.
    result: usize,
    tree_index: usize,
    excess: usize,
    mode: DecodingMode,
    run_consumed: usize,
}

impl Default for EntityDecoder {
    fn default() -> Self {
        Self {
            state: DecoderState::EntityStart,
            consumed: 1,
            result: 0,
            tree_index: 0,
            excess: 1,
            mode: DecodingMode::Legacy,
            run_consumed: 0,
        }
    }
}

impl EntityDecoder {
    pub fn start_entity(&mut self, mode: DecodingMode) {
        *self = Self {
            mode,
            ..Self::default()
        };
    }

    /// Reads the entity from `offset` (the character after the `&`).
    pub fn write(&mut self, input: &[u8], offset: usize, out: &mut Decoded) -> Written {
        match self.state {
            DecoderState::EntityStart => {
                if input.get(offset) == Some(&b'#') {
                    self.state = DecoderState::NumericStart;
                    self.consumed += 1;
                    return self.state_numeric_start(input, offset + 1, out);
                }
                self.state = DecoderState::NamedEntity;
                self.state_named_entity(input, offset, out)
            }
            DecoderState::NumericStart => self.state_numeric_start(input, offset, out),
            DecoderState::NumericDecimal => self.state_numeric_decimal(input, offset, out),
            DecoderState::NumericHex => self.state_numeric_hex(input, offset, out),
            DecoderState::NamedEntity => self.state_named_entity(input, offset, out),
        }
    }

    fn state_numeric_start(&mut self, input: &[u8], offset: usize, out: &mut Decoded) -> Written {
        let Some(&first) = input.get(offset) else {
            return Written::Incomplete;
        };
        if first | TO_LOWER_BIT == b'x' {
            self.state = DecoderState::NumericHex;
            self.consumed += 1;
            return self.state_numeric_hex(input, offset + 1, out);
        }
        self.state = DecoderState::NumericDecimal;
        self.state_numeric_decimal(input, offset, out)
    }

    fn state_numeric_hex(&mut self, input: &[u8], mut offset: usize, out: &mut Decoded) -> Written {
        while let Some(&char) = input.get(offset) {
            let digit = if char.is_ascii_digit() {
                char - b'0'
            } else if is_hexadecimal_letter(char) {
                (char | TO_LOWER_BIT) - b'a' + 10
            } else {
                return self.emit_numeric_entity(Some(char), 3, out);
            };
            // The original's number loses precision past 2^53 and this one
            // stops at its maximum; both are far beyond the last code point,
            // so both end as the replacement character.
            self.result = self
                .result
                .saturating_mul(16)
                .saturating_add(usize::from(digit));
            self.consumed += 1;
            offset += 1;
        }
        Written::Incomplete
    }

    fn state_numeric_decimal(
        &mut self,
        input: &[u8],
        mut offset: usize,
        out: &mut Decoded,
    ) -> Written {
        while let Some(&char) = input.get(offset) {
            if !char.is_ascii_digit() {
                return self.emit_numeric_entity(Some(char), 2, out);
            }
            self.result = self
                .result
                .saturating_mul(10)
                .saturating_add(usize::from(char - b'0'));
            self.consumed += 1;
            offset += 1;
        }
        Written::Incomplete
    }

    fn emit_numeric_entity(
        &mut self,
        last: Option<u8>,
        expected_length: usize,
        out: &mut Decoded,
    ) -> Written {
        // No digits at all.
        if self.consumed <= expected_length {
            return Written::Nothing;
        }
        if last == Some(b';') {
            self.consumed += 1;
        }
        let mut units = [0; 2];
        for unit in replace_code_point(self.result).encode_utf16(&mut units) {
            out.emit(*unit, self.consumed);
        }
        Written::Entity
    }

    fn state_named_entity(
        &mut self,
        input: &[u8],
        mut offset: usize,
        out: &mut Decoded,
    ) -> Written {
        let mut current = tree(self.tree_index);
        let mut length = value_length(current);
        while offset < input.len() {
            // A compact run: a stretch of the name with no branches.
            if length == 0 && current & FLAG13 != 0 {
                let run_length = usize::from((current & BRANCH_LENGTH) >> 7);
                if self.run_consumed == 0 {
                    let first = current & JUMP_TABLE;
                    if input.get(offset).map(|&char| u16::from(char)) != Some(first) {
                        return self.not_terminated_or_nothing(out);
                    }
                    offset += 1;
                    self.excess += 1;
                    self.run_consumed += 1;
                }
                while self.run_consumed < run_length {
                    let Some(&char) = input.get(offset) else {
                        return Written::Incomplete;
                    };
                    let index_in_packed = self.run_consumed - 1;
                    let packed = tree(self.tree_index + 1 + (index_in_packed >> 1));
                    let expected = if index_in_packed.is_multiple_of(2) {
                        packed & 0xff
                    } else {
                        (packed >> 8) & 0xff
                    };
                    if u16::from(char) != expected {
                        self.run_consumed = 0;
                        return self.not_terminated_or_nothing(out);
                    }
                    offset += 1;
                    self.excess += 1;
                    self.run_consumed += 1;
                }
                self.run_consumed = 0;
                self.tree_index += 1 + (run_length >> 1);
                current = tree(self.tree_index);
                length = value_length(current);
            }
            let Some(&char) = input.get(offset) else {
                break;
            };
            // A node whose value needs a semicolon, with no `;` branch stored.
            if char == b';' && length != 0 && current & FLAG13 != 0 {
                return Self::emit_named_entity_data(
                    self.tree_index,
                    length,
                    self.consumed + self.excess,
                    out,
                );
            }
            let Some(next) = determine_branch(current, self.tree_index + length.max(1), char)
            else {
                return if self.result == 0
                    || (self.mode == DecodingMode::Attribute
                        && (length == 0 || is_entity_in_attribute_invalid_end(char)))
                {
                    Written::Nothing
                } else {
                    self.emit_not_terminated_named_entity(out)
                };
            };
            self.tree_index = next;
            current = tree(self.tree_index);
            length = value_length(current);
            if length != 0 {
                if char == b';' {
                    return Self::emit_named_entity_data(
                        self.tree_index,
                        length,
                        self.consumed + self.excess,
                        out,
                    );
                }
                // An entity that may end without a semicolon: remember it.
                if current & FLAG13 == 0 {
                    self.result = self.tree_index;
                    self.consumed += self.excess;
                    self.excess = 0;
                }
            }
            offset += 1;
            self.excess += 1;
        }
        Written::Incomplete
    }

    fn not_terminated_or_nothing(&mut self, out: &mut Decoded) -> Written {
        if self.result == 0 {
            Written::Nothing
        } else {
            self.emit_not_terminated_named_entity(out)
        }
    }

    fn emit_not_terminated_named_entity(&mut self, out: &mut Decoded) -> Written {
        Self::emit_named_entity_data(
            self.result,
            value_length(tree(self.result)),
            self.consumed,
            out,
        )
    }

    fn emit_named_entity_data(
        result: usize,
        length: usize,
        consumed: usize,
        out: &mut Decoded,
    ) -> Written {
        out.emit(
            if length == 1 {
                tree(result) & !(VALUE_LENGTH | FLAG13)
            } else {
                tree(result + 1)
            },
            consumed,
        );
        if length == 3 {
            out.emit(tree(result + 2), consumed);
        }
        Written::Entity
    }

    /// The input ended inside an entity: emit what was matched, if anything.
    pub fn end(&mut self, out: &mut Decoded) -> Written {
        match self.state {
            DecoderState::NamedEntity => {
                if self.result != 0
                    && (self.mode != DecodingMode::Attribute || self.result == self.tree_index)
                {
                    self.emit_not_terminated_named_entity(out)
                } else {
                    Written::Nothing
                }
            }
            DecoderState::NumericDecimal => self.emit_numeric_entity(None, 2, out),
            DecoderState::NumericHex => self.emit_numeric_entity(None, 3, out),
            DecoderState::NumericStart | DecoderState::EntityStart => Written::Nothing,
        }
    }
}

/// The table index of the branch for `char`, if the node has one.
fn determine_branch(current: u16, node_index: usize, char: u8) -> Option<usize> {
    let branch_count = usize::from((current & BRANCH_LENGTH) >> 7);
    let jump_offset = current & JUMP_TABLE;
    let char = u16::from(char);
    // A single branch, stored in the node itself.
    if branch_count == 0 {
        return (jump_offset != 0 && char == jump_offset).then_some(node_index);
    }
    // A jump table: one slot per character from `jump_offset`.
    if jump_offset != 0 {
        let value = usize::from(char.checked_sub(jump_offset)?);
        if value >= branch_count {
            return None;
        }
        // An empty slot holds 0, which the original turns into -1.
        return usize::from(tree(node_index + value)).checked_sub(1);
    }
    // Sorted keys, packed two to a word, then one index per key.
    let packed_key_slots = (branch_count + 1) >> 1;
    let mut low = 0;
    let mut high = branch_count;
    while low < high {
        let middle = (low + high) >> 1;
        let packed = tree(node_index + (middle >> 1));
        let key = (packed >> ((middle & 1) * 8)) & 0xff;
        match key.cmp(&char) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => {
                return Some(usize::from(tree(node_index + packed_key_slots + middle)));
            }
        }
    }
    None
}

/// What a numeric entity stands for: the HTML spec's replacements for
/// surrogates, out-of-range numbers and the Windows-1252 control range.
fn replace_code_point(code_point: usize) -> char {
    let replaced = match code_point {
        0 => 65_533,
        128 => 8364,
        130 => 8218,
        131 => 402,
        132 => 8222,
        133 => 8230,
        134 => 8224,
        135 => 8225,
        136 => 710,
        137 => 8240,
        138 => 352,
        139 => 8249,
        140 => 338,
        142 => 381,
        145 => 8216,
        146 => 8217,
        147 => 8220,
        148 => 8221,
        149 => 8226,
        150 => 8211,
        151 => 8212,
        152 => 732,
        153 => 8482,
        154 => 353,
        155 => 8250,
        156 => 339,
        158 => 382,
        159 => 376,
        other => other,
    };
    // A surrogate or a number past the last code point isn't a `char`.
    u32::try_from(replaced)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// `decodeHTML`: every entity in a string, in legacy mode.
pub fn decode_html(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut decoder = EntityDecoder::default();
    let mut output = Decoded::default();
    let mut result = String::new();
    let mut last_index = 0;
    let mut offset = 0;
    while let Some(found) = find_from(bytes, b'&', offset) {
        offset = found;
        result.push_str(slice(input, last_index, offset));
        decoder.start_entity(DecodingMode::Legacy);
        output.clear();
        let written = decoder.write(bytes, offset + 1, &mut output);
        if written == Written::Incomplete {
            decoder.end(&mut output);
            result.push_str(&output.text());
            last_index = offset + output.consumed;
            break;
        }
        result.push_str(&output.text());
        last_index = offset + output.consumed;
        offset = if written == Written::Nothing {
            last_index + 1
        } else {
            last_index
        };
    }
    result.push_str(slice(input, last_index, input.len()));
    result
}

fn find_from(bytes: &[u8], needle: u8, from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|&byte| byte == needle)
        .map(|position| position + from)
}

/// A slice that is empty instead of a panic when the range isn't one.
fn slice(input: &str, start: usize, end: usize) -> &str {
    input.get(start..end).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::decode_html;

    #[test]
    fn named_numeric_and_legacy() {
        assert_eq!(decode_html("a &amp; b"), "a & b");
        assert_eq!(decode_html("&lt;div&gt;"), "<div>");
        assert_eq!(decode_html("&#65;&#x42;&#X43;"), "ABC");
        assert_eq!(decode_html("&amp"), "&");
        assert_eq!(decode_html("&ampx"), "&x");
        assert_eq!(decode_html("&notit;"), "\u{ac}it;");
        assert_eq!(decode_html("&unknown; &"), "&unknown; &");
        assert_eq!(decode_html("&#;&#x;"), "&#;&#x;");
    }

    #[test]
    fn replacements_and_pairs() {
        assert_eq!(decode_html("&#128;"), "\u{20ac}");
        assert_eq!(
            decode_html("&#0;&#xD800;&#x110000;"),
            "\u{fffd}\u{fffd}\u{fffd}"
        );
        assert_eq!(decode_html("&#x1F600;"), "\u{1f600}");
        assert_eq!(decode_html("&Afr;"), "\u{1d504}");
        assert_eq!(decode_html("&NotEqualTilde;"), "\u{2242}\u{338}");
    }
}
