//! Vue's template parser: a port of `@vue/compiler-core`'s `tokenizer.ts`
//! and `parser.ts` (3.5.45), with `@vue/compiler-dom`'s parser options.
//!
//! The original is a tokenizer that calls back into a parser, both kept in
//! module-level state. Here they are one struct, with the callbacks as
//! methods under the names the original gives them.
//!
//! It walks UTF-8 bytes where the original walks UTF-16 code units. Every
//! character the tokenizer compares against is ASCII, and a byte of a longer
//! character never equals one, so both make the same decisions at the same
//! characters. Offsets are byte offsets (see `ast.rs`).
//!
//! Where the original's behavior depends on something this port can't
//! reproduce (an offset before the start of the text, half of a surrogate
//! pair), parsing still finishes and the result is marked
//! [`Parsed::unsupported`], so the caller uses the JavaScript codegen for
//! that file instead of a guess.
//!
//! The tokenizer is adapted by Vue from htmlparser2, Copyright 2010, 2011,
//! Chris Winberry, MIT License. Vue is Copyright (c) Yuxi (Evan) You, MIT.

use super::ast::DecodedValue;
use super::ast::{
    Attribute, CompilerError, ConstantType, Directive, Element, ElementType, ErrorCode,
    ForParseResult, Interpolation, Loc, Namespace, Node, NodeId, Position, Prop, Root,
    SimpleExpression, Span, Text,
};
use super::entities::{Decoded, DecodingMode, EntityDecoder, Written, decode_html};
use super::tags::{HTML_TAGS, MATH_TAGS, SVG_TAGS, VOID_TAGS};
use crate::js;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseMode {
    /// A template: `<script>` and `<style>` hold raw text, `<title>` and
    /// `<textarea>` hold text with entities.
    Html,
    /// A single-file component: every root-level block except an HTML
    /// `<template>` holds raw text.
    Sfc,
}

/// How the parser is called. Everything else is `@vue/compiler-dom`'s
/// defaults, which Vue's language tools never change.
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub mode: ParseMode,
    /// `isNativeTag: () => true`: no tag is a component (single-file
    /// components have none at their root).
    pub every_tag_is_native: bool,
    /// `isPreTag: () => true`: whitespace is kept everywhere.
    pub every_tag_is_pre: bool,
    /// `@vue/compiler-dom` before 3.5.36: a CDATA section outside any element
    /// makes the parser throw (it reads the namespace of an element that
    /// isn't there).
    pub cdata_outside_element_throws: bool,
}

impl ParseOptions {
    /// As `parseSfc.ts` splits a single-file component into blocks.
    pub fn sfc(cdata_outside_element_throws: bool) -> Self {
        Self {
            mode: ParseMode::Sfc,
            every_tag_is_native: true,
            every_tag_is_pre: true,
            cdata_outside_element_throws,
        }
    }

    /// As `template/compile.ts` parses a template block.
    pub fn template(cdata_outside_element_throws: bool) -> Self {
        Self {
            mode: ParseMode::Html,
            every_tag_is_native: false,
            every_tag_is_pre: false,
            cdata_outside_element_throws,
        }
    }
}

/// A parse, and whether this port stands behind it.
#[derive(Debug)]
pub struct Parsed {
    pub root: Root,
    /// Set when the original's result couldn't be reproduced: why.
    pub unsupported: Option<&'static str>,
    /// The original keeps its `v-pre` and `<pre>` state between parses and
    /// never clears it. A parse that ends inside `v-pre`, or that throws
    /// halfway, changes how every later parse in the same process goes, which
    /// only the original can reproduce: a project with such a file can't use
    /// this port at all.
    pub poisons_later_parses: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    InterpolationOpen,
    Interpolation,
    InterpolationClose,
    BeforeTagName,
    InTagName,
    InSelfClosingTag,
    BeforeClosingTagName,
    InClosingTagName,
    AfterClosingTagName,
    BeforeAttrName,
    InAttrName,
    InDirName,
    InDirArg,
    InDirDynamicArg,
    InDirModifier,
    AfterAttrName,
    BeforeAttrValue,
    InAttrValueDq,
    InAttrValueSq,
    InAttrValueNq,
    BeforeDeclaration,
    InDeclaration,
    InProcessingInstruction,
    BeforeComment,
    CDATASequence,
    InSpecialComment,
    InCommentLike,
    BeforeSpecialS,
    BeforeSpecialT,
    SpecialStartSequence,
    InRCDATA,
    InEntity,
    InSFCRootTagName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuoteType {
    NoValue,
    Unquoted,
    Single,
    Double,
}

/// The sequence the tokenizer is looking for. The original compares these by
/// identity, so an end tag built from a root block's name is never the
/// `<title>` or `<textarea>` one, whatever its text.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Sequence {
    None,
    CdataEnd,
    CommentEnd,
    ScriptEnd,
    StyleEnd,
    TitleEnd,
    TextareaEnd,
    Custom(Vec<u8>),
}

impl Sequence {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::None => b"",
            Self::CdataEnd => b"]]>",
            Self::CommentEnd => b"-->",
            Self::ScriptEnd => b"</script",
            Self::StyleEnd => b"</style",
            Self::TitleEnd => b"</title",
            Self::TextareaEnd => b"</textarea",
            Self::Custom(bytes) => bytes,
        }
    }

    fn get(&self, index: usize) -> Option<u8> {
        self.bytes().get(index).copied()
    }

    fn len(&self) -> usize {
        self.bytes().len()
    }
}

const CDATA: &[u8] = b"CDATA[";
const DELIMITER_OPEN: &[u8] = b"{{";
const DELIMITER_CLOSE: &[u8] = b"}}";

fn is_tag_start_char(char: u8) -> bool {
    char.is_ascii_alphabetic()
}

/// The tokenizer's own whitespace: the five ASCII characters of HTML.
pub fn is_whitespace(char: u8) -> bool {
    matches!(char, b' ' | b'\n' | b'\t' | 0xc | b'\r')
}

fn is_end_of_tag_section(char: u8) -> bool {
    char == b'/' || char == b'>' || is_whitespace(char)
}

pub fn is_all_whitespace(text: &str) -> bool {
    text.bytes().all(is_whitespace)
}

fn has_newline_char(text: &str) -> bool {
    text.bytes().any(|char| char == b'\n' || char == b'\r')
}

/// Runs of whitespace become one space.
fn condense(text: &str) -> String {
    let mut condensed = String::with_capacity(text.len());
    let mut previous_is_whitespace = false;
    for char in text.chars() {
        if u8::try_from(char).is_ok_and(is_whitespace) {
            if !previous_is_whitespace {
                condensed.push(' ');
                previous_is_whitespace = true;
            }
        } else {
            condensed.push(char);
            previous_is_whitespace = false;
        }
    }
    condensed
}

fn is_core_component(tag: &str) -> bool {
    matches!(
        tag,
        "Teleport"
            | "teleport"
            | "Suspense"
            | "suspense"
            | "KeepAlive"
            | "keep-alive"
            | "BaseTransition"
            | "base-transition"
    )
}

fn is_built_in_component(tag: &str) -> bool {
    matches!(
        tag,
        "Transition" | "transition" | "TransitionGroup" | "transition-group"
    )
}

fn is_void_tag(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

fn is_dom_tag(tag: &str) -> bool {
    HTML_TAGS.contains(&tag) || SVG_TAGS.contains(&tag) || MATH_TAGS.contains(&tag)
}

fn is_ignore_newline_tag(tag: &str) -> bool {
    tag == "pre" || tag == "textarea"
}

fn is_fragment_template(element: &Element) -> bool {
    element.tag == "template"
        && element.props.iter().any(|prop| {
            matches!(prop, Prop::Directive(directive)
                if matches!(directive.name.as_str(), "if" | "else" | "else-if" | "for" | "slot"))
        })
}

/// `locStub`: the location of a node that has none.
fn loc_stub() -> Loc {
    let position = Position { offset: 0, line: 1 };
    Loc {
        start: position,
        end: position,
        source: Span::default(),
    }
}

fn create_simple_expression(
    content: String,
    is_static: bool,
    loc: Loc,
    const_type: ConstantType,
) -> SimpleExpression {
    SimpleExpression {
        content,
        is_static,
        const_type: if is_static {
            ConstantType::CanStringify
        } else {
            const_type
        },
        loc,
    }
}

struct Parser<'a> {
    input: &'a str,
    bytes: &'a [u8],
    options: ParseOptions,

    // The tokenizer's state.
    state: State,
    /// The start of the section being read. `None` is the original's `-1`.
    section_start: Option<usize>,
    index: usize,
    entity_start: usize,
    base_state: State,
    in_rcdata: bool,
    in_xml: bool,
    newlines: Vec<usize>,
    decoder: EntityDecoder,
    delimiter_index: usize,
    current_sequence: Sequence,
    sequence_index: usize,

    // The parser's state.
    nodes: Vec<Node>,
    root_children: Vec<NodeId>,
    errors: Vec<CompilerError>,
    current_open_tag: Option<NodeId>,
    current_prop: Option<Prop>,
    current_attr_value: String,
    current_attr_start_index: Option<usize>,
    current_attr_end_index: Option<usize>,
    in_pre: usize,
    in_v_pre: bool,
    current_v_pre_boundary: Option<NodeId>,
    /// The open elements, innermost last (the original keeps it first).
    stack: Vec<NodeId>,
    unsupported: Option<&'static str>,
    decoded: Vec<DecodedValue>,
    /// Why the original throws during this parse, when it does.
    throws: Option<&'static str>,
}

