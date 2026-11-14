//! Slots: a port of `codegen/template/vSlot.ts` (a slot passed to a
//! component) and `slotOutlet.ts` (a `<slot>` of this component).

use super::analysis::slot_parameters;
use super::{
    ALL, COMPLETION, Generator, NAVIGATION, NAVIGATION_AND_VERIFICATION, VERIFICATION,
    WITHOUT_HIGHLIGHT, WITHOUT_HIGHLIGHT_AND_COMPLETION,
};
use crate::codegen::template::context::Slot;
use crate::codegen::{Code, END_OF_LINE, NEW_LINE, Source};
use crate::template::ast::{Directive, Element, Prop, SimpleExpression};

/// `replaceSourceRange` (muggle-string) with nothing: cuts a range of the
/// template out of the first piece of code that covers it.
fn remove_source_range(codes: &mut Vec<Code>, start: usize, end: usize) {
    let found = codes.iter().position(|code| match code {
        Code::Mapped {
            text,
            source: Source::Template,
            offset,
            ..
        } => *offset <= start && offset + text.len() >= end,
        _ => false,
    });
    let Some(index) = found else { return };
    let Some(Code::Mapped {
        text,
        source,
        offset,
        features,
    }) = codes.get(index).cloned()
    else {
        return;
    };
    let mut pieces = Vec::new();
    if start > offset {
        pieces.push(Code::Mapped {
            text: text.get(..start - offset).unwrap_or("").to_owned(),
            source,
            offset,
            features,
        });
    }
    if end < offset + text.len() {
        pieces.push(Code::Mapped {
            text: text.get(end - offset..).unwrap_or("").to_owned(),
            source,
            offset: end,
            features,
        });
    }
    codes.splice(index..=index, pieces);
}

impl<'a> Generator<'a> {
    /// `generateVSlot`: the content of a slot, with the slot's props in scope.
    pub(super) fn v_slot(
        &mut self,
        node: &'a Element,
        slot_dir: Option<&'a Directive>,
        ctx_var: &str,
    ) {
        let slot_var = self.ctx.internal_variable();
        if let Some(slot_dir) = slot_dir {
            self.text(format!("{{{NEW_LINE}"));
            self.text("const { ");
            match &slot_dir.arg {
                Some(arg) if !arg.content.is_empty() => {
                    self.object_property(
                        self.source(&arg.loc),
                        arg.loc.start.offset,
                        if arg.is_static {
                            WITHOUT_HIGHLIGHT
                        } else {
                            ALL
                        },
                        false,
                        true,
                    );
                }
                _ => {
                    let start = slot_dir.loc.start.offset;
                    let boundary = self.boundary(
                        start,
                        start + slot_dir.raw_name.len(),
                        WITHOUT_HIGHLIGHT_AND_COMPLETION,
                    );
                    self.text("default");
                    self.out.push(boundary.end());
                }
            }
        } else {
            self.text("const { ");
            // For an editor: the implicit default slot.
            let boundary = self.boundary(node.loc.start.offset, node.loc.end.offset, NAVIGATION);
            self.text("default");
            self.out.push(boundary.end());
        }
        self.text(format!(
            ": {slot_var} }} = __VLS_nonNull({ctx_var}.slots){END_OF_LINE}"
        ));

        self.ctx.open_scope();
        if let Some(exp) = slot_dir.and_then(|slot_dir| slot_dir.exp.as_ref()) {
            self.slot_parameters(exp, &slot_var);
        }
        for child in &node.children {
            self.template_child(*child, true, false);
        }
        self.ctx.end_scope(&mut self.out);

        if let Some(slot_dir) = slot_dir {
            self.text(format!("}}{NEW_LINE}"));
            // `#` or `v-slot:` with no name yet: where an editor completes one.
            let source = self.source(&slot_dir.loc);
            if slot_dir.arg.is_none() {
                let prefix = if source.starts_with('#') {
                    Some("#")
                } else if source.starts_with("v-slot:") {
                    Some("v-slot:")
                } else {
                    None
                };
                if let Some(prefix) = prefix {
                    self.text(format!(
                        "/** @type {{NonNullable<typeof {ctx_var}['slots']>['"
                    ));
                    self.mapped("", slot_dir.loc.start.offset + prefix.len(), COMPLETION);
                    self.text(format!("']}} */{END_OF_LINE}"));
                }
            }
        }
    }

