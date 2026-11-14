//! The codegen: a port of `@vue/language-core`'s `codegen/` (3.3.14), which
//! turns a single-file component into the TypeScript `vue-tsc` checks.
//!
//! The original is written as generators that yield pieces of code. Here each
//! one is a function that appends to a list, in the same order.

pub mod template;

use std::fmt::Write;

/// The block a piece of generated code maps back to (`block.name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Template,
    Script,
    ScriptSetup,
    Style(usize),
}

/// Which errors `vue-tsc` reports on a piece of generated code: the
/// `verification` of the original's `VueCodeInformation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verification {
    /// None are reported.
    Off,
    /// All are.
    On,
    /// `codeFeatures.doNotReportTs2353AndTs2561`.
    NotTs2353OrTs2561,
    /// `codeFeatures.doNotReportTs6133`.
    NotTs6133,
    /// A rule that depends on what was reported before it (under
    /// `@vue-expect-error`), which only Volar can evaluate.
    Stateful,
}

/// What a mapping carries. The original's other features (completion,
/// navigation, semantic tokens) serve an editor and don't reach a type check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub verification: Verification,
    /// Mappings that share a token are one mapping with several ranges
    /// (`__combineToken`, a `Symbol()` in the original).
    pub combine: Option<u32>,
}

impl Features {
    pub const fn new(verification: Verification) -> Self {
        Self {
            verification,
            combine: None,
        }
    }

    /// `{ verification: features.verification }`.
    pub fn verification_only(self) -> Self {
        Self::new(self.verification)
    }
}

/// A piece of generated code: plain text, or text mapped to a source block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Code {
    Text(String),
    Mapped {
        text: String,
        source: Source,
        offset: usize,
        features: Features,
    },
}

impl From<&str> for Code {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for Code {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

pub const NEW_LINE: &str = "\n";
pub const END_OF_LINE: &str = ";\n";

/// `identifierRE`: `/^[a-zA-Z_$][0-9a-zA-Z_$]*$/`.
pub fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '$')
}

/// `asType`: `{} as T` in TypeScript, a `JSDoc` cast in JavaScript.
pub fn as_type(type_text: &str, is_ts: bool) -> String {
    if is_ts {
        format!("{{}} as {type_text}")
    } else {
        format!("/** @type {{{type_text}}} */ ({{}})")
    }
}

/// `getRefBrandArgument`: the argument that carries the library's `Ref` type
/// into `__VLS_unwrap` and `__VLS_withDotValue`.
pub fn ref_brand_argument(lib: &str, is_ts: bool) -> String {
    as_type(&format!("import('{lib}').Ref<unknown>"), is_ts)
}

/// The original's `Boundary`: an empty mapping at the start of a range and
/// one at its end, combined into one mapping over the code between them.
#[derive(Clone, Copy, Debug)]
pub struct Boundary {
    source: Source,
    end_offset: usize,
    pub features: Features,
}

impl Boundary {
    /// `Boundary.start`: `token` is the new `Symbol()`.
    pub fn start(
        out: &mut Vec<Code>,
        token: u32,
        source: Source,
        start: usize,
        end: usize,
        features: Features,
    ) -> Self {
        let features = Features {
            combine: Some(token),
            ..features
        };
        out.push(Code::Mapped {
            text: String::new(),
            source,
            offset: start,
            features,
        });
        Self {
            source,
            end_offset: end,
            features,
        }
    }

    pub fn end(self) -> Code {
        Code::Mapped {
            text: String::new(),
            source: self.source,
            offset: self.end_offset,
            features: self.features,
        }
    }
}

/// `generateCamelized`: a hyphenated name as its camel-case form, each part
/// mapped to where it is in the name.
pub fn generate_camelized(
    out: &mut Vec<Code>,
    token: u32,
    code: &str,
    source: Source,
    offset: usize,
    features: Features,
) {
    let features = Features {
        combine: features.combine.or(Some(token)),
        ..features
    };
    let mut offset = offset;
    for (index, part) in code.split('-').enumerate() {
        if !part.is_empty() {
            out.push(Code::Mapped {
                text: if index == 0 {
                    part.to_owned()
                } else {
                    crate::js::capitalize(part)
                },
                source,
                offset,
                features,
            });
        }
        offset += part.len() + 1;
    }
}

/// `generateEscaped` with `/([\\'])/`: a class name inside quotes, with a
/// backslash before each backslash and quote.
pub fn generate_escaped(
    out: &mut Vec<Code>,
    token: u32,
    text: &str,
    source: Source,
    offset: usize,
    features: Features,
) {
    let features = Features {
        combine: features.combine.or(Some(token)),
        ..features
    };
    let mapped = |text: &str, offset: usize| Code::Mapped {
        text: text.to_owned(),
        source,
        offset,
        features,
    };
    // The original splits on the two characters and keeps them, so the
    // pieces alternate: text, then one character to escape.
    let mut start = 0;
    for (index, char) in text
        .char_indices()
        .filter(|(_, char)| matches!(char, '\\' | '\''))
    {
        out.push(mapped(text.get(start..index).unwrap_or(""), offset + start));
        out.push("\\".into());
        let end = index + char.len_utf8();
        out.push(mapped(text.get(index..end).unwrap_or(""), offset + index));
        start = end;
    }
    out.push(mapped(text.get(start..).unwrap_or(""), offset + start));
}

/// `generateStringLiteralKey`: a name in quotes.
pub fn generate_string_literal_key(
    out: &mut Vec<Code>,
    token: u32,
    code: &str,
    offset: usize,
    features: Features,
) {
    let boundary = Boundary::start(
        out,
        token,
        Source::Template,
        offset,
        offset + code.len(),
        features,
    );
    out.push("'".into());
    out.push(Code::Mapped {
        text: code.to_owned(),
        source: Source::Template,
        offset,
        features: boundary.features,
    });
    out.push("'".into());
    out.push(boundary.end());
}

/// `generateUnicode`: an attribute's value. One with a backslash or a line
/// break in it is written as `\uXXXX` escapes, one per UTF-16 unit.
pub fn generate_unicode(
    out: &mut Vec<Code>,
    token: u32,
    code: &str,
    offset: usize,
    features: Features,
) {
    if code.contains('\\') || code.contains('\n') {
        let boundary = Boundary::start(
            out,
            token,
            Source::Template,
            offset,
            offset + code.len(),
            features,
        );
        let mut escaped = String::with_capacity(code.len() * 6);
        for unit in code.encode_utf16() {
            // Writing to a `String` can't fail.
            let _ = write!(escaped, "\\u{unit:04x}");
        }
        out.push(escaped.into());
        out.push(boundary.end());
    } else {
        out.push(Code::Mapped {
            text: code.to_owned(),
            source: Source::Template,
            offset,
            features,
        });
    }
}

/// `toString(codes)`.
pub fn to_string(codes: &[Code]) -> String {
    codes
        .iter()
        .map(|code| match code {
            Code::Text(text) | Code::Mapped { text, .. } => text.as_str(),
        })
        .collect()
}
