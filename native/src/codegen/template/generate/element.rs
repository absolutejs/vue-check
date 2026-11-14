//! Elements and components: a port of `codegen/template/element.ts`.

use super::props::FailedPropExpression;
use super::{
    ALL, Generator, IMPORT_COMPLETION_ONLY, NAVIGATION, NOT_TS6133, VERIFICATION,
    WITHOUT_COMPLETION, WITHOUT_HIGHLIGHT_AND_COMPLETION,
};
use crate::codegen::template::context::ComponentEntry;
use crate::codegen::{END_OF_LINE, Features, NEW_LINE, Verification, is_identifier, to_string};
use crate::js;
use crate::template::ast::{Element, NodeId, Prop};

/// The copy of a component's props that is passed to its constructor: one
/// line, with each `//` comment turned into a `/* */` one. The original does
/// that with `/\/\/(.*)$/gm`, which also finds a `//` inside a string
/// (vuejs/language-tools#6360); so does this.
fn props_copy(props: &str) -> String {
    let mut copy = String::with_capacity(props.len());
    // `.` stops at any of JavaScript's line terminators, and `$` is there.
    let mut rest = props;
    while !rest.is_empty() {
        let line_end = rest
            .find(['\n', '\r', '\u{2028}', '\u{2029}'])
            .unwrap_or(rest.len());
        let (line, after) = rest.split_at(line_end);
        match line.find("//") {
            Some(comment) => {
                copy.push_str(line.get(..comment).unwrap_or(""));
                copy.push_str("/*");
                copy.push_str(&line.get(comment + 2..).unwrap_or("").replace("*/", "*\\/"));
                copy.push_str(" */");
            }
            None => copy.push_str(line),
        }
        let terminator = after.chars().next().map_or(0, char::len_utf8);
        copy.push_str(after.get(..terminator).unwrap_or(""));
        rest = after.get(terminator..).unwrap_or("");
    }
    copy.replace('\n', " ")
}

impl<'a> Generator<'a> {
    /// `getTemplateRef`: the element's `ref="name"`.
    fn template_ref(&self, node: &Element) -> Option<(&'a str, usize)> {
        node.props.iter().find_map(|prop| match prop {
            Prop::Attribute(attribute) if attribute.name == "ref" => attribute
                .value
                .as_ref()
                .map(|value| self.attribute_value(value)),
            _ => None,
        })
    }

    /// `hasVBindAttrs`: whether the attributes passed to the component fall
    /// through to this element.
    fn has_v_bind_attrs(&self, id: NodeId, node: &Element) -> bool {
        self.options.compiler.fallthrough_attributes
            && ((self.options.inherit_attrs && self.ctx.single_root_nodes.contains(&id))
                || node.props.iter().any(|prop| {
                    matches!(prop, Prop::Directive(directive)
                        if directive.name == "bind"
                            && directive.exp.as_ref().is_some_and(|exp| self.source(&exp.loc) == "$attrs"))
                }))
    }