/// Parses a template or a single-file component.
pub fn parse(input: &str, options: ParseOptions) -> Parsed {
    let mut parser = Parser {
        input,
        bytes: input.as_bytes(),
        options,
        state: State::Text,
        section_start: Some(0),
        index: 0,
        entity_start: 0,
        base_state: State::Text,
        in_rcdata: false,
        in_xml: false,
        newlines: Vec::new(),
        decoder: EntityDecoder::default(),
        delimiter_index: 0,
        current_sequence: Sequence::None,
        sequence_index: 0,
        nodes: Vec::new(),
        root_children: Vec::new(),
        errors: Vec::new(),
        current_open_tag: None,
        current_prop: None,
        current_attr_value: String::new(),
        current_attr_start_index: None,
        current_attr_end_index: None,
        in_pre: 0,
        in_v_pre: false,
        current_v_pre_boundary: None,
        stack: Vec::new(),
        unsupported: None,
        decoded: Vec::new(),
        throws: None,
    };
    parser.run();
    let loc = parser.get_loc(0, input.len());
    let children = std::mem::take(&mut parser.root_children);
    let (children, _) = parser.condense_whitespace(children);
    Parsed {
        root: Root {
            nodes: parser.nodes,
            children,
            loc,
            errors: parser.errors,
            components: Vec::new(),
            decoded: parser.decoded,
        },
        unsupported: parser.unsupported.or(parser.throws),
        poisons_later_parses: parser.in_v_pre || parser.throws.is_some(),
    }
}

impl<'a> Parser<'a> {
    fn give_up(&mut self, reason: &'static str) {
        self.unsupported.get_or_insert(reason);
    }

    // ---- Tokenizer ----

    fn byte(&self, index: usize) -> Option<u8> {
        self.bytes.get(index).copied()
    }

    fn peek(&self) -> Option<u8> {
        self.byte(self.index + 1)
    }

    fn in_sfc_root(&self) -> bool {
        self.options.mode == ParseMode::Sfc && self.stack.is_empty()
    }

    /// The section start, for the callbacks that are only reached with one.
    fn section(&mut self) -> usize {
        if let Some(start) = self.section_start {
            start
        } else {
            self.give_up("the tokenizer read a section that has no start");
            self.index
        }
    }

    /// A position with its line, from the newlines seen so far (as the
    /// original, which can therefore undercount for a position it has only
    /// looked ahead to).
    fn get_pos(&self, index: usize) -> Position {
        let before = self.newlines.partition_point(|&newline| newline < index);
        Position {
            offset: index,
            line: before + 1,
        }
    }

    fn state_text(&mut self, char: u8) {
        if char == b'<' {
            let start = self.section();
            if self.index > start {
                self.ontext(start, self.index);
            }
            self.state = State::BeforeTagName;
            self.section_start = Some(self.index);
        } else if char == b'&' {
            self.start_entity();
        } else if !self.in_v_pre && Some(&char) == DELIMITER_OPEN.first() {
            self.state = State::InterpolationOpen;
            self.delimiter_index = 0;
            self.state_interpolation_open(char);
        }
    }

    fn state_interpolation_open(&mut self, char: u8) {
        if DELIMITER_OPEN.get(self.delimiter_index) == Some(&char) {
            if self.delimiter_index == DELIMITER_OPEN.len() - 1 {
                let start = self.index + 1 - DELIMITER_OPEN.len();
                let section = self.section();
                if start > section {
                    self.ontext(section, start);
                }
                self.state = State::Interpolation;
                self.section_start = Some(start);
            } else {
                self.delimiter_index += 1;
            }
        } else if self.in_rcdata {
            self.state = State::InRCDATA;
            self.state_in_rcdata(char);
        } else {
            self.state = State::Text;
            self.state_text(char);
        }
    }

    fn state_interpolation(&mut self, char: u8) {
        if Some(&char) == DELIMITER_CLOSE.first() {
            self.state = State::InterpolationClose;
            self.delimiter_index = 0;
            self.state_interpolation_close(char);
        }
    }

    fn state_interpolation_close(&mut self, char: u8) {
        if DELIMITER_CLOSE.get(self.delimiter_index) == Some(&char) {
            if self.delimiter_index == DELIMITER_CLOSE.len() - 1 {
                let start = self.section();
                self.oninterpolation(start, self.index + 1);
                self.state = if self.in_rcdata {
                    State::InRCDATA
                } else {
                    State::Text
                };
                self.section_start = Some(self.index + 1);
            } else {
                self.delimiter_index += 1;
            }
        } else {
            self.state = State::Interpolation;
            self.state_interpolation(char);
        }
    }

    fn state_special_start_sequence(&mut self, char: u8) {
        let is_end = self.sequence_index == self.current_sequence.len();
        let is_match = if is_end {
            // At the end of the sequence, the tag name must have ended.
            is_end_of_tag_section(char)
        } else {
            self.current_sequence.get(self.sequence_index) == Some(char | 0x20)
        };
        if !is_match {
            self.in_rcdata = false;
        } else if !is_end {
            self.sequence_index += 1;
            return;
        }
        self.sequence_index = 0;
        self.state = State::InTagName;
        self.state_in_tag_name(char);
    }

    /// Looks for an end tag. In `<title>` and `<textarea>`, also decodes
    /// entities and reads interpolations.
    fn state_in_rcdata(&mut self, char: u8) {
        if self.sequence_index == self.current_sequence.len() {
            if char == b'>' || is_whitespace(char) {
                let end_of_text = self.index - self.current_sequence.len();
                let section = self.section();
                if section < end_of_text {
                    self.ontext(section, end_of_text);
                }
                // Skip over the `</`.
                self.section_start = Some(end_of_text + 2);
                self.state_in_closing_tag_name(char);
                self.in_rcdata = false;
                return;
            }
            self.sequence_index = 0;
        }
        if self.current_sequence.get(self.sequence_index) == Some(char | 0x20) {
            self.sequence_index += 1;
        } else if self.sequence_index == 0 {
            if self.current_sequence == Sequence::TitleEnd
                || (self.current_sequence == Sequence::TextareaEnd && !self.in_sfc_root())
            {
                if char == b'&' {
                    self.start_entity();
                } else if !self.in_v_pre && Some(&char) == DELIMITER_OPEN.first() {
                    self.state = State::InterpolationOpen;
                    self.delimiter_index = 0;
                    self.state_interpolation_open(char);
                }
            } else if self.fast_forward_to(b'<') {
                self.sequence_index = 1;
            }
        } else {
            // A `<` restarts the sequence: `<</script>`.
            self.sequence_index = usize::from(char == b'<');
        }
    }

    fn state_cdata_sequence(&mut self, char: u8) {
        if CDATA.get(self.sequence_index) == Some(&char) {
            self.sequence_index += 1;
            if self.sequence_index == CDATA.len() {
                self.state = State::InCommentLike;
                self.current_sequence = Sequence::CdataEnd;
                self.sequence_index = 0;
                self.section_start = Some(self.index + 1);
            }
        } else {
            self.sequence_index = 0;
            self.state = State::InDeclaration;
            self.state_in_declaration(char);
        }
    }

    /// Skips to the next `char`. Returns whether there is one.
    fn fast_forward_to(&mut self, char: u8) -> bool {
        loop {
            self.index += 1;
            let Some(current) = self.byte(self.index) else {
                break;
            };
            if current == b'\n' {
                self.newlines.push(self.index);
            }
            if current == char {
                return true;
            }
        }
        // The main loop moves on by one after every state.
        self.index = self.bytes.len().saturating_sub(1);
        false
    }

    /// Comments and CDATA end with `-->` and `]]>`.
    fn state_in_comment_like(&mut self, char: u8) {
        if self.current_sequence.get(self.sequence_index) == Some(char) {
            self.sequence_index += 1;
            if self.sequence_index == self.current_sequence.len() {
                let start = self.section();
                let end = self.index.saturating_sub(2);
                if self.current_sequence == Sequence::CdataEnd {
                    self.oncdata(start, end);
                } else {
                    self.oncomment(start, end);
                }
                self.sequence_index = 0;
                self.section_start = Some(self.index + 1);
                self.state = State::Text;
            }
        } else if self.sequence_index == 0 {
            if let Some(first) = self.current_sequence.get(0)
                && self.fast_forward_to(first)
            {
                self.sequence_index = 1;
            }
        } else if self.current_sequence.get(self.sequence_index - 1) != Some(char) {
            // Longer sequences are allowed: `--->`, `]]]>`.
            self.sequence_index = 0;
        }
    }

