//! The parser's results as JSON, in the shape `native/oracle/dump.ts` writes
//! the original's, so the two can be compared value for value. Offsets are
//! converted to UTF-16 here.

use serde_json::{Value, json};

use crate::sfc::{AttrOrTrue, AttrValue, Block, BlockAttr, ParsedSfc};
use crate::template::ast::{
    Attribute, ConstantType, Directive, ElementType, Loc, Namespace, Node, NodeId, Prop, Root,
    SimpleExpression, Text,
};
use crate::template::parser::Parsed;
use crate::utf16::Utf16Map;

struct Dumper<'a> {
    source: &'a str,
    map: Utf16Map,
}

impl Dumper<'_> {
    fn loc(&self, loc: &Loc) -> Value {
        json!([
            self.map.offset(loc.start.offset),
            loc.start.line,
            self.map.offset(loc.end.offset),
            loc.end.line,
            self.source
                .get(loc.source.start..loc.source.end)
                .unwrap_or(""),
        ])
    }

    fn expression(&self, expression: &SimpleExpression) -> Value {
        json!({
            "type": 4,
            "content": expression.content,
            "isStatic": expression.is_static,
            "constType": match expression.const_type {
                ConstantType::NotConstant => 0,
                ConstantType::CanStringify => 3,
            },
            "loc": self.loc(&expression.loc),
        })
    }

    fn text(&self, kind: u8, text: &Text) -> Value {
        json!({ "type": kind, "content": text.content, "loc": self.loc(&text.loc) })
    }

    fn attribute(&self, attribute: &Attribute) -> Value {
        json!({
            "type": 6,
            "name": attribute.name,
            "nameLoc": self.loc(&attribute.name_loc),
            "value": attribute.value.as_ref().map(|value| self.text(2, value)),
            "loc": self.loc(&attribute.loc),
        })
    }

    fn directive(&self, directive: &Directive) -> Value {
        let optional = |expression: &Option<SimpleExpression>| {
            expression
                .as_ref()
                .map(|expression| self.expression(expression))
        };
        json!({
            "type": 7,
            "name": directive.name,
            "rawName": directive.raw_name,
            "exp": optional(&directive.exp),
            "arg": optional(&directive.arg),
            "modifiers": directive
                .modifiers
                .iter()
                .map(|modifier| self.expression(modifier))
                .collect::<Vec<_>>(),
            "forParseResult": directive.for_parse_result.as_ref().map(|result| json!({
                "source": self.expression(&result.source),
                "value": optional(&result.value),
                "key": optional(&result.key),
                "index": optional(&result.index),
            })),
            "loc": self.loc(&directive.loc),
        })
    }

    fn node(&self, root: &Root, id: NodeId) -> Value {
        match root.node(id) {
            Some(Node::Element(element)) => json!({
                "type": 1,
                "tag": element.tag,
                "ns": match element.ns {
                    Namespace::Html => 0,
                    Namespace::Svg => 1,
                    Namespace::MathMl => 2,
                },
                "tagType": match element.tag_type {
                    ElementType::Element => 0,
                    ElementType::Component => 1,
                    ElementType::Slot => 2,
                    ElementType::Template => 3,
                },
                "props": element
                    .props
                    .iter()
                    .map(|prop| match prop {
                        Prop::Attribute(attribute) => self.attribute(attribute),
                        Prop::Directive(directive) => self.directive(directive),
                    })
                    .collect::<Vec<_>>(),
                "children": self.children(root, &element.children),
                "isSelfClosing": element.is_self_closing,
                "innerLoc": element.inner_loc.as_ref().map(|loc| self.loc(loc)),
                "loc": self.loc(&element.loc),
            }),
            Some(Node::Text(text)) => self.text(2, text),
            Some(Node::Comment(comment)) => self.text(3, comment),
            Some(Node::Interpolation(interpolation)) => json!({
                "type": 5,
                "content": self.expression(&interpolation.content),
                "loc": self.loc(&interpolation.loc),
            }),
            None => Value::Null,
        }
    }

    fn children(&self, root: &Root, children: &[NodeId]) -> Vec<Value> {
        children.iter().map(|id| self.node(root, *id)).collect()
    }

    fn root(&self, parsed: &Parsed) -> Value {
        json!({
            "children": self.children(&parsed.root, &parsed.root.children),
            "loc": self.loc(&parsed.root.loc),
            "errors": parsed
                .root
                .errors
                .iter()
                .map(|error| json!([
                    error.code.number(),
                    self.map.offset(error.position.offset),
                    error.position.line,
                ]))
                .collect::<Vec<_>>(),
        })
    }

    /// An attribute's offset is counted from the start of its block's tag.
    fn block_attr(&self, block: &Block, attr: &BlockAttr) -> Value {
        json!({
            "text": attr.text,
            "offset": self.map.offset(attr.position) - self.map.offset(block.tag_start),
        })
    }

    fn attr_or_true(&self, block: &Block, attr: Option<&AttrOrTrue>) -> Value {
        match attr {
            None => Value::Null,
            Some(AttrOrTrue::True) => Value::Bool(true),
            Some(AttrOrTrue::Attr(attr)) => self.block_attr(block, attr),
        }
    }

    fn block(&self, block: &Block) -> Value {
        let or_true = |value: &AttrValue| match value {
            AttrValue::True => json!(true),
            AttrValue::Text(text) => json!(text),
        };
        json!({
            "type": block.tag,
            "content": block.content,
            "start": self.map.offset(block.start.offset),
            "startLine": block.start.line,
            "end": self.map.offset(block.end.offset),
            "lang": block.lang,
            "attrs": block
                .attrs
                .iter()
                .map(|(name, value)| json!([name, or_true(value)]))
                .collect::<Vec<_>>(),
            "src": self.attr_or_true(block, block.src.as_ref()),
            "setup": block.setup.as_ref().map(or_true),
            "generic": self.attr_or_true(block, block.generic.as_ref()),
            "module": block.module.as_ref().map(|module| self.block_attr(block, module)),
            "scoped": block.scoped,
        })
    }
}

/// A parsed template.
pub fn template(source: &str, parsed: &Parsed) -> Value {
    Dumper {
        source,
        map: Utf16Map::new(source),
    }
    .root(parsed)
}

/// A single-file component: its tree and its blocks.
pub fn sfc(source: &str, sfc: &ParsedSfc) -> Value {
    let dumper = Dumper {
        source,
        map: Utf16Map::new(source),
    };
    let optional = |block: &Option<Block>| block.as_ref().map(|block| dumper.block(block));
    let all = |blocks: &[Block]| {
        blocks
            .iter()
            .map(|block| dumper.block(block))
            .collect::<Vec<_>>()
    };
    json!({
        "ast": dumper.root(&sfc.parsed),
        "comments": sfc.descriptor.comments,
        "template": optional(&sfc.descriptor.template),
        "script": optional(&sfc.descriptor.script),
        "scriptSetup": optional(&sfc.descriptor.script_setup),
        "styles": all(&sfc.descriptor.styles),
        "customBlocks": all(&sfc.descriptor.custom_blocks),
    })
}
