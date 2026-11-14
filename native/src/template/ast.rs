//! The template syntax tree, as `@vue/compiler-core` builds it (`ast.ts`).
//!
//! Nodes live in one arena and refer to each other by [`NodeId`]: the parser
//! keeps changing an element after it has been added to its parent, which an
//! arena allows without shared mutable references.
//!
//! Offsets are UTF-8 byte offsets into the parsed text. The original's are
//! UTF-16 code units; they are converted once, where results leave this
//! program (see `utf16.rs`).

/// A position in the parsed text. The original also has a column, which
/// nothing in Vue's language tools reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub offset: usize,
    pub line: usize,
}

/// A range of the parsed text: `[start, end)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A node's location. `source` is the text the location was taken from; it is
/// kept apart from `start` and `end` because the original moves those in one
/// place without retaking the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub start: Position,
    pub end: Position,
    pub source: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Html,
    Svg,
    MathMl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementType {
    Element,
    Component,
    Slot,
    Template,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstantType {
    NotConstant,
    CanStringify,
}

#[derive(Clone, Debug)]
pub struct SimpleExpression {
    pub content: String,
    pub is_static: bool,
    pub const_type: ConstantType,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub struct Text {
    pub content: String,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub name_loc: Loc,
    pub value: Option<Text>,
    pub loc: Loc,
}

/// `v-for`'s expression, split into its parts.
#[derive(Clone, Debug)]
pub struct ForParseResult {
    pub source: SimpleExpression,
    pub value: Option<SimpleExpression>,
    pub key: Option<SimpleExpression>,
    pub index: Option<SimpleExpression>,
}

#[derive(Clone, Debug)]
pub struct Directive {
    pub name: String,
    pub raw_name: String,
    pub exp: Option<SimpleExpression>,
    pub arg: Option<SimpleExpression>,
    pub modifiers: Vec<SimpleExpression>,
    pub for_parse_result: Option<ForParseResult>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub enum Prop {
    Attribute(Attribute),
    Directive(Box<Directive>),
}

impl Prop {
    pub fn loc(&self) -> &Loc {
        match self {
            Self::Attribute(attribute) => &attribute.loc,
            Self::Directive(directive) => &directive.loc,
        }
    }

    pub fn loc_mut(&mut self) -> &mut Loc {
        match self {
            Self::Attribute(attribute) => &mut attribute.loc,
            Self::Directive(directive) => &mut directive.loc,
        }
    }

    /// The name duplicate attributes are found by: as written for a
    /// directive, the name for an attribute.
    pub fn written_name(&self) -> &str {
        match self {
            Self::Attribute(attribute) => &attribute.name,
            Self::Directive(directive) => &directive.raw_name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Element {
    pub tag: String,
    pub ns: Namespace,
    pub tag_type: ElementType,
    pub props: Vec<Prop>,
    pub children: Vec<NodeId>,
    pub loc: Loc,
    pub is_self_closing: bool,
    /// The content of a root-level block, in single-file-component mode.
    pub inner_loc: Option<Loc>,
}

#[derive(Clone, Debug)]
pub struct Interpolation {
    pub content: SimpleExpression,
    pub loc: Loc,
}

/// A run of adjacent text and interpolations, joined by the text transform.
#[derive(Clone, Debug)]
pub struct Compound {
    pub children: Vec<CompoundChild>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub enum CompoundChild {
    Node(NodeId),
    /// The ` + ` between two nodes.
    Text(&'static str),
}

/// A `v-if` chain: one branch per `v-if`, `v-else-if` and `v-else`.
#[derive(Clone, Debug)]
pub struct If {
    pub branches: Vec<NodeId>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub struct IfBranch {
    /// `None` for `v-else`.
    pub condition: Option<SimpleExpression>,
    pub children: Vec<NodeId>,
    pub loc: Loc,
}

/// A `v-for`, around the element it was on.
#[derive(Clone, Debug)]
pub struct For {
    pub parse_result: ForParseResult,
    pub children: Vec<NodeId>,
    pub loc: Loc,
}

#[derive(Clone, Debug)]
pub enum Node {
    Element(Element),
    Text(Text),
    Comment(Text),
    Interpolation(Interpolation),
    // Created by the transforms (`transform.rs`).
    Compound(Compound),
    If(If),
    IfBranch(IfBranch),
    For(Box<For>),
}

impl Node {
    pub fn loc(&self) -> &Loc {
        match self {
            Self::Element(element) => &element.loc,
            Self::Text(text) | Self::Comment(text) => &text.loc,
            Self::Interpolation(interpolation) => &interpolation.loc,
            Self::Compound(compound) => &compound.loc,
            Self::If(node) => &node.loc,
            Self::IfBranch(branch) => &branch.loc,
            Self::For(node) => &node.loc,
        }
    }
}

/// A compiler error: its code in the original's `ErrorCodes`, and where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompilerError {
    pub code: ErrorCode,
    pub position: Position,
}

/// The parser's error codes, numbered as `ErrorCodes` in `errors.ts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    CdataInHtmlContent,
    DuplicateAttribute,
    EofBeforeTagName,
    EofInCdata,
    EofInComment,
    EofInTag,
    MissingAttributeValue,
    MissingEndTagName,
    UnexpectedCharacterInAttributeName,
    UnexpectedCharacterInUnquotedAttributeValue,
    UnexpectedEqualsSignBeforeAttributeName,
    UnexpectedQuestionMarkInsteadOfTagName,
    UnexpectedSolidusInTag,
    InvalidEndTag,
    MissingEndTag,
    MissingInterpolationEnd,
    MissingDirectiveName,
    MissingDynamicDirectiveArgumentEnd,
}

impl ErrorCode {
    /// The code's number in the original's enum.
    pub fn number(self) -> u8 {
        match self {
            Self::CdataInHtmlContent => 1,
            Self::DuplicateAttribute => 2,
            Self::EofBeforeTagName => 5,
            Self::EofInCdata => 6,
            Self::EofInComment => 7,
            Self::EofInTag => 9,
            Self::MissingAttributeValue => 13,
            Self::MissingEndTagName => 14,
            Self::UnexpectedCharacterInAttributeName => 17,
            Self::UnexpectedCharacterInUnquotedAttributeValue => 18,
            Self::UnexpectedEqualsSignBeforeAttributeName => 19,
            Self::UnexpectedQuestionMarkInsteadOfTagName => 21,
            Self::UnexpectedSolidusInTag => 22,
            Self::InvalidEndTag => 23,
            Self::MissingEndTag => 24,
            Self::MissingInterpolationEnd => 25,
            Self::MissingDirectiveName => 26,
            Self::MissingDynamicDirectiveArgumentEnd => 27,
        }
    }
}

/// A directive's value in which an entity was decoded and which has a
/// character outside ASCII. The original locates the parts of an expression
/// by adding offsets in its text to where it starts in the source. With an
/// entity decoded the two no longer line up, and what it adds is counted in
/// UTF-16 units of the decoded text (see `Utf16Map::offset_in`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedValue {
    /// Where the value starts and ends in the parsed text.
    pub start: usize,
    pub end: usize,
    /// Its text, decoded.
    pub text: String,
}

/// A parsed template.
#[derive(Debug)]
pub struct Root {
    pub nodes: Vec<Node>,
    pub children: Vec<NodeId>,
    pub loc: Loc,
    pub errors: Vec<CompilerError>,
    /// The components the template uses, in the order the transforms meet
    /// them (`root.components`). Empty until `transform.rs` has run.
    pub components: Vec<String>,
    /// The directive values whose offsets are counted in their decoded text.
    pub decoded: Vec<DecodedValue>,
}

impl Root {
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0)
    }
}
