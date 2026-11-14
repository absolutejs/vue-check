//! A component's event listeners: a port of
//! `codegen/template/elementEvents.ts`.

use super::analysis::handler_shape;
use super::{ALL, Generator, NAVIGATION, NOT_TS2353, VERIFICATION};
use crate::codegen::{END_OF_LINE, Features, NEW_LINE, is_identifier, ref_brand_argument};
use crate::js;
use crate::template::ast::{Directive, Element, Prop};

/// `getEventArgName`: the name of the prop an event is listened to with.
pub(super) fn event_arg_name(name: &str, directive: &str) -> String {
    let name = if directive.is_empty() {
        name.to_owned()
    } else {
        js::capitalize(name)
    };
    let camelized: String = name
        .split('-')
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                part.to_owned()
            } else {
                js::capitalize(part)
            }
        })
        .collect();
    format!("{directive}{camelized}")
}

/// The listeners of one event, written the same way.
struct Definition<'a> {
    key: String,
    prop_prefix: &'static str,
    emit_prefix: &'static str,
    prop_name: String,
    emit_name: String,
    items: Vec<(&'a Directive, &'a str, Option<usize>)>,
}

impl<'a> Generator<'a> {
    /// The listeners of an element, grouped by event and modifiers.
    fn event_definitions(&self, node: &'a Element) -> Vec<Definition<'a>> {
        let options = self.options;
        let mut definitions: Vec<Definition<'a>> = Vec::new();
        for prop in &node.props {
            let Prop::Directive(directive) = prop else {
                continue;
            };
            let is_static_or_none = directive.arg.as_ref().is_none_or(|arg| arg.is_static);
            let is_event =
                directive.name == "on" && directive.arg.as_ref().is_some_and(|arg| arg.is_static);
            let is_model =
                options.compiler.strict_v_model && directive.name == "model" && is_static_or_none;
            if !is_event && !is_model {
                continue;
            }
            let mut source = directive
                .arg
                .as_ref()
                .map_or("model-value", |arg| self.source(&arg.loc));
            let mut offset = directive.arg.as_ref().map(|arg| arg.loc.start.offset);
            let (mut prop_prefix, mut emit_prefix) = ("on-", "");
            if directive.name == "model" {
                (prop_prefix, emit_prefix) = ("onUpdate:", "update:");
            } else if let Some(hook) = source.strip_prefix("vue:") {
                source = hook;
                offset = offset.map(|offset| offset + "vue:".len());
                (prop_prefix, emit_prefix) = ("onVnode-", "vnode-");
            }
            let prop_name = js::camelize(&format!("{prop_prefix}{source}"));
            let key = std::iter::once(directive.name.as_str())
                .chain(std::iter::once(prop_name.as_str()))
                .chain(
                    directive
                        .modifiers
                        .iter()
                        .map(|modifier| modifier.content.as_str()),
                )
                .collect::<Vec<_>>()
                .join("+");
            let index = definitions
                .iter()
                .position(|known| known.key == key)
                .unwrap_or_else(|| {
                    definitions.push(Definition {
                        key,
                        prop_prefix,
                        emit_prefix,
                        emit_name: format!("{emit_prefix}{source}"),
                        prop_name,
                        items: Vec::new(),
                    });
                    definitions.len() - 1
                });
            if let Some(definition) = definitions.get_mut(index) {
                definition.items.push((directive, source, offset));
            }
        }
        definitions
    }

    /// `generateElementEvents`.
    pub(super) fn element_events(
        &mut self,
        node: &'a Element,
        component_var: &str,
        component_index: usize,
        props_var: &str,
        is_props_var_used: &mut bool,
    ) {
        let options = self.options;
        let definitions = self.event_definitions(node);
        if definitions.is_empty() {
            return;
        }

        let emits_var = self.ctx.internal_variable();
        self.typed_var("let", &emits_var, |generator| {
            let ctx_var = generator.use_ctx_var(component_index);
            generator.text(format!(
                "__VLS_ResolveEmits<typeof {component_var}, typeof {ctx_var}.emit>"
            ));
        });

        for definition in definitions {
            let Definition {
                prop_prefix,
                emit_prefix,
                prop_name,
                emit_name,
                items,
                ..
            } = definition;
            let event_var = self.ctx.internal_variable();
            *is_props_var_used = true;
            let event_type = format!(
                "{}Partial<__VLS_ResolveEvent<typeof {props_var}, typeof {emits_var}, '{prop_name}', '{emit_name}', '{}'>>",
                if options.compiler.check_unknown_events {
                    ""
                } else {
                    "Record<string, unknown> & "
                },
                js::camelize(&emit_name)
            );
            if options.is_ts {
                self.text(format!("const {event_var}: {event_type} = {{{NEW_LINE}"));
            } else {
                self.text(format!(
                    "/** @type {{{event_type}}} */{NEW_LINE}const {event_var} = {{{NEW_LINE}"
                ));
            }
            let emit_directive = emit_prefix
                .get(..emit_prefix.len().saturating_sub(1))
                .unwrap_or("");
            let prop_directive = prop_prefix
                .get(..prop_prefix.len().saturating_sub(1))
                .unwrap_or("");
            for (directive, source, offset) in items {
                let args_type = format!("__VLS_EventArgs<{event_type}, '{prop_name}'>");
                if directive.name == "on" {
                    let offset = offset.unwrap_or(0);
                    self.text(format!("/** @type {{typeof {emits_var}."));
                    self.event_arg(source, offset, emit_directive, Some(NAVIGATION));
                    self.text(format!("}} */{NEW_LINE}"));
                    // In JavaScript the JSDoc above types the property, and
                    // doesn't parse for a name that isn't an identifier.
                    let emit_arg_name = event_arg_name(source, emit_directive);
                    self.event_arg(source, offset, prop_directive, None);
                    self.text(": ");
                    let args_type = if options.is_ts {
                        args_type
                    } else if is_identifier(&emit_arg_name) {
                        format!("__VLS_EventArgs<typeof {emits_var}, '{emit_arg_name}'>")
                    } else {
                        "any[]".to_owned()
                    };
                    self.event_expression(directive, &args_type);
                } else {
                    self.text(format!("'{prop_name}': "));
                    self.model_event_expression(directive, &args_type);
                }
                self.text(format!(",{NEW_LINE}"));
            }
            self.text(format!("}}{END_OF_LINE}"));
            self.text(format!("void {event_var}{END_OF_LINE}"));
        }
    }

    /// `generateEventArg`: an event's name as a prop's name (`onClick`).
    pub(super) fn event_arg(
        &mut self,
        name: &str,
        start: usize,
        directive: &str,
        features: Option<Features>,
    ) {
        // `semanticWithoutHighlight`, `navigationWithoutRename`, and whether
        // an unknown event is reported.
        let features = features.unwrap_or(if self.options.compiler.check_unknown_events {
            VERIFICATION
        } else {
            NOT_TS2353
        });
        let name = if directive.is_empty() {
            name.to_owned()
        } else {
            js::capitalize(name)
        };
        let boundary = self.boundary(start, start + name.len(), features);
        if is_identifier(&js::camelize(&name)) {
            // The original doesn't end the boundary here.
            self.text(directive);
            self.camelized(&name, start, boundary.features);
        } else {
            self.text("'");
            self.text(directive);
            self.camelized(&name, start, boundary.features);
            self.text("'");
            self.out.push(boundary.end());
        }
    }

    /// `generateEventParameter`.
    fn event_parameter(&mut self, args_type: &str) {
        if !self.options.compiler.core.is_3_3_14() {
            // Before 3.3.14 `$event` has no type of its own.
            self.text(format!("// @ts-ignore{NEW_LINE}"));
            self.text(format!("(...[$event]) => {{{NEW_LINE}"));
        } else if self.options.is_ts {
            self.text(format!("(...[$event]: {args_type}) => {{{NEW_LINE}"));
        } else {
            self.text(format!(
                "(/** @type {{{args_type}}} */ ...[$event]) => {{{NEW_LINE}"
            ));
        }
    }

    /// `generateEventExpression`: a listener. A function or a reference to
    /// one is passed as it is; anything else is the body of a function that
    /// has `$event`.
    pub(super) fn event_expression(&mut self, directive: &Directive, args_type: &str) {
        let Some(exp) = &directive.exp else {
            self.text("() => {}");
            return;
        };
        let Some(shape) = handler_shape(&exp.content) else {
            self.ctx.give_up("an event handler has a syntax error");
            return;
        };
        let start = exp.loc.start.offset;
        if !shape.is_compound {
            self.interpolation(ALL, &exp.content, start, "(", ")", false);
            return;
        }
        self.ctx.open_scope();
        self.ctx.declare("$event");
        let access_mark = self.ctx.access_log.len();
        let codes = self
            .buffered(|generator| generator.interpolation(ALL, &exp.content, start, "", "", false));

        self.event_parameter(args_type);
        // So that an unused `$event` isn't reported.
        self.text(format!("void $event{END_OF_LINE}"));
        self.reasserts(access_mark);
        self.ctx.generate_condition_guards(&mut self.out);
        if shape.is_single_expression {
            self.text("return (");
            self.out.extend(codes);
            self.text(")");
        } else {
            self.out.extend(codes);
        }
        self.text(END_OF_LINE);
        self.ctx.end_scope(&mut self.out);
        self.text("}");
    }

    /// `generateModelEventExpression`: `v-model` as the listener of its
    /// update event.
    fn model_event_expression(&mut self, directive: &Directive, args_type: &str) {
        let Some(exp) = &directive.exp else {
            self.text("() => {}");
            return;
        };
        let access_mark = self.ctx.access_log.len();
        let start = exp.loc.start.offset;
        let codes = self.buffered(|generator| {
            generator.interpolation(VERIFICATION, &exp.content, start, "", "", true);
        });
        self.event_parameter(args_type);
        self.reasserts(access_mark);
        self.ctx.generate_condition_guards(&mut self.out);
        if self.options.compiler.core.is_3_3_14() {
            let boundary = self.boundary(start, exp.loc.end.offset, VERIFICATION);
            self.text("(");
            self.out.extend(codes);
            self.text(")");
            self.out.push(boundary.end());
        } else {
            self.out.extend(codes);
        }
        self.text(format!(" = $event{END_OF_LINE}"));
        self.text("}");
    }

    /// `generateReasserts`: assertion narrowing doesn't flow into a closure
    /// for an import or a `let`, so the ones read are asserted again.
    fn reasserts(&mut self, access_mark: usize) {
        let mut reasserts: Vec<&str> = Vec::new();
        let accessed = self.ctx.access_log.get(access_mark..).unwrap_or(&[]).iter();
        let in_conditions = self
            .ctx
            .conditions
            .iter()
            .flat_map(|condition| condition.accesses.iter());
        for name in accessed.chain(in_conditions) {
            if self.options.reassert_bindings.contains(name)
                && !self.ctx.is_in_scope(name)
                && !reasserts.contains(&name.as_str())
            {
                reasserts.push(name);
            }
        }
        let brand = ref_brand_argument(&self.options.compiler.lib, self.options.is_ts);
        let codes: Vec<String> = reasserts
            .iter()
            .map(|name| format!("__VLS_withDotValue({name}, {brand}){END_OF_LINE}"))
            .collect();
        for code in codes {
            self.text(code);
        }
    }
}