    /// `generateFailedExpressions`: the expressions of the props that aren't
    /// passed as props, so that they are checked all the same.
    fn failed_expressions(&mut self, failed: &[FailedPropExpression<'a>]) {
        for expression in failed {
            self.interpolation(
                ALL,
                expression.source,
                expression.offset,
                expression.prefix,
                expression.suffix,
                false,
            );
            self.text(END_OF_LINE);
        }
    }

    /// `generateComponent`.
    #[expect(
        clippy::too_many_lines,
        reason = "one function in the original, in the same order"
    )]
    pub(super) fn component(&mut self, id: NodeId, node: &'a Element) {
        let options = self.options;
        let compiler = options.compiler;
        let mut tag = node.tag.as_str();
        let mut props: Vec<&'a Prop> = node.props.iter().collect();
        let (mut start_tag_offset, mut end_tag_offset) = self.tag_offsets(node);
        let mut is_expression = false;
        let mut is_is_shorthand = false;

        if tag.contains('.') {
            is_expression = true;
        } else if tag == "component" {
            // `<component :is="expression">`
            for (index, prop) in node.props.iter().enumerate() {
                if let Prop::Directive(directive) = prop
                    && directive.name == "bind"
                    && directive
                        .arg
                        .as_ref()
                        .is_some_and(|arg| self.source(&arg.loc) == "is")
                    && let (Some(arg), Some(exp)) = (&directive.arg, &directive.exp)
                {
                    is_is_shorthand = arg.loc.end.offset == exp.loc.end.offset;
                    is_expression = true;
                    tag = exp.content.as_str();
                    start_tag_offset = exp.loc.start.offset;
                    end_tag_offset = None;
                    props.remove(index);
                    break;
                }
            }
        }

        let component_var = self.ctx.internal_variable();

        if is_expression {
            self.text(format!("const {component_var} = "));
            self.interpolation(
                if is_is_shorthand {
                    WITHOUT_HIGHLIGHT_AND_COMPLETION
                } else {
                    ALL
                },
                tag,
                start_tag_offset,
                "(",
                ")",
                false,
            );
            if let Some(end_tag_offset) = end_tag_offset {
                self.text(" || ");
                self.interpolation(WITHOUT_COMPLETION, tag, end_tag_offset, "(", ")", false);
            }
            self.text(END_OF_LINE);
        } else {
            // A `Set`: the three forms of the name, without repeats.
            let mut original_names: Vec<String> = Vec::new();
            for name in [
                js::capitalize(&js::camelize(tag)),
                js::camelize(tag),
                tag.to_owned(),
            ] {
                if !original_names.contains(&name) {
                    original_names.push(name);
                }
            }
            let matched = original_names
                .iter()
                .find(|name| options.imported_components.contains(*name));
            if let Some(matched) = matched {
                // The imported name, written as the tag is: the tag with the
                // name's first character.
                let mut tag_chars = tag.chars();
                let first_of_tag = tag_chars.next();
                let first_of_name = matched.chars().next();
                if first_of_tag.is_some_and(|char| char.len_utf16() > 1)
                    || first_of_name.is_some_and(|char| char.len_utf16() > 1)
                {
                    self.ctx.give_up(
                        "a component name starts with a character outside the basic plane",
                    );
                }
                let written: String = first_of_name.into_iter().chain(tag_chars).collect();
                self.text(format!("const {component_var} = "));
                // `withoutHighlightAndCompletion` and `importCompletionOnly`
                self.camelized(&written, start_tag_offset, WITHOUT_HIGHLIGHT_AND_COMPLETION);
                if let Some(end_tag_offset) = end_tag_offset {
                    self.text(" || ");
                    self.camelized(&written, end_tag_offset, WITHOUT_HIGHLIGHT_AND_COMPLETION);
                }
                self.text(END_OF_LINE);
            } else {
                let is_self = original_names
                    .iter()
                    .any(|name| name == options.component_name);
                self.typed_var("let", &component_var, |generator| {
                    generator.text(format!(
                        "__VLS_WithComponent<'{tag}', __VLS_LocalComponents, __VLS_GlobalComponents"
                    ));
                    generator.text(if is_self {
                        ", typeof __VLS_export"
                    } else {
                        ", void"
                    });
                    generator.text(format!(", {}", compiler.check_unknown_components));
                    for name in &original_names {
                        generator.text(format!(", '{name}'"));
                    }
                    generator.text(">[");
                    // `semanticWithoutHighlight` and `verification`
                    generator.string_literal_key(tag, start_tag_offset, VERIFICATION);
                    generator.text("]");
                });

                if is_identifier(&js::camelize(tag)) {
                    // For an editor: where the component is defined.
                    self.text("/** @ts-ignore @type {");
                    for offset in [Some(start_tag_offset), end_tag_offset]
                        .into_iter()
                        .flatten()
                    {
                        let capitalized = js::capitalize(tag);
                        let names = if capitalized == tag {
                            vec![tag.to_owned()]
                        } else {
                            vec![tag.to_owned(), capitalized]
                        };
                        for name in names {
                            let prefix = if options.setup_bindings.contains(&js::camelize(&name)) {
                                ""
                            } else {
                                "__VLS_components."
                            };
                            self.text(format!(" | typeof {prefix}"));
                            self.camelized(&name, offset, NAVIGATION);
                        }
                        if tag.contains('-') {
                            self.text(" | typeof __VLS_components[");
                            self.string_literal_key(tag, offset, NAVIGATION);
                            self.text("]");
                        }
                    }
                    self.text(format!("}} */{NEW_LINE}"));
                    // For an editor: where an import is completed.
                    self.camelized(tag, start_tag_offset, IMPORT_COMPLETION_ONLY);
                    self.text(END_OF_LINE);
                }
            }
        }

        self.ctx.components.push(ComponentEntry {
            ctx_var: String::new(),
            is_ctx_var_used: false,
        });
        let component_index = self.ctx.components.len() - 1;
        let mut is_props_var_used = false;

        let functional_var = self.ctx.internal_variable();
        let vnode_var = self.ctx.internal_variable();
        let ctx_var = self.ctx.internal_variable();
        let props_var = self.ctx.internal_variable();
        if let Some(component) = self.ctx.components.get_mut(component_index) {
            component.ctx_var.clone_from(&ctx_var);
        }

        let mut failed = Vec::new();
        let prop_codes = self.buffered(|generator| {
            generator.element_props(
                node,
                &props,
                compiler.check_unknown_props,
                Some(&mut failed),
                None,
            );
        });
        let props_str = to_string(&prop_codes);

        // The instance's type comes from a construct call with a copy of the
        // props, under `@ts-ignore`: only a call resolves the component's
        // overloads and infers its type parameters.
        self.text(format!("// @ts-ignore{NEW_LINE}"));
        self.text(format!(
            "const {functional_var} = {}({component_var}, new {component_var}({{{NEW_LINE}",
            if compiler.check_unknown_props {
                "__VLS_asFunctionalComponent0"
            } else {
                "__VLS_asFunctionalComponent1"
            }
        ));
        self.text(format!("// @ts-ignore{NEW_LINE}"));
        self.text(props_copy(&props_str));
        self.text(format!("}})){END_OF_LINE}"));

        self.text("const ");
        let boundary = self.boundary(node.loc.start.offset, node.loc.end.offset, NOT_TS6133);
        self.text(vnode_var.clone());
        self.out.push(boundary.end());
        self.text(format!(" = {functional_var}"));

        // `<!-- @vue-generic {T} -->`
        if let Some((content, offset)) = self
            .ctx
            .comment_stack
            .last()
            .and_then(|info| info.generic.clone())
        {
            let boundary = self.boundary(offset, offset + content.len(), VERIFICATION);
            self.text("<");
            self.mapped(content, offset, ALL);
            self.text(">");
            self.out.push(boundary.end());
        }

        let should_inherit_attrs = self.has_v_bind_attrs(id, node);

        self.text("(");
        let boundary = self.boundary(
            start_tag_offset,
            start_tag_offset + tag.len(),
            if should_inherit_attrs && compiler.check_required_fallthrough_attributes {
                Features::new(Verification::Off)
            } else {
                VERIFICATION
            },
        );
        self.text("{");
        // `__propsCompletion`: where an editor completes a prop.
        self.mapped("", node.loc.start.offset, Features::new(Verification::Off));
        self.text(NEW_LINE);
        self.out.extend(prop_codes);
        self.text("}");
        self.out.push(boundary.end());
        self.text(format!(
            ", ...__VLS_functionalComponentArgsRest({functional_var})){END_OF_LINE}"
        ));

        self.failed_expressions(&failed);
        self.element_events(
            node,
            &component_var,
            component_index,
            &props_var,
            &mut is_props_var_used,
        );
        self.element_directives(node);

        let template_ref = self.template_ref(node);
        let is_single_root = self.ctx.single_root_nodes.contains(&id)
            && !compiler
                .fallthrough_component_names
                .contains(&js::hyphenate(tag));
        let infer_root_el =
            self.ctx.dollar_vars.contains("$el") || compiler.infer_component_dollar_el;
        let infer_template_refs = compiler.infer_template_dollar_refs
            || compiler.infer_component_dollar_refs
            || !options.setup_refs.is_empty();

        if (template_ref.is_some() && infer_template_refs) || (is_single_root && infer_root_el) {
            let instance_var = self.ctx.internal_variable();
            self.typed_var("var", &instance_var, |generator| {
                let ctx_var = generator.use_ctx_var(component_index);
                generator.text(format!(
                    "Parameters<NonNullable<typeof {ctx_var}['expose']>>[0]"
                ));
            });
            if let Some((name, offset)) = template_ref
                && infer_template_refs
            {
                let mut type_exp =
                    format!("typeof {} | null", self.ctx.hoist_variable(&instance_var));
                if self.ctx.in_v_for {
                    type_exp = format!("({type_exp})[]");
                }
                self.ctx.add_template_ref(name, type_exp, offset);
            }
            if is_single_root && infer_root_el {
                let type_text = format!("NonNullable<typeof {instance_var}>['$el']");
                if !self.ctx.single_root_el_types.contains(&type_text) {
                    self.ctx.single_root_el_types.push(type_text);
                }
            }
        }

        if should_inherit_attrs {
            is_props_var_used = true;
            let inherited = if compiler.check_required_fallthrough_attributes {
                let rests_var = self.ctx.internal_variable();
                self.text(format!(
                    "var {rests_var} = __VLS_omit({props_var}, {{\n{props_str}}}){END_OF_LINE}"
                ));
                rests_var
            } else {
                props_var.clone()
            };
            if !self.ctx.inherited_attr_vars.contains(&inherited) {
                self.ctx.inherited_attr_vars.push(inherited);
            }
        }

        self.style_scoped_class_references(node);

        let slot_dir = node.props.iter().find_map(|prop| match prop {
            Prop::Directive(directive) if directive.name == "slot" => Some(&**directive),
            _ => None,
        });
        let has_slots = slot_dir.is_some() || !node.children.is_empty();

        // Declared before the slots' code that reads them.
        let is_ctx_var_used = self
            .ctx
            .components
            .get(component_index)
            .is_some_and(|component| component.is_ctx_var_used);
        if is_ctx_var_used || has_slots {
            self.typed_var("var", &ctx_var, |generator| {
                generator.text(format!(
                    "__VLS_ExtractComponentContext<typeof {component_var}, typeof {vnode_var}>"
                ));
            });
        }
        if is_props_var_used {
            self.typed_var("var", &props_var, |generator| {
                generator.text(format!(
                    "__VLS_ExtractComponentProps<typeof {component_var}, typeof {vnode_var}>"
                ));
            });
        }
        if has_slots {
            self.v_slot(node, slot_dir, &ctx_var);
        }
        self.ctx.components.pop();
    }

    /// `getCtxVar()`: the component's context variable, which is then
    /// declared.
    pub(super) fn use_ctx_var(&mut self, component_index: usize) -> String {
        self.ctx
            .components
            .get_mut(component_index)
            .map(|component| {
                component.is_ctx_var_used = true;
                component.ctx_var.clone()
            })
            .unwrap_or_default()
    }

    /// `generateElement`.
    pub(super) fn plain_element(&mut self, id: NodeId, node: &'a Element) {
        let compiler = self.options.compiler;
        let (start_tag_offset, end_tag_offset) = self.tag_offsets(node);
        let mut failed = Vec::new();
        let as_element = if compiler.check_unknown_props {
            "__VLS_asFunctionalElement0"
        } else {
            "__VLS_asFunctionalElement1"
        };

        self.text(format!("{as_element}(__VLS_intrinsics"));
        self.property_access(
            &node.tag,
            start_tag_offset,
            WITHOUT_HIGHLIGHT_AND_COMPLETION,
        );
        if let Some(end_tag_offset) = end_tag_offset {
            self.text(", ");
            self.text("__VLS_intrinsics");
            self.property_access(&node.tag, end_tag_offset, WITHOUT_HIGHLIGHT_AND_COMPLETION);
        }
        self.text(")(");
        let boundary = self.boundary(
            start_tag_offset,
            start_tag_offset + node.tag.len(),
            VERIFICATION,
        );
        self.text(format!("{{{NEW_LINE}"));
        let props: Vec<&'a Prop> = node.props.iter().collect();
        let attrs_type = format!("typeof __VLS_intrinsics['{}']", node.tag);
        self.element_props(
            node,
            &props,
            compiler.check_unknown_props,
            Some(&mut failed),
            Some(&attrs_type),
        );
        self.text("}");
        self.out.push(boundary.end());
        self.text(format!("){END_OF_LINE}"));

        self.failed_expressions(&failed);
        self.element_directives(node);

        if let Some((name, offset)) = self.template_ref(node) {
            let mut type_exp = format!("__VLS_Elements['{}']", node.tag);
            if self.ctx.in_v_for {
                type_exp.push_str("[]");
            }
            self.ctx.add_template_ref(name, type_exp, offset);
        }
        if self.ctx.single_root_nodes.contains(&id)
            && (self.ctx.dollar_vars.contains("$el") || compiler.infer_component_dollar_el)
        {
            let type_text = format!("__VLS_Elements['{}']", node.tag);
            if !self.ctx.single_root_el_types.contains(&type_text) {
                self.ctx.single_root_el_types.push(type_text);
            }
        }
        if self.has_v_bind_attrs(id, node) {
            let inherited = format!("__VLS_intrinsics.{}", node.tag);
            if !self.ctx.inherited_attr_vars.contains(&inherited) {
                self.ctx.inherited_attr_vars.push(inherited);
            }
        }

        self.style_scoped_class_references(node);

        for child in &node.children {
            self.template_child(*child, true, false);
        }
    }

    /// `generateFragment`: a `<template>` that is only a wrapper.
    pub(super) fn fragment(&mut self, node: &'a Element) {
        let compiler = self.options.compiler;
        let (start_tag_offset, _) = self.tag_offsets(node);
        // `<template v-for="..." :key="...">`
        if !node.props.is_empty() {
            let mut failed = Vec::new();
            let as_element = if compiler.check_unknown_props {
                "__VLS_asFunctionalElement0"
            } else {
                "__VLS_asFunctionalElement1"
            };
            self.text(format!("{as_element}(__VLS_intrinsics.template)("));
            let boundary = self.boundary(
                start_tag_offset,
                start_tag_offset + node.tag.len(),
                VERIFICATION,
            );
            self.text(format!("{{{NEW_LINE}"));
            let props: Vec<&'a Prop> = node.props.iter().collect();
            self.element_props(
                node,
                &props,
                compiler.check_unknown_props,
                Some(&mut failed),
                Some("typeof __VLS_intrinsics.template"),
            );
            self.text("}");
            self.out.push(boundary.end());
            self.text(format!("){END_OF_LINE}"));
            self.failed_expressions(&failed);
        }
        for child in &node.children {
            self.template_child(*child, true, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::props_copy;

    #[test]
    fn comments_become_block_comments_on_one_line() {
        assert_eq!(
            props_copy("a: 1, // note\nb: 2,\n"),
            "a: 1, /* note */ b: 2, "
        );
        // The original's bug, kept: a `//` inside a string.
        assert_eq!(
            props_copy("href: \"https://example.com\",\ncount: 1,\n"),
            "href: \"https:/*example.com\", */ count: 1, "
        );
        assert_eq!(props_copy("a: 1, // x */ y\n"), "a: 1, /* x *\\/ y */ ");
    }
}