    /// `generateSlotParameters`, and the names it puts in scope: the slot's
    /// props, destructured as an arrow function's parameters.
    fn slot_parameters(&mut self, exp: &SimpleExpression, slot_var: &str) {
        let text = format!("({}) => {{}}", exp.content);
        let Some(parsed) = slot_parameters(&text) else {
            self.ctx
                .give_up("a slot's parameters aren't a parameter list");
            return;
        };
        if !parsed.is_arrow {
            return;
        }
        let start_offset = exp.loc.start.offset.saturating_sub(1);
        let mut interpolation = self
            .buffered(|generator| generator.interpolation(ALL, &text, start_offset, "", "", false));
        // Without the parentheses and the function's body.
        remove_source_range(&mut interpolation, start_offset, start_offset + "(".len());
        remove_source_range(
            &mut interpolation,
            start_offset + text.len() - ") => {}".len(),
            start_offset + text.len(),
        );
        // A parameter's type is moved to the call that the props come from.
        let mut types: Vec<Option<Code>> = Vec::new();
        for (name_end, type_end) in &parsed.parameters {
            types.push(type_end.map(|type_end| {
                remove_source_range(
                    &mut interpolation,
                    start_offset + name_end,
                    start_offset + type_end,
                );
                Code::Mapped {
                    text: text.get(*name_end..type_end).unwrap_or("").to_owned(),
                    source: Source::Template,
                    offset: start_offset + name_end,
                    features: ALL,
                }
            }));
        }

        self.text("const [");
        self.out.extend(interpolation);
        self.text(format!("] = __VLS_vSlot(__VLS_nonNull({slot_var})"));
        if types.iter().any(Option::is_some) {
            self.text(", ");
            let boundary = self.boundary(exp.loc.start.offset, exp.loc.end.offset, VERIFICATION);
            self.text("(");
            for type_code in types {
                match type_code {
                    Some(type_code) => {
                        self.text("_");
                        self.out.push(type_code);
                        self.text(", ");
                    }
                    None => self.text("_, "),
                }
            }
            self.text(if self.options.is_ts {
                ") => [] as any"
            } else {
                ") => /** @type {any} */ ([])"
            });
            self.out.push(boundary.end());
        }
        self.text(format!("){END_OF_LINE}"));
        for name in parsed.declared {
            self.ctx.declare(&name);
        }
    }

