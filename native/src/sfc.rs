//! A single-file component split into its blocks: a port of
//! `@vue/language-core`'s `utils/parseSfc.ts` and the `file-vue` plugin's
//! `parseSFC2` (3.3.14).

use crate::js;
use crate::template::ast::{Attribute, Element, ErrorCode, Node, Position, Prop, Text};
use crate::template::parser::{ParseOptions, Parsed, parse};

/// An attribute's value as written (quotes removed), and where it starts.
/// The original keeps that as a distance from the start of the block's tag
/// ([`Block::tag_start`]); a distance in bytes isn't one in UTF-16 units, so
/// the position itself is kept here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockAttr {
    pub text: String,
    pub position: usize,
}

/// An attribute's decoded value, as the original keeps it: `true` for an
/// attribute with no value or an empty one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrValue {
    True,
    Text(String),
}

/// `parseAttr`: an attribute with no value is `true`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttrOrTrue {
    True,
    Attr(BlockAttr),
}

#[derive(Clone, Debug)]
pub struct Block {
    /// The tag: `template`, `script`, `style`, or a custom block's.
    pub tag: String,
    /// Where the block's tag starts (its `<`).
    pub tag_start: usize,
    pub content: String,
    /// Where the content starts and ends.
    pub start: Position,
    pub end: Position,
    pub lang: Option<String>,
    /// Each attribute and its value, in order.
    pub attrs: Vec<(String, AttrValue)>,
    pub src: Option<AttrOrTrue>,
    /// `<script setup>`: the attribute's value.
    pub setup: Option<AttrValue>,
    pub generic: Option<AttrOrTrue>,
    pub module: Option<BlockAttr>,
    /// `module` as it is written: with a value (which may be empty), or none.
    pub module_written: Option<AttrOrTrue>,
    pub scoped: bool,
}

#[derive(Debug, Default)]
pub struct Descriptor {
    pub comments: Vec<String>,
    pub template: Option<Block>,
    pub script: Option<Block>,
    pub script_setup: Option<Block>,
    pub styles: Vec<Block>,
    pub custom_blocks: Vec<Block>,
}

#[derive(Debug)]
pub struct ParsedSfc {
    pub descriptor: Descriptor,
    pub parsed: Parsed,
    pub unsupported: Option<&'static str>,
}

/// `normalizeAttributeValue`: a value's source without its quotes, and where
/// that starts.
fn normalize_attribute_value<'a>(value: &Text, source: &'a str) -> (&'a str, usize) {
    let text = source
        .get(value.loc.source.start..value.loc.source.end)
        .unwrap_or("");
    let quoted = |quote: char| text.starts_with(quote) && text.ends_with(quote);
    if quoted('"') || quoted('\'') {
        // `slice(1, -1)`: a lone quote is both the start and the end.
        let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or("");
        (inner, value.loc.start.offset + 1)
    } else {
        (text, value.loc.start.offset)
    }
}

fn parse_attr(value: Option<&Text>, source: &str) -> AttrOrTrue {
    let Some(value) = value else {
        return AttrOrTrue::True;
    };
    let (text, offset) = normalize_attribute_value(value, source);
    AttrOrTrue::Attr(BlockAttr {
        text: text.to_owned(),
        position: offset,
    })
}

/// One attribute of a block's tag.
fn read_attribute(
    block: &mut Block,
    attribute: &Attribute,
    source: &str,
    unsupported: &mut Option<&'static str>,
) {
    let name = attribute.name.as_str();
    // `p.value ? p.value.content || true : true`
    let value = attribute
        .value
        .as_ref()
        .map(|value| value.content.clone())
        .filter(|content| !content.is_empty())
        .map_or(AttrValue::True, AttrValue::Text);
    // The original keeps attributes as an object's keys, which JavaScript
    // orders differently when a key is a whole number.
    if name.bytes().all(|char| char.is_ascii_digit()) {
        unsupported.get_or_insert("a block attribute named with a number");
    }
    if let Some(existing) = block.attrs.iter_mut().find(|(key, _)| key == name) {
        existing.1.clone_from(&value);
    } else {
        block.attrs.push((name.to_owned(), value.clone()));
    }
    let is_script = block.tag == "script";
    let is_style = block.tag == "style";
    if name == "lang" {
        block.lang = attribute.value.as_ref().map(|value| value.content.clone());
    } else if name == "src" {
        block.src = Some(parse_attr(attribute.value.as_ref(), source));
    } else if is_script {
        match name {
            "vapor" => {
                block.setup.get_or_insert(value);
                block.generic.get_or_insert(AttrOrTrue::True);
            }
            "setup" => block.setup = Some(value),
            "generic" => {
                block.generic = Some(parse_attr(attribute.value.as_ref(), source));
            }
            _ => {}
        }
    } else if is_style {
        if name == "scoped" {
            block.scoped = true;
        } else if name == "module" {
            block.module_written = Some(parse_attr(attribute.value.as_ref(), source));
            // An empty name is the default `$style`, at the attribute.
            block.module = Some(match parse_attr(attribute.value.as_ref(), source) {
                AttrOrTrue::Attr(attr) if !attr.text.is_empty() => attr,
                AttrOrTrue::Attr(_) | AttrOrTrue::True => BlockAttr {
                    text: String::new(),
                    position: attribute.loc.start.offset,
                },
            });
        }
    }
}

