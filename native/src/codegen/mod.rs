//! The codegen: a port of `@vue/language-core`'s `codegen/` (3.3.14), which
//! turns a single-file component into the TypeScript `vue-tsc` checks.
//!
//! The original is written as generators that yield pieces of code. Here each
//! one is a function that appends to a list, in the same order.

pub mod template;

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