    fn start_special(&mut self, sequence: Sequence, offset: usize) {
        self.enter_rcdata(sequence, offset);
        self.state = State::SpecialStartSequence;
    }

    fn enter_rcdata(&mut self, sequence: Sequence, offset: usize) {
        self.in_rcdata = true;
        self.current_sequence = sequence;
        self.sequence_index = offset;
    }

    fn state_before_tag_name(&mut self, char: u8) {
        if char == b'!' {
            self.state = State::BeforeDeclaration;
            self.section_start = Some(self.index + 1);
        } else if char == b'?' {
            self.state = State::InProcessingInstruction;
            self.section_start = Some(self.index + 1);
        } else if is_tag_start_char(char) {
            self.section_start = Some(self.index);
            if self.in_sfc_root() {
                // Everything at the root but an HTML <template> is raw text.
                self.state = State::InSFCRootTagName;
            } else if self.in_xml {
                self.state = State::InTagName;
            } else if char == b't' {
                self.state = State::BeforeSpecialT;
            } else {
                self.state = if char == b's' {
                    State::BeforeSpecialS
                } else {
                    State::InTagName
                };
            }
        } else if char == b'/' {
            self.state = State::BeforeClosingTagName;
        } else {
            self.state = State::Text;
            self.state_text(char);
        }
    }

    fn state_in_tag_name(&mut self, char: u8) {
        if is_end_of_tag_section(char) {
            self.handle_tag_name(char);
        }
    }

    fn state_in_sfc_root_tag_name(&mut self, char: u8) {
        if is_end_of_tag_section(char) {
            let start = self.section();
            let tag = self.get_slice(start, self.index);
            if tag != "template" {
                // The original keeps each character's low 8 bits, which for a
                // character outside ASCII is neither it nor its UTF-8 bytes.
                if !tag.is_ascii() {
                    self.give_up("a root block's name has a character outside ASCII");
                }
                let mut sequence = b"</".to_vec();
                sequence.extend_from_slice(tag.as_bytes());
                self.enter_rcdata(Sequence::Custom(sequence), 0);
            }
            self.handle_tag_name(char);
        }
    }

    fn handle_tag_name(&mut self, char: u8) {
        let start = self.section();
        self.onopentagname(start, self.index);
        self.section_start = None;
        self.state = State::BeforeAttrName;
        self.state_before_attr_name(char);
    }

    fn state_before_closing_tag_name(&mut self, char: u8) {
        if is_whitespace(char) {
            // Ignored.
        } else if char == b'>' {
            self.emit_error(ErrorCode::MissingEndTagName, self.index);
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
        } else {
            self.state = if is_tag_start_char(char) {
                State::InClosingTagName
            } else {
                State::InSpecialComment
            };
            self.section_start = Some(self.index);
        }
    }

    fn state_in_closing_tag_name(&mut self, char: u8) {
        if char == b'>' || is_whitespace(char) {
            let start = self.section();
            self.onclosetag(start, self.index);
            self.section_start = None;
            self.state = State::AfterClosingTagName;
            self.state_after_closing_tag_name(char);
        }
    }