    /// `generateSlotOutlet`: a `<slot>`, whose props are what the parent's
    /// slot content receives.
    #[expect(
        clippy::too_many_lines,
        reason = "one function in the original, in the same order"
    )]
    pub(super) fn slot_outlet(&mut self, node: &'a Element) {
        let options = self.options;
        let (start_tag_offset, _) = self.tag_offsets(node);
        let start_tag_end_offset = start_tag_offset + node.tag.len();
        let props_var = self.ctx.internal_variable();
        // `name="..."` or `:name="..."`
        let name_index = node.props.iter().position(|prop| match prop {
            Prop::Attribute(attribute) => attribute.name == "name",
            Prop::Directive(directive) => {
                directive.name == "bind"
                    && directive
                        .arg
                        .as_ref()
                        .is_some_and(|arg| arg.content == "name")
            }
        });
        let name_prop = name_index.and_then(|index| node.props.get(index));
        let other_props: Vec<&'a Prop> = node
            .props
            .iter()
            .enumerate()
            .filter_map(|(index, prop)| (Some(index) != name_index).then_some(prop))
            .collect();
        let slots = options.slots_assign_name.unwrap_or("__VLS_slots");

        if options.has_define_slots {
            let slot_var = self.ctx.internal_variable();
            self.text(format!("const {slot_var} = __VLS_asFunctionalSlot("));
            if let Some(name_prop) = name_prop {
                // The original makes the name's code before the boundary
                // for an attribute's, and inside it for a bound one's.
                let directive_codes = match name_prop {
                    Prop::Directive(directive) if directive.exp.is_some() => {
                        Some(self.buffered(|generator| {
                            generator.text("[");
                            generator.prop_exp(directive, directive.exp.as_ref());
                            generator.text("]");
                        }))
                    }
                    _ => None,
                };
                let loc = name_prop.loc();
                let boundary = self.boundary(loc.start.offset, loc.end.offset, VERIFICATION);
                self.text(slots);
                match (name_prop, directive_codes) {
                    (_, Some(codes)) => self.out.extend(codes),
                    (Prop::Attribute(attribute), None) if attribute.value.is_some() => {
                        if let Some(value) = &attribute.value {
                            let (content, offset) = self.attribute_value(value);
                            self.property_access(content, offset, NAVIGATION_AND_VERIFICATION);
                        }
                    }
                    _ => self.text("['default']"),
                }
                self.out.push(boundary.end());
            } else {
                let boundary = self.boundary(start_tag_offset, start_tag_end_offset, VERIFICATION);
                self.text(format!("{slots}["));
                let inner = self.boundary(start_tag_offset, start_tag_end_offset, VERIFICATION);
                self.text("'default'");
                self.out.push(inner.end());
                self.text("]");
                self.out.push(boundary.end());
            }
            self.text(format!("){END_OF_LINE}"));
            self.text(format!("{slot_var}("));
            let boundary = self.boundary(start_tag_offset, start_tag_end_offset, VERIFICATION);
            self.text(format!("{{{NEW_LINE}"));
            self.element_props(
                node,
                &other_props,
                true,
                None,
                Some(&format!("Parameters<typeof {slot_var}>[0]")),
            );
            self.text("}");
            self.out.push(boundary.end());
            self.text(format!("){END_OF_LINE}"));
        } else {
            self.text(format!("var {props_var} = {{{NEW_LINE}"));
            self.element_props(
                node,
                &other_props,
                options.compiler.check_unknown_props,
                None,
                None,
            );
            self.text(format!("}}{END_OF_LINE}"));

            match name_prop {
                Some(Prop::Attribute(attribute)) if attribute.value.is_some() => {
                    if let Some(value) = &attribute.value {
                        // Where the name is, after the attribute's own name.
                        let source = self.source(&attribute.loc);
                        let found =
                            crate::js::index_of(source, &value.content, attribute.name.len());
                        if found.is_none() {
                            self.ctx
                                .give_up("a slot's name isn't found in its attribute");
                        }
                        let props_var = self.ctx.hoist_variable(&props_var);
                        self.ctx.slots.push(Slot {
                            name: value.content.clone(),
                            offset: Some(attribute.loc.start.offset + found.unwrap_or(0)),
                            tag_range: (start_tag_offset, start_tag_offset + node.tag.len()),
                            props_var,
                        });
                    }
                }
                Some(Prop::Directive(directive)) if directive.exp.is_some() => {
                    if let Some(exp) = &directive.exp {
                        let is_shorthand = directive.arg.as_ref().map(|arg| arg.loc.start.offset)
                            == Some(exp.loc.start.offset);
                        let exp_var = self.ctx.internal_variable();
                        self.text(format!("var {exp_var} = __VLS_tryAsConstant("));
                        self.interpolation(
                            if is_shorthand {
                                WITHOUT_HIGHLIGHT_AND_COMPLETION
                            } else {
                                ALL
                            },
                            &exp.content,
                            exp.loc.start.offset,
                            "",
                            "",
                            false,
                        );
                        self.text(format!("){END_OF_LINE}"));
                        let exp_var = self.ctx.hoist_variable(&exp_var);
                        let props_var = self.ctx.hoist_variable(&props_var);
                        self.ctx.dynamic_slots.push((exp_var, props_var));
                    }
                }
                _ => {
                    let props_var = self.ctx.hoist_variable(&props_var);
                    self.ctx.slots.push(Slot {
                        name: "default".to_owned(),
                        offset: None,
                        tag_range: (start_tag_offset, start_tag_end_offset),
                        props_var,
                    });
                }
            }
        }
        for child in &node.children {
            self.template_child(*child, true, false);
        }
    }
}