fn create_block(
    node: &Element,
    parsed: &Parsed,
    source: &str,
    unsupported: &mut Option<&'static str>,
) -> Block {
    let mut start = node.loc.start;
    let end;
    let mut content = "";
    let loc_end =
        |id: &crate::template::ast::NodeId| parsed.root.node(*id).map(|child| *child.loc());
    if let (Some(first), Some(last)) = (
        node.children.first().and_then(loc_end),
        node.children.last().and_then(loc_end),
    ) {
        start = first.start;
        end = last.end;
        content = source.get(start.offset..end.offset).unwrap_or_else(|| {
            unsupported.get_or_insert("a block's content doesn't fall on character boundaries");
            ""
        });
    } else {
        let node_source = source
            .get(node.loc.source.start..node.loc.source.end)
            .unwrap_or("");
        if let Some(offset) = node_source.find("</") {
            start = Position {
                offset: start.offset + offset,
                line: start.line,
            };
        }
        end = start;
    }

    let mut block = Block {
        tag: node.tag.clone(),
        tag_start: node.loc.start.offset,
        content: content.to_owned(),
        start,
        end,
        lang: None,
        attrs: Vec::new(),
        src: None,
        setup: None,
        generic: None,
        module: None,
        module_written: None,
        scoped: false,
    };
    for prop in &node.props {
        if let Prop::Attribute(attribute) = prop {
            read_attribute(&mut block, attribute, source, unsupported);
        }
    }
    block
}

/// `parseSfc.ts`'s `parse`, then the fix `file-vue` applies to its result.
pub fn parse_sfc(source: &str) -> ParsedSfc {
    let parsed = parse(source, ParseOptions::sfc());
    let mut unsupported = parsed.unsupported;
    let mut descriptor = Descriptor::default();
    for id in &parsed.root.children {
        let node = match parsed.root.node(*id) {
            Some(Node::Comment(comment)) => {
                descriptor.comments.push(comment.content.clone());
                continue;
            }
            Some(Node::Element(element)) => element,
            _ => continue,
        };
        let block = create_block(node, &parsed, source, &mut unsupported);
        match node.tag.as_str() {
            "template" => descriptor.template = Some(block),
            "script" => {
                // The first of each kind is kept.
                let slot = if block.setup.is_some() {
                    &mut descriptor.script_setup
                } else {
                    &mut descriptor.script
                };
                if slot.is_none() {
                    *slot = Some(block);
                }
            }
            "style" => descriptor.styles.push(block),
            _ => descriptor.custom_blocks.push(block),
        }
    }

    // A <template> that is missing its end tag also holds the end tag being
    // typed (`</templ`), which isn't content (vuejs/language-tools#4893).
    for error in &parsed.root.errors {
        let Some(template) = descriptor.template.as_mut() else {
            break;
        };
        if error.code != ErrorCode::MissingEndTag || error.position.line != template.start.line {
            continue;
        }
        let text = template.content.as_str();
        // With no `<`, the original's -1 means the last character.
        let (end_tag_offset, end_tag_text) = match text.rfind('<') {
            Some(offset) => (Some(offset), text.get(offset..).unwrap_or("")),
            None => (
                None,
                text.char_indices()
                    .next_back()
                    .and_then(|(last, _)| text.get(last..))
                    .unwrap_or(""),
            ),
        };
        if !"</template>".starts_with(js::trim_end(end_tag_text)) {
            continue;
        }
        match end_tag_offset {
            Some(offset) => {
                template.end.offset = template.start.offset + offset;
                template.content.truncate(offset);
            }
            None => {
                // The original ends the block one before its start here and
                // drops the content's last UTF-16 unit.
                unsupported.get_or_insert("a template with no end tag and no tag in it");
            }
        }
    }

    ParsedSfc {
        descriptor,
        parsed,
        unsupported,
    }
}