    fn state_after_closing_tag_name(&mut self, char: u8) {
        // Everything up to the ">" is skipped.
        if char == b'>' {
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_before_attr_name(&mut self, char: u8) {
        if char == b'>' {
            self.onopentagend(self.index);
            self.state = if self.in_rcdata {
                State::InRCDATA
            } else {
                State::Text
            };
            self.section_start = Some(self.index + 1);
        } else if char == b'/' {
            self.state = State::InSelfClosingTag;
            if self.peek() != Some(b'>') {
                self.emit_error(ErrorCode::UnexpectedSolidusInTag, self.index);
            }
        } else if char == b'<' && self.peek() == Some(b'/') {
            // A `</` inside an open tag ends it, which helps with the
            // half-typed input an editor has.
            self.onopentagend(self.index);
            self.state = State::BeforeTagName;
            self.section_start = Some(self.index);
        } else if !is_whitespace(char) {
            if char == b'=' {
                self.emit_error(
                    ErrorCode::UnexpectedEqualsSignBeforeAttributeName,
                    self.index,
                );
            }
            self.handle_attr_start(char);
        }
    }

    fn handle_attr_start(&mut self, char: u8) {
        if char == b'v' && self.peek() == Some(b'-') {
            self.state = State::InDirName;
            self.section_start = Some(self.index);
        } else if matches!(char, b'.' | b':' | b'@' | b'#') {
            self.ondirname(self.index, self.index + 1);
            self.state = State::InDirArg;
            self.section_start = Some(self.index + 1);
        } else {
            self.state = State::InAttrName;
            self.section_start = Some(self.index);
        }
    }

    fn state_in_self_closing_tag(&mut self, char: u8) {
        if char == b'>' {
            self.onselfclosingtag(self.index);
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
            // In case the self-closing tag was a special one.
            self.in_rcdata = false;
        } else if !is_whitespace(char) {
            self.state = State::BeforeAttrName;
            self.state_before_attr_name(char);
        }
    }

    fn state_in_attr_name(&mut self, char: u8) {
        if char == b'=' || is_end_of_tag_section(char) {
            let start = self.section();
            self.onattribname(start, self.index);
            self.handle_attr_name_end(char);
        } else if matches!(char, b'"' | b'\'' | b'<') {
            self.emit_error(ErrorCode::UnexpectedCharacterInAttributeName, self.index);
        }
    }

    fn state_in_dir_name(&mut self, char: u8) {
        if char == b'=' || is_end_of_tag_section(char) {
            let start = self.section();
            self.ondirname(start, self.index);
            self.handle_attr_name_end(char);
        } else if char == b':' {
            let start = self.section();
            self.ondirname(start, self.index);
            self.state = State::InDirArg;
            self.section_start = Some(self.index + 1);
        } else if char == b'.' {
            let start = self.section();
            self.ondirname(start, self.index);
            self.state = State::InDirModifier;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_in_dir_arg(&mut self, char: u8) {
        if char == b'=' || is_end_of_tag_section(char) {
            let start = self.section();
            self.ondirarg(start, self.index);
            self.handle_attr_name_end(char);
        } else if char == b'[' {
            self.state = State::InDirDynamicArg;
        } else if char == b'.' {
            let start = self.section();
            self.ondirarg(start, self.index);
            self.state = State::InDirModifier;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_in_dynamic_dir_arg(&mut self, char: u8) {
        if char == b']' {
            self.state = State::InDirArg;
        } else if char == b'=' || is_end_of_tag_section(char) {
            // The original includes the character that ended the argument.
            let start = self.section();
            self.ondirarg(start, self.index + 1);
            self.handle_attr_name_end(char);
            self.emit_error(ErrorCode::MissingDynamicDirectiveArgumentEnd, self.index);
        }
    }

    fn state_in_dir_modifier(&mut self, char: u8) {
        if char == b'=' || is_end_of_tag_section(char) {
            let start = self.section();
            self.ondirmodifier(start, self.index);
            self.handle_attr_name_end(char);
        } else if char == b'.' {
            let start = self.section();
            self.ondirmodifier(start, self.index);
            self.section_start = Some(self.index + 1);
        }
    }

    fn handle_attr_name_end(&mut self, char: u8) {
        self.section_start = Some(self.index);
        self.state = State::AfterAttrName;
        self.onattribnameend(self.index);
        self.state_after_attr_name(char);
    }

    fn state_after_attr_name(&mut self, char: u8) {
        if char == b'=' {
            self.state = State::BeforeAttrValue;
        } else if char == b'/' || char == b'>' {
            let start = self.section();
            self.onattribend(QuoteType::NoValue, start);
            self.section_start = None;
            self.state = State::BeforeAttrName;
            self.state_before_attr_name(char);
        } else if !is_whitespace(char) {
            let start = self.section();
            self.onattribend(QuoteType::NoValue, start);
            self.handle_attr_start(char);
        }
    }

    fn state_before_attr_value(&mut self, char: u8) {
        if char == b'"' {
            self.state = State::InAttrValueDq;
            self.section_start = Some(self.index + 1);
        } else if char == b'\'' {
            self.state = State::InAttrValueSq;
            self.section_start = Some(self.index + 1);
        } else if !is_whitespace(char) {
            self.section_start = Some(self.index);
            self.state = State::InAttrValueNq;
            self.state_in_attr_value_no_quotes(char);
        }
    }

    fn handle_in_attr_value(&mut self, char: u8, quote: u8) {
        if char == quote {
            let start = self.section();
            self.onattribdata(start, self.index);
            self.section_start = None;
            self.onattribend(
                if quote == b'"' {
                    QuoteType::Double
                } else {
                    QuoteType::Single
                },
                self.index + 1,
            );
            self.state = State::BeforeAttrName;
        } else if char == b'&' {
            self.start_entity();
        }
    }

    fn state_in_attr_value_no_quotes(&mut self, char: u8) {
        if is_whitespace(char) || char == b'>' {
            let start = self.section();
            self.onattribdata(start, self.index);
            self.section_start = None;
            self.onattribend(QuoteType::Unquoted, self.index);
            self.state = State::BeforeAttrName;
            self.state_before_attr_name(char);
        } else if matches!(char, b'"' | b'\'' | b'<' | b'=' | b'`') {
            self.emit_error(
                ErrorCode::UnexpectedCharacterInUnquotedAttributeValue,
                self.index,
            );
        } else if char == b'&' {
            self.start_entity();
        }
    }

    fn state_before_declaration(&mut self, char: u8) {
        if char == b'[' {
            self.state = State::CDATASequence;
            self.sequence_index = 0;
        } else {
            self.state = if char == b'-' {
                State::BeforeComment
            } else {
                State::InDeclaration
            };
        }
    }

    fn state_in_declaration(&mut self, char: u8) {
        if char == b'>' || self.fast_forward_to(b'>') {
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_in_processing_instruction(&mut self, char: u8) {
        if char == b'>' || self.fast_forward_to(b'>') {
            let start = self.section();
            self.onprocessinginstruction(start);
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_before_comment(&mut self, char: u8) {
        if char == b'-' {
            self.state = State::InCommentLike;
            self.current_sequence = Sequence::CommentEnd;
            // Short comments are allowed: `<!-->`.
            self.sequence_index = 2;
            self.section_start = Some(self.index + 1);
        } else {
            self.state = State::InDeclaration;
        }
    }

    fn state_in_special_comment(&mut self, char: u8) {
        if char == b'>' || self.fast_forward_to(b'>') {
            let start = self.section();
            self.oncomment(start, self.index);
            self.state = State::Text;
            self.section_start = Some(self.index + 1);
        }
    }

    fn state_before_special_s(&mut self, char: u8) {
        if char == b'c' {
            self.start_special(Sequence::ScriptEnd, 4);
        } else if char == b't' {
            self.start_special(Sequence::StyleEnd, 4);
        } else {
            self.state = State::InTagName;
            self.state_in_tag_name(char);
        }
    }

    fn state_before_special_t(&mut self, char: u8) {
        if char == b'i' {
            self.start_special(Sequence::TitleEnd, 4);
        } else if char == b'e' {
            self.start_special(Sequence::TextareaEnd, 4);
        } else {
            self.state = State::InTagName;
            self.state_in_tag_name(char);
        }
    }

    fn start_entity(&mut self) {
        self.base_state = self.state;
        self.state = State::InEntity;
        self.entity_start = self.index;
        self.decoder.start_entity(
            if self.base_state == State::Text || self.base_state == State::InRCDATA {
                DecodingMode::Legacy
            } else {
                DecodingMode::Attribute
            },
        );
    }

    fn state_in_entity(&mut self) {
        let mut decoded = Decoded::default();
        let written = self.decoder.write(self.bytes, self.index, &mut decoded);
        self.emit_code_points(&decoded);
        match written {
            Written::Incomplete => self.index = self.bytes.len().saturating_sub(1),
            Written::Nothing => {
                self.state = self.base_state;
                self.index = self.entity_start;
            }
            Written::Entity => self.state = self.base_state,
        }
    }

    /// `emitCodePoint`: an entity was decoded. (The original is called once
    /// per UTF-16 unit; a second call for the same entity only appends.)
    fn emit_code_points(&mut self, decoded: &Decoded) {
        if decoded.is_empty() {
            return;
        }
        let text = decoded.text();
        let in_attribute = self.base_state != State::Text && self.base_state != State::InRCDATA;
        let section = self.section();
        if section < self.entity_start {
            if in_attribute {
                self.onattribdata(section, self.entity_start);
            } else {
                self.ontext(section, self.entity_start);
            }
        }
        let end = self.entity_start + decoded.consumed;
        self.section_start = Some(end);
        self.index = end.saturating_sub(1);
        if in_attribute {
            self.onattribentity(&text, self.entity_start, end);
        } else {
            self.ontextentity(&text, self.entity_start, end);
        }
    }

    fn run(&mut self) {
        while let Some(char) = self.byte(self.index) {
            if char == b'\n' && self.state != State::InEntity {
                self.newlines.push(self.index);
            }
            match self.state {
                State::Text => self.state_text(char),
                State::InterpolationOpen => self.state_interpolation_open(char),
                State::Interpolation => self.state_interpolation(char),
                State::InterpolationClose => self.state_interpolation_close(char),
                State::SpecialStartSequence => self.state_special_start_sequence(char),
                State::InRCDATA => self.state_in_rcdata(char),
                State::CDATASequence => self.state_cdata_sequence(char),
                State::InAttrValueDq => self.handle_in_attr_value(char, b'"'),
                State::InAttrName => self.state_in_attr_name(char),
                State::InDirName => self.state_in_dir_name(char),
                State::InDirArg => self.state_in_dir_arg(char),
                State::InDirDynamicArg => self.state_in_dynamic_dir_arg(char),
                State::InDirModifier => self.state_in_dir_modifier(char),
                State::InCommentLike => self.state_in_comment_like(char),
                State::InSpecialComment => self.state_in_special_comment(char),
                State::BeforeAttrName => self.state_before_attr_name(char),
                State::InTagName => self.state_in_tag_name(char),
                State::InSFCRootTagName => self.state_in_sfc_root_tag_name(char),
                State::InClosingTagName => self.state_in_closing_tag_name(char),
                State::BeforeTagName => self.state_before_tag_name(char),
                State::AfterAttrName => self.state_after_attr_name(char),
                State::InAttrValueSq => self.handle_in_attr_value(char, b'\''),
                State::BeforeAttrValue => self.state_before_attr_value(char),
                State::BeforeClosingTagName => self.state_before_closing_tag_name(char),
                State::AfterClosingTagName => self.state_after_closing_tag_name(char),
                State::BeforeSpecialS => self.state_before_special_s(char),
                State::BeforeSpecialT => self.state_before_special_t(char),
                State::InAttrValueNq => self.state_in_attr_value_no_quotes(char),
                State::InSelfClosingTag => self.state_in_self_closing_tag(char),
                State::InDeclaration => self.state_in_declaration(char),
                State::BeforeDeclaration => self.state_before_declaration(char),
                State::BeforeComment => self.state_before_comment(char),
                State::InProcessingInstruction => self.state_in_processing_instruction(char),
                State::InEntity => self.state_in_entity(),
            }
            self.index += 1;
        }
        self.cleanup();
        self.finish();
    }

    /// Emits what has been read of a text or an attribute value.
    fn cleanup(&mut self) {
        if self.section_start == Some(self.index) {
            return;
        }
        if self.state == State::Text || (self.state == State::InRCDATA && self.sequence_index == 0)
        {
            let start = self.section();
            self.ontext(start, self.index);
            self.section_start = Some(self.index);
        } else if matches!(
            self.state,
            State::InAttrValueDq | State::InAttrValueSq | State::InAttrValueNq
        ) {
            let start = self.section();
            self.onattribdata(start, self.index);
            self.section_start = Some(self.index);
        }
    }

    fn finish(&mut self) {
        if self.state == State::InEntity {
            let mut decoded = Decoded::default();
            self.decoder.end(&mut decoded);
            self.emit_code_points(&decoded);
            self.state = self.base_state;
        }
        self.handle_trailing_data();
        self.onend();
    }

    fn handle_trailing_data(&mut self) {
        let end_index = self.bytes.len();
        if self.section_start.is_some_and(|start| start >= end_index) {
            return;
        }
        if self.state == State::InCommentLike {
            let start = self.section();
            if self.current_sequence == Sequence::CdataEnd {
                self.oncdata(start, end_index);
            } else {
                self.oncomment(start, end_index);
            }
        } else if matches!(
            self.state,
            State::InTagName
                | State::BeforeAttrName
                | State::BeforeAttrValue
                | State::AfterAttrName
                | State::InAttrName
                | State::InDirName
                | State::InDirArg
                | State::InDirDynamicArg
                | State::InDirModifier
                | State::InAttrValueSq
                | State::InAttrValueDq
                | State::InAttrValueNq
                | State::InClosingTagName
        ) {
            // An unfinished tag is ignored.
        } else if let Some(start) = self.section_start {
            self.ontext(start, end_index);
        } else {
            // The original reads from offset -1 here, which JavaScript counts
            // from the end of the text.
            self.give_up("the text ends inside a tag that has no section start");
        }
    }

    // ---- Parser ----

    fn get_slice(&mut self, start: usize, end: usize) -> &'a str {
        let input = self.input;
        if let Some(slice) = input.get(start..end.min(input.len())) {
            slice
        } else {
            if start <= end {
                self.give_up("a slice doesn't fall on character boundaries");
            }
            ""
        }
    }

    fn get_loc(&mut self, start: usize, end: usize) -> Loc {
        let clamped = end.min(self.input.len());
        if self.input.get(start.min(clamped)..clamped).is_none() {
            self.give_up("a location doesn't fall on character boundaries");
        }
        Loc {
            start: self.get_pos(start),
            end: self.get_pos(end),
            source: Span {
                start: start.min(clamped),
                end: clamped,
            },
        }
    }

    /// `getLoc(start)`: a location whose end is attached later.
    fn get_loc_start(&self, start: usize) -> Loc {
        Loc {
            start: self.get_pos(start),
            end: Position::default(),
            source: Span { start, end: start },
        }
    }

    fn end_loc(&mut self, loc: Loc, end: usize) -> Loc {
        self.get_loc(loc.start.offset, end)
    }

    fn element(&mut self, id: NodeId) -> Option<&mut Element> {
        match self.nodes.get_mut(id.0) {
            Some(Node::Element(element)) => Some(element),
            _ => None,
        }
    }

    fn element_ref(&self, id: NodeId) -> Option<&Element> {
        match self.nodes.get(id.0) {
            Some(Node::Element(element)) => Some(element),
            _ => None,
        }
    }

    /// `stack[0]`: the innermost open element.
    fn parent(&self) -> Option<NodeId> {
        self.stack.last().copied()
    }

    fn parent_ns(&self) -> Namespace {
        self.parent()
            .and_then(|id| self.element_ref(id))
            .map_or(Namespace::Html, |element| element.ns)
    }

    fn children_of_parent(&mut self) -> &mut Vec<NodeId> {
        let parent = self.parent();
        match parent.and_then(|id| self.nodes.get_mut(id.0)) {
            Some(Node::Element(element)) => &mut element.children,
            _ => &mut self.root_children,
        }
    }

    fn add_node(&mut self, node: Node) {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        self.children_of_parent().push(id);
    }

    fn emit_error(&mut self, code: ErrorCode, index: usize) {
        let position = self.get_pos(index);
        self.errors.push(CompilerError { code, position });
    }

    fn ontext(&mut self, start: usize, end: usize) {
        let content = self.get_slice(start, end).to_owned();
        self.on_text(&content, start, end);
    }

    fn ontextentity(&mut self, char: &str, start: usize, end: usize) {
        self.on_text(char, start, end);
    }

    fn on_text(&mut self, content: &str, start: usize, end: usize) {
        let last = self.children_of_parent().last().copied();
        if let Some(Node::Text(text)) = last.and_then(|id| self.nodes.get(id.0)) {
            // Merged into the text before it.
            let loc = text.loc;
            let loc = self.end_loc(loc, end);
            if let Some(Node::Text(text)) = last.and_then(|id| self.nodes.get_mut(id.0)) {
                text.content.push_str(content);
                text.loc = loc;
            }
        } else {
            let loc = self.get_loc(start, end);
            self.add_node(Node::Text(Text {
                content: content.to_owned(),
                loc,
            }));
        }
    }

    fn oninterpolation(&mut self, start: usize, end: usize) {
        if self.in_v_pre {
            self.ontext(start, end);
            return;
        }
        let mut inner_start = start + DELIMITER_OPEN.len();
        let mut inner_end = end - DELIMITER_CLOSE.len();
        while self.byte(inner_start).is_some_and(is_whitespace) {
            inner_start += 1;
        }
        while inner_end > 0 && self.byte(inner_end - 1).is_some_and(is_whitespace) {
            inner_end -= 1;
        }
        let mut exp = self.get_slice(inner_start, inner_end).to_owned();
        // Entities are decoded, for backwards compatibility.
        if exp.contains('&') {
            exp = decode_html(&exp);
        }
        let mut content_loc = self.get_loc(inner_start.min(inner_end), inner_end);
        // An interpolation of only whitespace: the start is past the end.
        content_loc.start = self.get_pos(inner_start);
        let loc = self.get_loc(start, end);
        self.add_node(Node::Interpolation(Interpolation {
            content: create_simple_expression(exp, false, content_loc, ConstantType::NotConstant),
            loc,
        }));
    }

    fn onopentagname(&mut self, start: usize, end: usize) {
        let name = self.get_slice(start, end).to_owned();
        let ns = self.get_namespace(&name);
        let loc = self.get_loc(start.saturating_sub(1), end);
        // The element is created now and added to its parent when the open
        // tag ends; until then it's only reachable as the current open tag.
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node::Element(Element {
            tag: name,
            ns,
            // Refined when the tag closes.
            tag_type: ElementType::Element,
            props: Vec::new(),
            children: Vec::new(),
            loc,
            is_self_closing: false,
            inner_loc: None,
        }));
        self.current_open_tag = Some(id);
    }

    /// `@vue/compiler-dom`'s `getNamespace`.
    fn get_namespace(&self, tag: &str) -> Namespace {
        let parent = self.parent().and_then(|id| self.element_ref(id));
        let mut ns = parent.map_or(Namespace::Html, |element| element.ns);
        if let Some(parent) = parent {
            if ns == Namespace::MathMl {
                if parent.tag == "annotation-xml" {
                    if tag == "svg" {
                        return Namespace::Svg;
                    }
                    if parent.props.iter().any(|prop| {
                        matches!(prop, Prop::Attribute(attribute)
                        if attribute.name == "encoding"
                            && attribute.value.as_ref().is_some_and(|value| {
                                value.content == "text/html"
                                    || value.content == "application/xhtml+xml"
                            }))
                    }) {
                        ns = Namespace::Html;
                    }
                } else if matches!(parent.tag.as_str(), "mi" | "mo" | "mn" | "ms" | "mtext")
                    && tag != "mglyph"
                    && tag != "malignmark"
                {
                    ns = Namespace::Html;
                }
            } else if ns == Namespace::Svg
                && matches!(parent.tag.as_str(), "foreignObject" | "desc" | "title")
            {
                ns = Namespace::Html;
            }
        }
        if ns == Namespace::Html {
            if tag == "svg" {
                return Namespace::Svg;
            }
            if tag == "math" {
                return Namespace::MathMl;
            }
        }
        ns
    }

    fn is_pre_tag(&self, tag: &str) -> bool {
        self.options.every_tag_is_pre || tag == "pre"
    }

    fn onopentagend(&mut self, end: usize) {
        self.end_open_tag(end);
    }

    fn end_open_tag(&mut self, end: usize) {
        let Some(id) = self.current_open_tag.take() else {
            self.give_up("an open tag ended with no tag open");
            return;
        };
        if self.in_sfc_root() {
            // In a single-file component, a root block's content is located.
            let inner_loc = self.get_loc(end + 1, end + 1);
            if let Some(element) = self.element(id) {
                element.inner_loc = Some(inner_loc);
            }
        }
        self.children_of_parent().push(id);
        let Some((tag, ns)) = self
            .element_ref(id)
            .map(|element| (element.tag.clone(), element.ns))
        else {
            return;
        };
        if ns == Namespace::Html && self.is_pre_tag(&tag) {
            self.in_pre += 1;
        }
        if is_void_tag(&tag) {
            self.on_close_tag(id, end, false);
        } else {
            self.stack.push(id);
            if ns == Namespace::Svg || ns == Namespace::MathMl {
                self.in_xml = true;
            }
        }
    }

    fn onclosetag(&mut self, start: usize, end: usize) {
        let name = self.get_slice(start, end).to_owned();
        if is_void_tag(&name) {
            return;
        }
        let lower = name.to_lowercase();
        // From the innermost open element outwards.
        let found = self.stack.iter().rev().position(|&id| {
            self.element_ref(id)
                .is_some_and(|element| element.tag.to_lowercase() == lower)
        });
        if let Some(depth) = found {
            if depth > 0
                && let Some(offset) = self
                    .parent()
                    .and_then(|id| self.element_ref(id))
                    .map(|element| element.loc.start.offset)
            {
                self.emit_error(ErrorCode::MissingEndTag, offset);
            }
            for closed in 0..=depth {
                if let Some(id) = self.stack.pop() {
                    self.on_close_tag(id, end, closed < depth);
                }
            }
        } else {
            let index = self.back_track(start, b'<');
            self.emit_error(ErrorCode::InvalidEndTag, index);
        }
    }

    fn onselfclosingtag(&mut self, end: usize) {
        let Some(name) = self
            .current_open_tag
            .and_then(|id| self.element(id))
            .map(|element| {
                element.is_self_closing = true;
                element.tag.clone()
            })
        else {
            self.give_up("a tag self-closed with no tag open");
            return;
        };
        self.end_open_tag(end);
        let parent = self.parent();
        if parent
            .and_then(|id| self.element_ref(id))
            .is_some_and(|element| element.tag == name)
            && let Some(id) = self.stack.pop()
        {
            self.on_close_tag(id, end, false);
        }
    }

    fn onattribname(&mut self, start: usize, end: usize) {
        let name = self.get_slice(start, end).to_owned();
        let name_loc = self.get_loc(start, end);
        self.current_prop = Some(Prop::Attribute(Attribute {
            name,
            name_loc,
            value: None,
            loc: self.get_loc_start(start),
        }));
    }

    fn ondirname(&mut self, start: usize, end: usize) {
        let raw = self.get_slice(start, end).to_owned();
        let name = match raw.as_str() {
            "." | ":" => "bind",
            "@" => "on",
            "#" => "slot",
            // `v-` and what follows it.
            other => other.get(2..).unwrap_or(""),
        }
        .to_owned();
        if !self.in_v_pre && name.is_empty() {
            self.emit_error(ErrorCode::MissingDirectiveName, start);
        }
        if self.in_v_pre || name.is_empty() {
            let name_loc = self.get_loc(start, end);
            self.current_prop = Some(Prop::Attribute(Attribute {
                name: raw,
                name_loc,
                value: None,
                loc: self.get_loc_start(start),
            }));
            return;
        }
        let is_pre = name == "pre";
        self.current_prop = Some(Prop::Directive(Box::new(Directive {
            name,
            modifiers: if raw == "." {
                vec![create_simple_expression(
                    "prop".to_owned(),
                    false,
                    loc_stub(),
                    ConstantType::NotConstant,
                )]
            } else {
                Vec::new()
            },
            raw_name: raw,
            exp: None,
            arg: None,
            for_parse_result: None,
            loc: self.get_loc_start(start),
        })));
        if is_pre {
            self.in_v_pre = true;
            self.current_v_pre_boundary = self.current_open_tag;
            // The directives before this one become attributes.
            let Some(id) = self.current_open_tag else {
                return;
            };
            let props = self
                .element(id)
                .map(|element| std::mem::take(&mut element.props))
                .unwrap_or_default();
            let props = props
                .into_iter()
                .map(|prop| match prop {
                    Prop::Directive(directive) => Prop::Attribute(self.dir_to_attr(*directive)),
                    attribute @ Prop::Attribute(_) => attribute,
                })
                .collect();
            if let Some(element) = self.element(id) {
                element.props = props;
            }
        }
    }

    fn ondirarg(&mut self, start: usize, end: usize) {
        if start == end {
            return;
        }
        let arg = self.get_slice(start, end).to_owned();
        let arg_loc = self.get_loc(start, end);
        let in_v_pre = self.in_v_pre;
        match self.current_prop.as_mut() {
            Some(Prop::Attribute(attribute)) if in_v_pre => {
                attribute.name.push_str(&arg);
                let name_loc = attribute.name_loc;
                let name_loc = self.end_loc(name_loc, end);
                if let Some(Prop::Attribute(attribute)) = self.current_prop.as_mut() {
                    attribute.name_loc = name_loc;
                }
            }
            Some(Prop::Directive(directive)) => {
                let is_static = !arg.starts_with('[');
                let content = if is_static {
                    arg
                } else {
                    // `arg.slice(1, -1)`: without its first and last character.
                    let mut inner = arg.chars();
                    inner.next();
                    let last = inner.next_back();
                    let inner = inner.as_str().to_owned();
                    if last.is_some_and(|char| char.len_utf16() > 1) {
                        self.unsupported
                            .get_or_insert("a dynamic argument ends with half a character");
                    }
                    inner
                };
                directive.arg = Some(create_simple_expression(
                    content,
                    is_static,
                    arg_loc,
                    ConstantType::NotConstant,
                ));
            }
            // An argument on a directive with no name (`v-:arg`), which the
            // original stores on the attribute where nothing reads it.
            Some(Prop::Attribute(_)) | None => {}
        }
    }

    fn ondirmodifier(&mut self, start: usize, end: usize) {
        let modifier = self.get_slice(start, end).to_owned();
        let loc = self.get_loc(start, end);
        let in_v_pre = self.in_v_pre;
        match self.current_prop.as_mut() {
            Some(Prop::Attribute(attribute)) if in_v_pre => {
                attribute.name.push('.');
                attribute.name.push_str(&modifier);
                let name_loc = attribute.name_loc;
                let name_loc = self.end_loc(name_loc, end);
                if let Some(Prop::Attribute(attribute)) = self.current_prop.as_mut() {
                    attribute.name_loc = name_loc;
                }
            }
            Some(Prop::Directive(directive)) if directive.name == "slot" => {
                // A slot has no modifiers: the name goes on.
                if let Some(arg) = directive.arg.as_mut() {
                    arg.content.push('.');
                    arg.content.push_str(&modifier);
                    let arg_loc = arg.loc;
                    let arg_loc = self.end_loc(arg_loc, end);
                    if let Some(Prop::Directive(directive)) = self.current_prop.as_mut()
                        && let Some(arg) = directive.arg.as_mut()
                    {
                        arg.loc = arg_loc;
                    }
                }
            }
            Some(Prop::Directive(directive)) => {
                directive.modifiers.push(create_simple_expression(
                    modifier,
                    true,
                    loc,
                    ConstantType::NotConstant,
                ));
            }
            Some(Prop::Attribute(_)) | None => {
                // A modifier on a directive with no name (`v-.modifier`): the
                // original throws here and the template has no tree.
                self.throws
                    .get_or_insert("a modifier on a directive that has no name");
            }
        }
    }

    fn onattribdata(&mut self, start: usize, end: usize) {
        let data = self.get_slice(start, end).to_owned();
        self.current_attr_value.push_str(&data);
        if self.current_attr_start_index.is_none() {
            self.current_attr_start_index = Some(start);
        }
        self.current_attr_end_index = Some(end);
    }

    fn onattribentity(&mut self, char: &str, start: usize, end: usize) {
        self.current_attr_value.push_str(char);
        if self.current_attr_start_index.is_none() {
            self.current_attr_start_index = Some(start);
        }
        self.current_attr_end_index = Some(end);
    }

    fn onattribnameend(&mut self, end: usize) {
        let Some(start) = self
            .current_prop
            .as_ref()
            .map(|prop| prop.loc().start.offset)
        else {
            return;
        };
        let name = self.get_slice(start, end).to_owned();
        let duplicate = self
            .current_open_tag
            .and_then(|id| self.element_ref(id))
            .is_some_and(|element| element.props.iter().any(|prop| prop.written_name() == name));
        if let Some(Prop::Directive(directive)) = self.current_prop.as_mut() {
            directive.raw_name = name;
        }
        if duplicate {
            self.emit_error(ErrorCode::DuplicateAttribute, start);
        }
    }

    fn onattribend(&mut self, quote: QuoteType, end: usize) {
        if let (Some(open_tag), Some(mut prop)) = (self.current_open_tag, self.current_prop.take())
        {
            let loc = *prop.loc();
            *prop.loc_mut() = self.end_loc(loc, end);
            if quote != QuoteType::NoValue {
                let value = std::mem::take(&mut self.current_attr_value);
                let (Some(value_start), Some(value_end)) =
                    (self.current_attr_start_index, self.current_attr_end_index)
                else {
                    self.give_up("an attribute value ended with no value read");
                    return;
                };
                match &mut prop {
                    Prop::Attribute(attribute) => {
                        self.finish_attribute(attribute, value, quote, value_start, value_end, end);
                    }
                    Prop::Directive(directive) => {
                        let loc = self.get_loc(value_start, value_end);
                        // The codegen locates the parts of an expression by
                        // adding offsets in its text to where it starts.
                        // After an entity was decoded the text and the source
                        // no longer line up, and the original's offsets are
                        // then counted in UTF-16 units of the decoded text.
                        if !value.is_ascii()
                            && self.input.get(value_start..value_end) != Some(&value)
                        {
                            self.decoded.push(DecodedValue {
                                start: value_start,
                                end: value_end,
                                text: value.clone(),
                            });
                        }
                        let exp =
                            create_simple_expression(value, false, loc, ConstantType::NotConstant);
                        if directive.name == "for" {
                            directive.for_parse_result = self.parse_for_expression(&exp);
                        }
                        directive.exp = Some(exp);
                    }
                }
            }
            let is_v_pre = matches!(&prop, Prop::Directive(directive) if directive.name == "pre");
            if !is_v_pre && let Some(element) = self.element(open_tag) {
                element.props.push(prop);
            }
        }
        self.current_attr_value.clear();
        self.current_attr_start_index = None;
        self.current_attr_end_index = None;
    }

    fn finish_attribute(
        &mut self,
        attribute: &mut Attribute,
        mut value: String,
        quote: QuoteType,
        value_start: usize,
        value_end: usize,
        end: usize,
    ) {
        // Whitespace in a class is condensed.
        if attribute.name == "class" {
            let condensed = condense(&value);
            js::trim(&condensed).clone_into(&mut value);
        }
        if quote == QuoteType::Unquoted && value.is_empty() {
            self.emit_error(ErrorCode::MissingAttributeValue, end);
        }
        let loc = if quote == QuoteType::Unquoted {
            self.get_loc(value_start, value_end)
        } else {
            // With its quotes.
            self.get_loc(value_start.saturating_sub(1), value_end + 1)
        };
        let is_template_lang = self.in_sfc_root()
            && attribute.name == "lang"
            && !value.is_empty()
            && value != "html"
            && self
                .current_open_tag
                .and_then(|id| self.element_ref(id))
                .is_some_and(|element| element.tag == "template");
        attribute.value = Some(Text {
            content: value,
            loc,
        });
        if is_template_lang {
            // A root <template> in another language is raw text.
            self.enter_rcdata(Sequence::Custom(b"</template".to_vec()), 0);
        }
    }

    fn oncomment(&mut self, start: usize, end: usize) {
        let content = self.get_slice(start, end).to_owned();
        // With its `<!--` and `-->`: four characters before, three after. The
        // comment an invalid closing tag becomes (`</#>`) gets the same seven,
        // whatever they are, counted by the original in UTF-16 units.
        let (Some(loc_start), Some(loc_end)) =
            (self.units_before(start, 4), self.units_after(end, 3))
        else {
            self.give_up("a comment's location is outside the text or inside a character");
            return;
        };
        let loc = self.get_loc(loc_start, loc_end);
        self.add_node(Node::Comment(Text { content, loc }));
    }

    /// The offset `units` UTF-16 units before `index`.
    fn units_before(&self, index: usize, units: usize) -> Option<usize> {
        let mut remaining = units;
        let mut offset = index;
        for char in self.input.get(..index)?.chars().rev() {
            if remaining == 0 {
                break;
            }
            remaining = remaining.checked_sub(char.len_utf16())?;
            offset -= char.len_utf8();
        }
        (remaining == 0).then_some(offset)
    }

    /// The offset `units` UTF-16 units after `index`. Past the end of the
    /// text, each unit counts as one more, as it does for the original.
    fn units_after(&self, index: usize, units: usize) -> Option<usize> {
        let mut remaining = units;
        let mut offset = index;
        for char in self.input.get(index..)?.chars() {
            if remaining == 0 {
                break;
            }
            remaining = remaining.checked_sub(char.len_utf16())?;
            offset += char.len_utf8();
        }
        Some(offset + remaining)
    }

    fn onend(&mut self) {
        let end = self.input.len();
        match self.state {
            State::BeforeTagName | State::BeforeClosingTagName => {
                self.emit_error(ErrorCode::EofBeforeTagName, end);
            }
            State::Interpolation | State::InterpolationClose => {
                let start = self.section();
                self.emit_error(ErrorCode::MissingInterpolationEnd, start);
            }
            State::InCommentLike => {
                if self.current_sequence == Sequence::CdataEnd {
                    self.emit_error(ErrorCode::EofInCdata, end);
                } else {
                    self.emit_error(ErrorCode::EofInComment, end);
                }
            }
            State::InTagName
            | State::InSelfClosingTag
            | State::InClosingTagName
            | State::BeforeAttrName
            | State::InAttrName
            | State::InDirName
            | State::InDirArg
            | State::InDirDynamicArg
            | State::InDirModifier
            | State::AfterAttrName
            | State::BeforeAttrValue
            | State::InAttrValueDq
            | State::InAttrValueSq
            | State::InAttrValueNq => self.emit_error(ErrorCode::EofInTag, end),
            _ => {}
        }
        // Every element still open is closed, innermost first. They stay on
        // the stack, so none of them is treated as a root block.
        for id in self.stack.clone().into_iter().rev() {
            self.on_close_tag(id, end.saturating_sub(1), false);
            if let Some(offset) = self.element_ref(id).map(|element| element.loc.start.offset) {
                self.emit_error(ErrorCode::MissingEndTag, offset);
            }
        }
    }

    fn oncdata(&mut self, start: usize, end: usize) {
        if self.options.cdata_outside_element_throws && self.stack.is_empty() {
            self.throws
                .get_or_insert("a CDATA section outside any element");
            return;
        }
        if self.parent_ns() == Namespace::Html {
            // From the `<![CDATA[`.
            self.emit_error(ErrorCode::CdataInHtmlContent, start.saturating_sub(9));
        } else {
            self.ontext(start, end);
        }
    }

    fn onprocessinginstruction(&mut self, start: usize) {
        if self.parent_ns() == Namespace::Html {
            self.emit_error(
                ErrorCode::UnexpectedQuestionMarkInsteadOfTagName,
                start.saturating_sub(1),
            );
        }
    }

    fn on_close_tag(&mut self, id: NodeId, end: usize, is_implied: bool) {
        let Some(loc) = self.element_ref(id).map(|element| element.loc) else {
            return;
        };
        // An implied close ends where the tag that closed it starts.
        let loc_end = if is_implied {
            self.back_track(end, b'<')
        } else {
            self.look_ahead(end, b'>') + 1
        };
        let loc = self.end_loc(loc, loc_end);
        if let Some(element) = self.element(id) {
            element.loc = loc;
        }

        if self.in_sfc_root() {
            // A root block: where its content ends.
            let last_child_end = self
                .element_ref(id)
                .and_then(|element| element.children.last().copied())
                .and_then(|child| self.nodes.get(child.0))
                .map(|child| child.loc().end);
            if let Some(inner_loc) = self.element_ref(id).and_then(|element| element.inner_loc) {
                let inner_end = last_child_end.unwrap_or(inner_loc.start);
                let mut inner_loc = self.get_loc(inner_loc.start.offset, inner_end.offset);
                inner_loc.end = inner_end;
                if let Some(element) = self.element(id) {
                    element.inner_loc = Some(inner_loc);
                }
            } else {
                self.give_up("a root block closed that has no content location");
            }
        }

        // The element's type.
        if !self.in_v_pre {
            let tag_type = self.element_ref(id).and_then(|element| {
                if element.tag == "slot" {
                    Some(ElementType::Slot)
                } else if is_fragment_template(element) {
                    Some(ElementType::Template)
                } else if self.is_component(element) {
                    Some(ElementType::Component)
                } else {
                    None
                }
            });
            if let (Some(tag_type), Some(element)) = (tag_type, self.element(id)) {
                element.tag_type = tag_type;
            }
        }

        let Some((tag, ns, children)) = self
            .element_ref(id)
            .map(|element| (element.tag.clone(), element.ns, element.children.clone()))
        else {
            return;
        };

        // Whitespace. The original removes a child by emptying its slot in
        // the list it then replaces, and reads the first slot again below.
        let mut first_child = children.first().copied();
        if !self.in_rcdata {
            let (condensed, first_was_removed) = self.condense_whitespace(children);
            if first_was_removed {
                first_child = None;
            }
            if let Some(element) = self.element(id) {
                element.children = condensed;
            }
        }

        if ns == Namespace::Html && is_ignore_newline_tag(&tag) {
            // The newline that starts a <textarea> or a <pre> isn't content.
            if let Some(Node::Text(text)) =
                first_child.and_then(|child| self.nodes.get_mut(child.0))
            {
                let stripped = text
                    .content
                    .strip_prefix("\r\n")
                    .or_else(|| text.content.strip_prefix('\n'))
                    .map(str::to_owned);
                if let Some(stripped) = stripped {
                    text.content = stripped;
                }
            }
        }

        if ns == Namespace::Html && self.is_pre_tag(&tag) {
            self.in_pre = self.in_pre.saturating_sub(1);
        }
        if self.current_v_pre_boundary == Some(id) {
            self.in_v_pre = false;
            self.current_v_pre_boundary = None;
        }
        if self.in_xml && self.parent_ns() == Namespace::Html {
            self.in_xml = false;
        }
    }

    fn look_ahead(&self, index: usize, char: u8) -> usize {
        let last = self.bytes.len().saturating_sub(1);
        let mut i = index;
        while self.byte(i) != Some(char) && i < last {
            i += 1;
        }
        i
    }

    fn back_track(&mut self, index: usize, char: u8) -> usize {
        let mut i = index;
        loop {
            if self.byte(i) == Some(char) {
                return i;
            }
            let Some(previous) = i.checked_sub(1) else {
                // The original returns -1 here.
                self.give_up("a location is before the start of the text");
                return 0;
            };
            i = previous;
        }
    }

    fn is_component(&self, element: &Element) -> bool {
        let tag = element.tag.as_str();
        if tag == "component"
            || tag.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
            || is_core_component(tag)
            || is_built_in_component(tag)
            || (!self.options.every_tag_is_native && !is_dom_tag(tag))
        {
            return true;
        }
        // A native tag cast to a component: `is="vue:name"`.
        element.props.iter().any(|prop| {
            matches!(prop, Prop::Attribute(attribute)
                if attribute.name == "is"
                    && attribute.value.as_ref().is_some_and(|value| value.content.starts_with("vue:")))
        })
    }

    /// Removes or condenses whitespace-only text between nodes. Also returns
    /// whether the first node was removed.
    fn condense_whitespace(&mut self, nodes: Vec<NodeId>) -> (Vec<NodeId>, bool) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum Kind {
            Element,
            Comment,
            Text,
            Other,
        }
        let kinds: Vec<Kind> = nodes
            .iter()
            .map(|id| match self.nodes.get(id.0) {
                Some(Node::Element(_)) => Kind::Element,
                Some(Node::Comment(_)) => Kind::Comment,
                Some(Node::Text(_)) => Kind::Text,
                Some(_) | None => Kind::Other,
            })
            .collect();
        let mut removed = vec![false; nodes.len()];
        for (i, id) in nodes.iter().enumerate() {
            let Some(Node::Text(text)) = self.nodes.get_mut(id.0) else {
                continue;
            };
            if self.in_pre > 0 {
                // Windows newlines are normalized in <pre>, as a browser does.
                text.content = text.content.replace("\r\n", "\n");
            } else if is_all_whitespace(&text.content) {
                // A removed node counts as no node for the one after it.
                let previous = i
                    .checked_sub(1)
                    .filter(|&previous| !removed.get(previous).copied().unwrap_or(false))
                    .and_then(|previous| kinds.get(previous).copied());
                let next = kinds.get(i + 1).copied();
                // Removed when it is the first or last node, between two
                // comments, between a comment and an element, or between two
                // elements with a newline in it.
                let remove = match (previous, next) {
                    (None, _)
                    | (_, None)
                    | (Some(Kind::Comment), Some(Kind::Comment | Kind::Element))
                    | (Some(Kind::Element), Some(Kind::Comment)) => true,
                    (Some(Kind::Element), Some(Kind::Element)) => has_newline_char(&text.content),
                    _ => false,
                };
                if !remove {
                    " ".clone_into(&mut text.content);
                } else if let Some(flag) = removed.get_mut(i) {
                    *flag = true;
                }
            } else {
                text.content = condense(&text.content);
            }
        }
        let first_was_removed = removed.first().copied().unwrap_or(false);
        let kept = nodes
            .into_iter()
            .zip(removed)
            .filter_map(|(id, removed)| (!removed).then_some(id))
            .collect();
        (kept, first_was_removed)
    }

    fn dir_to_attr(&mut self, directive: Directive) -> Attribute {
        let name_end = directive.loc.start.offset + directive.raw_name.len();
        let name_loc = self.get_loc(directive.loc.start.offset, name_end);
        let value = directive.exp.map(|exp| {
            let mut loc = exp.loc;
            // An attribute's value isn't an expression: nothing is located
            // by its decoded text.
            self.decoded
                .retain(|value| value.start != exp.loc.source.start);
            // With its quotes, when it has them. The text isn't retaken.
            if loc.end.offset < directive.loc.end.offset {
                loc.start.offset = loc.start.offset.saturating_sub(1);
                loc.end.offset += 1;
            }
            Text {
                content: exp.content,
                loc,
            }
        });
        Attribute {
            name: directive.raw_name,
            name_loc,
            value,
            loc: directive.loc,
        }
    }

    /// Splits `v-for`'s expression: `(value, key, index) in source`.
    fn parse_for_expression(&mut self, input: &SimpleExpression) -> Option<ForParseResult> {
        let exp = input.content.as_str();
        let (lhs, rhs) = match_for_alias(exp)?;
        // The parts are found in the expression's text and located by adding
        // those offsets to where it starts. After an entity was decoded the
        // text and the source no longer line up, and the original's offsets
        // are then counted in UTF-16 units of the decoded text.
        let is_source = self.input.get(input.loc.source.start..input.loc.source.end) == Some(exp);
        if !is_source && !exp.is_ascii() {
            self.give_up("a v-for expression with an entity and characters outside ASCII");
        }
        let base = input.loc.start.offset;
        let alias = |parser: &mut Self, content: &str, offset: Option<usize>| {
            let Some(offset) = offset else {
                // The original adds -1 here.
                parser.give_up("a part of a v-for expression wasn't found in it");
                return create_simple_expression(
                    content.to_owned(),
                    false,
                    loc_stub(),
                    ConstantType::NotConstant,
                );
            };
            let start = base + offset;
            let loc = parser.get_loc(start, start + content.len());
            create_simple_expression(content.to_owned(), false, loc, ConstantType::NotConstant)
        };

        let source = alias(self, js::trim(rhs), js::index_of(exp, rhs, lhs.len()));
        let mut result = ForParseResult {
            source,
            value: None,
            key: None,
            index: None,
        };

        let mut value_content = js::trim(strip_parens(js::trim(lhs)));
        let trimmed_offset = lhs.find(value_content);

        if let Some((before, key, index)) = match_for_iterator(value_content) {
            value_content = js::trim(before);
            let after_value = trimmed_offset.map(|offset| offset + value_content.len());
            let key_content = js::trim(key);
            let mut key_offset = None;
            if !key_content.is_empty() {
                key_offset = after_value.and_then(|from| js::index_of(exp, key_content, from));
                result.key = Some(alias(self, key_content, key_offset));
            }
            if let Some(index) = index.filter(|index| !index.is_empty()) {
                let index_content = js::trim(index);
                if !index_content.is_empty() {
                    let from = if result.key.is_some() {
                        key_offset.map(|offset| offset + key_content.len())
                    } else {
                        after_value
                    };
                    let offset = from.and_then(|from| js::index_of(exp, index_content, from));
                    result.index = Some(alias(self, index_content, offset));
                }
            }
        }

        if !value_content.is_empty() {
            result.value = Some(alias(self, value_content, trimmed_offset));
        }
        Some(result)
    }
}

/// `forAliasRE`: `/([\s\S]*?)\s+(?:in|of)\s+(\S[\s\S]*)/`. Returns what is
/// before the ` in ` or ` of `, and what is after it.
fn match_for_alias(exp: &str) -> Option<(&str, &str)> {
    for (start, char) in exp.char_indices() {
        if !js::is_whitespace(char) {
            continue;
        }
        // The whitespace has to reach the keyword: `\s+` can't stop early,
        // since what follows it must be a letter.
        let after_space = js::trim_start(exp.get(start..)?);
        let Some(after_keyword) = after_space
            .strip_prefix("in")
            .or_else(|| after_space.strip_prefix("of"))
        else {
            continue;
        };
        let rhs = js::trim_start(after_keyword);
        if rhs.len() < after_keyword.len() && !rhs.is_empty() {
            return Some((exp.get(..start)?, rhs));
        }
    }
    None
}

/// `stripParensRE`: `/^\(|\)$/g`.
fn strip_parens(text: &str) -> &str {
    let text = text.strip_prefix('(').unwrap_or(text);
    text.strip_suffix(')').unwrap_or(text)
}

/// `forIteratorRE`: `/,([^,\}\]]*)(?:,([^,\}\]]*))?$/`. Returns what is
/// before the match, and its one or two groups.
fn match_for_iterator(text: &str) -> Option<(&str, &str, Option<&str>)> {
    let is_stop = |char: char| matches!(char, ',' | '}' | ']');
    for (comma, _) in text.match_indices(',') {
        let rest = text.get(comma + 1..)?;
        let first_end = rest.find(is_stop).unwrap_or(rest.len());
        let first = rest.get(..first_end)?;
        let after_first = rest.get(first_end..)?;
        if after_first.is_empty() {
            return Some((text.get(..comma)?, first, None));
        }
        if let Some(second) = after_first.strip_prefix(',')
            && !second.contains(is_stop)
        {
            return Some((text.get(..comma)?, first, Some(second)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{match_for_alias, match_for_iterator, strip_parens};

    #[test]
    fn for_alias() {
        assert_eq!(match_for_alias("item in items"), Some(("item", "items")));
        assert_eq!(
            match_for_alias("(a, b)  of  list in x"),
            Some(("(a, b)", "list in x"))
        );
        assert_eq!(match_for_alias("in in in"), Some(("in", "in")));
        assert_eq!(match_for_alias("item in "), None);
        assert_eq!(match_for_alias("iteminitems"), None);
    }

    #[test]
    fn for_iterator() {
        assert_eq!(match_for_iterator("a, b"), Some(("a", " b", None)));
        assert_eq!(match_for_iterator("a, b, c"), Some(("a", " b", Some(" c"))));
        assert_eq!(
            match_for_iterator("{ a, b }, c"),
            Some(("{ a, b }", " c", None))
        );
        assert_eq!(match_for_iterator("{ a, b }"), None);
        assert_eq!(strip_parens("(a, b)"), "a, b");
    }
}
