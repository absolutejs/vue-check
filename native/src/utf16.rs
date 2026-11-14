//! Offsets as JavaScript counts them.
//!
//! Everything inside this program is a UTF-8 byte offset. Vue's language
//! tools, Volar's source maps and TypeScript all count UTF-16 code units, so
//! every offset is converted here on its way out, and nowhere else.

/// Converts byte offsets into one text to UTF-16 offsets.
#[derive(Debug)]
pub struct Utf16Map {
    /// For each byte offset, the UTF-16 offset. `None` when the text is
    /// ASCII, where the two are the same.
    table: Option<Vec<usize>>,
    bytes: usize,
    units: usize,
}

impl Utf16Map {
    pub fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self {
                table: None,
                bytes: text.len(),
                units: text.len(),
            };
        }
        let mut table = Vec::with_capacity(text.len() + 1);
        let mut units = 0;
        for char in text.chars() {
            // Every byte of a character maps to where the character starts.
            table.extend(std::iter::repeat_n(units, char.len_utf8()));
            units += char.len_utf16();
        }
        table.push(units);
        Self {
            table: Some(table),
            bytes: text.len(),
            units,
        }
    }

    /// The UTF-16 offset of a byte offset. An offset past the end keeps its
    /// distance from the end (the original has a few of those).
    pub fn offset(&self, byte: usize) -> usize {
        if byte > self.bytes {
            return self.units + (byte - self.bytes);
        }
        match &self.table {
            Some(table) => table.get(byte).copied().unwrap_or(self.units),
            None => byte,
        }
    }
}

/// Marks an offset that was counted in the source's own text inside a
/// directive's value (see `Utf16Map::offset_in`): the original reads a value
/// both ways, decoded as an expression and as it is written when the
/// expression is checked again after its element.
pub const fn in_source(offset: usize) -> usize {
    offset | IN_SOURCE
}

/// Above any offset in a file.
const IN_SOURCE: usize = 1 << 48;

impl Utf16Map {
    /// The UTF-16 offset of `offset` in a template that starts at `base` in
    /// this text. Inside a directive's value that was decoded, the original
    /// counts from the value's start in units of the decoded text.
    pub fn offset_in(
        &self,
        base: usize,
        offset: usize,
        decoded: &[crate::template::ast::DecodedValue],
    ) -> usize {
        if offset & IN_SOURCE != 0 {
            return self.offset(base + (offset & !IN_SOURCE));
        }
        let Some(value) = decoded
            .iter()
            .find(|value| (value.start..value.end).contains(&offset))
        else {
            return self.offset(base + offset);
        };
        let within = offset - value.start;
        let units = match value.text.get(..within) {
            Some(before) => before.encode_utf16().count(),
            // Past the text's end: as far past it.
            None if within > value.text.len() => {
                value.text.encode_utf16().count() + (within - value.text.len())
            }
            // Inside a character: the characters that start before it.
            None => value
                .text
                .char_indices()
                .take_while(|(index, _)| *index < within)
                .map(|(_, char)| char.len_utf16())
                .sum(),
        };
        self.offset(base + value.start) + units
    }
}

#[cfg(test)]
mod tests {
    use super::Utf16Map;

    #[test]
    fn ascii_is_unchanged() {
        let map = Utf16Map::new("abc");
        assert_eq!(map.offset(2), 2);
        assert_eq!(map.offset(5), 5);
    }

    #[test]
    fn counts_utf16_units() {
        // "é" is 2 bytes and 1 unit; "😀" is 4 bytes and 2 units.
        let map = Utf16Map::new("aé😀b");
        assert_eq!(map.offset(1), 1);
        assert_eq!(map.offset(3), 2);
        assert_eq!(map.offset(7), 4);
        assert_eq!(map.offset(8), 5);
        assert_eq!(map.offset(10), 7);
    }
}
