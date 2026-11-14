//! The parser's results as JSON, in the shape `native/oracle/dump.ts` writes
//! the original's, so the two can be compared value for value. Offsets are
//! converted to UTF-16 here.

use serde_json::{Value, json};

use crate::sfc::{AttrOrTrue, AttrValue, Block, BlockAttr, ParsedSfc};
use crate::template::ast::{
    Attribute, CompoundChild, ConstantType, Directive, ElementType, ForParseResult, Loc, Namespace,
    Node, NodeId, Prop, Root, SimpleExpression, Text,
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
            "forParseResult": directive
                .for_parse_result
                .as_ref()
                .map(|result| self.for_parse_result(result)),
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
            Some(Node::Compound(compound)) => json!({
                "type": 8,
                "children": compound
                    .children
                    .iter()
                    .map(|child| match child {
                        CompoundChild::Node(id) => self.node(root, *id),
                        CompoundChild::Text(text) => json!(text),
                    })
                    .collect::<Vec<_>>(),
                "loc": self.loc(&compound.loc),
            }),
            Some(Node::If(node)) => json!({
                "type": 9,
                "branches": self.children(root, &node.branches),
                "loc": self.loc(&node.loc),
            }),
            Some(Node::IfBranch(branch)) => json!({
                "type": 10,
                "condition": branch.condition.as_ref().map(|condition| self.expression(condition)),
                "children": self.children(root, &branch.children),
                "loc": self.loc(&branch.loc),
            }),
            Some(Node::For(node)) => json!({
                "type": 11,
                "parseResult": self.for_parse_result(&node.parse_result),
                "children": self.children(root, &node.children),
                "loc": self.loc(&node.loc),
            }),
            None => Value::Null,
        }
    }

    fn for_parse_result(&self, result: &ForParseResult) -> Value {
        let optional = |expression: &Option<SimpleExpression>| {
            expression
                .as_ref()
                .map(|expression| self.expression(expression))
        };
        json!({
            "source": self.expression(&result.source),
            "value": optional(&result.value),
            "key": optional(&result.key),
            "index": optional(&result.index),
        })
    }

    fn children(&self, root: &Root, children: &[NodeId]) -> Vec<Value> {
        children.iter().map(|id| self.node(root, *id)).collect()
    }

    fn root(&self, parsed: &Parsed) -> Value {
        json!({
            "children": self.children(&parsed.root, &parsed.root.children),
            "loc": self.loc(&parsed.root.loc),
            "components": parsed.root.components,
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

/// A `<script>` block's analysis, with offsets into the block's text.
pub mod script {
    use serde_json::{Value, json};

    use crate::script::{
        Bindings, CallRange, ComponentOptions, MacroCall, Range, ScriptRanges, ScriptSetupRanges,
    };
    use crate::utf16::Utf16Map;

    fn range(map: &Utf16Map, range: Range) -> Value {
        json!([map.offset(range.start), map.offset(range.end)])
    }

    fn optional(map: &Utf16Map, value: Option<Range>) -> Value {
        value.map_or(Value::Null, |value| range(map, value))
    }

    fn bindings(bindings: &Bindings) -> Value {
        json!(bindings.0)
    }

    fn call(map: &Utf16Map, call: &CallRange) -> Value {
        json!({
            "callExp": range(map, call.call_exp),
            "exp": range(map, call.exp),
            "arg": optional(map, call.arg),
            "typeArg": optional(map, call.type_arg),
        })
    }

    fn macro_call(map: &Utf16Map, value: &MacroCall) -> Value {
        let mut dumped = call(map, &value.call);
        if let Some(fields) = dumped.as_object_mut() {
            fields.insert("name".to_owned(), json!(value.name));
            fields.insert("statement".to_owned(), range(map, value.statement));
        }
        dumped
    }

    fn options(map: &Utf16Map, options: &ComponentOptions) -> Value {
        json!({
            "isObjectLiteral": options.is_object_literal,
            "expression": range(map, options.expression),
            "args": range(map, options.args),
            "components": optional(map, options.components),
            "directives": optional(map, options.directives),
            "name": optional(map, options.name),
            "inheritAttrs": options.inherit_attrs,
        })
    }

    pub fn ranges(text: &str, ranges: &ScriptRanges) -> Value {
        let map = Utf16Map::new(text);
        json!({
            "bindings": bindings(&ranges.bindings),
            "exportDefault": ranges.export_default.as_ref().map(|export| json!({
                "start": map.offset(export.range.start),
                "end": map.offset(export.range.end),
                "expression": range(&map, export.expression),
                "isObjectLiteral": export.is_object_literal,
                "options": export.options.as_ref().map(|found| options(&map, found)),
            })),
        })
    }

    pub fn setup_ranges(text: &str, ranges: &ScriptSetupRanges) -> Value {
        let map = Utf16Map::new(text);
        let calls = |calls: &[CallRange]| {
            calls
                .iter()
                .map(|found| call(&map, found))
                .collect::<Vec<_>>()
        };
        json!({
            "bindings": bindings(&ranges.bindings),
            "leadingCommentEndOffset": map.offset(ranges.leading_comment_end_offset),
            "importSectionEndOffset": map.offset(ranges.import_section_end_offset),
            "defineModel": ranges.define_model.iter().map(|model| json!({
                "arg": optional(&map, model.arg),
                "localName": optional(&map, model.local_name),
                "name": optional(&map, model.name),
                "type": optional(&map, model.r#type),
                "modifierType": optional(&map, model.modifier_type),
                "runtimeType": optional(&map, model.runtime_type),
                "defaultValue": optional(&map, model.default_value),
                "defaultPropsArg": optional(&map, model.default_props_arg),
                "required": model.required,
                "comments": optional(&map, model.comments),
            })).collect::<Vec<_>>(),
            "defineProps": ranges.define_props.as_ref().map(|props| {
                let mut dumped = macro_call(&map, &props.call);
                if let Some(fields) = dumped.as_object_mut() {
                    fields.insert(
                        "destructured".to_owned(),
                        json!(props.destructured.as_ref().map(|names| names
                            .iter()
                            .map(|(name, initializer)| json!([name, optional(&map, *initializer)]))
                            .collect::<Vec<_>>())),
                    );
                    fields.insert("destructuredRest".to_owned(), json!(props.destructured_rest));
                }
                dumped
            }),
            "withDefaults": ranges.with_defaults.as_ref().map(|found| call(&map, found)),
            "defineEmits": ranges.define_emits.as_ref().map(|emits| {
                let mut dumped = macro_call(&map, &emits.call);
                if let Some(fields) = dumped.as_object_mut() {
                    fields.insert("hasUnionTypeArg".to_owned(), json!(emits.has_union_type_arg));
                }
                dumped
            }),
            "defineSlots": ranges.define_slots.as_ref().map(|found| macro_call(&map, found)),
            "defineExpose": ranges.define_expose.as_ref().map(|found| call(&map, found)),
            "defineOptions": ranges.define_options.as_ref().map(|found| json!({
                "name": found.name,
                "inheritAttrs": found.inherit_attrs,
            })),
            "useAttrs": calls(&ranges.use_attrs),
            "useCssModule": calls(&ranges.use_css_module),
            "useSlots": calls(&ranges.use_slots),
            "useTemplateRef": ranges.use_template_ref.iter().map(|found| {
                let mut dumped = call(&map, &found.call);
                if let Some(fields) = dumped.as_object_mut() {
                    fields.insert("name".to_owned(), json!(found.name));
                }
                dumped
            }).collect::<Vec<_>>(),
        })
    }
}
