//! An element's directives: a port of
//! `codegen/template/elementDirectives.ts`.

use super::{ALL, Generator, VERIFICATION, WITHOUT_HIGHLIGHT};
use crate::codegen::{END_OF_LINE, Features, NEW_LINE, Verification};
use crate::js;
use crate::template::ast::{Directive, Element, Prop};

/// `isBuiltInDirective` (`@vue/shared`).
fn is_built_in_directive(name: &str) -> bool {
    matches!(
        name,
        "bind"
            | "cloak"
            | "else-if"
            | "else"
            | "for"
            | "html"
            | "if"
            | "model"
            | "on"
            | "once"
            | "pre"
            | "show"
            | "slot"
            | "text"
            | "memo"
    )
}

impl Generator<'_> {
    /// `generateElementDirectives`.
    pub(super) fn element_directives(&mut self, node: &Element) {
        for prop in &node.props {
            let Prop::Directive(directive) = prop else {
                continue;
            };
            if matches!(directive.name.as_str(), "slot" | "on" | "model" | "bind") {
                continue;
            }
            let (start, end) = (directive.loc.start.offset, directive.loc.end.offset);
            if self.options.is_vapor && !is_built_in_directive(&directive.name) {
                // A vapor directive is called with a getter of its value.
                let boundary = self.boundary(start, end, VERIFICATION);
                self.directive_identifier(directive);
                self.text("(__VLS_nonNull(null), ");
                self.directive_value(directive, false);
                self.directive_arg(directive, false);
                self.modifiers(directive, "modifiers", false);
                self.text(")");
                self.out.push(boundary.end());
                self.text(END_OF_LINE);
            } else if !self.options.compiler.core.is_3_3_14() {
                // Before 3.3.14: one call, with two more arguments whatever
                // the hook declares.
                let boundary = self.boundary(start, end, VERIFICATION);
                self.text("__VLS_asFunctionalDirective(");
                self.directive_identifier(directive);
                let directive_type = self.as_type(&format!(
                    "import('{}').ObjectDirective",
                    self.options.compiler.lib
                ));
                self.text(format!(
                    ", {directive_type})(__VLS_nonNull(null), {{ ...__VLS_directiveBindingRestFields, "
                ));
                self.directive_arg(directive, true);
                self.modifiers(directive, "modifiers", true);
                self.directive_value(directive, true);
                self.text(format!(
                    "}},{NEW_LINE}__VLS_nonNull(null), __VLS_nonNull(null))"
                ));
                self.out.push(boundary.end());
                self.text(END_OF_LINE);
            } else {
                let hook_var = self.ctx.internal_variable();
                self.text(format!("const {hook_var} = __VLS_asFunctionalDirective("));
                self.directive_identifier(directive);
                let directive_type = self.as_type(&format!(
                    "import('{}').ObjectDirective",
                    self.options.compiler.lib
                ));
                self.text(format!(", {directive_type}){END_OF_LINE}"));
                let boundary = self.boundary(start, end, VERIFICATION);
                self.text(format!(
                    "{hook_var}(__VLS_nonNull(null), {{ ...__VLS_directiveBindingRestFields, "
                ));
                self.directive_arg(directive, true);
                self.modifiers(directive, "modifiers", true);
                self.directive_value(directive, true);
                // The rest of a hook's arguments, as many as it declares.
                self.text(format!(
                    "}},{NEW_LINE}...__VLS_directiveHookArgsRest({hook_var}))"
                ));
                self.out.push(boundary.end());
                self.text(END_OF_LINE);
            }
        }
    }

    /// `generateIdentifier`: the directive, read from `__VLS_directives`.
    fn directive_identifier(&mut self, directive: &Directive) {
        let raw_name = format!("v-{}", directive.name);
        let start = directive.loc.start.offset;
        let is_built_in = is_built_in_directive(&directive.name);
        let boundary = self.boundary(start, start + raw_name.len(), VERIFICATION);
        self.text("__VLS_directives");
        self.text(".");
        self.camelized(
            &raw_name,
            start,
            Features::new(
                if self.options.compiler.check_unknown_directives && !is_built_in {
                    Verification::On
                } else {
                    Verification::Off
                },
            ),
        );
        if !is_built_in {
            self.ctx.access_variable(
                crate::codegen::Source::Template,
                &js::camelize(&raw_name),
                Some(start),
                false,
                // 3.3.14 keeps the name as written, for the import it may add.
                self.options
                    .compiler
                    .core
                    .is_3_3_14()
                    .then_some(raw_name.as_str()),
            );
        }
        self.out.push(boundary.end());
    }

    /// `generateArg`: `v-name:arg`.
    fn directive_arg(&mut self, directive: &Directive, as_binding_property: bool) {
        let Some(arg) = &directive.arg else {
            if !as_binding_property {
                self.text("undefined, ");
            }
            return;
        };
        // The argument's content, inside its brackets when it has them.
        let start = arg.loc.start.offset
            + self
                .source(&arg.loc)
                .find(arg.content.as_str())
                .unwrap_or(0);
        if as_binding_property {
            let boundary = self.boundary(start, start + arg.content.len(), VERIFICATION);
            self.text("arg");
            self.out.push(boundary.end());
            self.text(": ");
        }
        if arg.is_static {
            self.string_literal_key(&arg.content, start, ALL);
        } else {
            self.interpolation(ALL, &arg.content, start, "(", ")", false);
        }
        self.text(", ");
    }

    /// `generateModifiers`: `v-name.a.b` as `{ a: true, b: true }`.
    pub(super) fn modifiers(
        &mut self,
        directive: &Directive,
        property_name: &str,
        as_binding_property: bool,
    ) {
        let (Some(first), Some(last)) = (directive.modifiers.first(), directive.modifiers.last())
        else {
            if !as_binding_property {
                self.text("undefined, ");
            }
            return;
        };
        if as_binding_property {
            // From the `.` before the first one.
            let boundary = self.boundary(
                first.loc.start.offset.saturating_sub(1),
                last.loc.end.offset,
                VERIFICATION,
            );
            self.text(property_name);
            self.out.push(boundary.end());
            self.text(": ");
        }
        self.text("{ ");
        for modifier in &directive.modifiers {
            self.object_property(
                &modifier.content,
                modifier.loc.start.offset,
                WITHOUT_HIGHLIGHT,
                false,
                false,
            );
            self.text(": true, ");
        }
        self.text("}, ");
    }

    /// `generateValue`: `v-name="value"`.
    fn directive_value(&mut self, directive: &Directive, as_binding_property: bool) {
        let Some(exp) = &directive.exp else {
            if !as_binding_property {
                self.text("undefined, ");
            }
            return;
        };
        let boundary = self.boundary(exp.loc.start.offset, exp.loc.end.offset, VERIFICATION);
        if as_binding_property {
            self.text("value");
            self.out.push(boundary.end());
            self.text(": ");
            self.prop_exp(directive, Some(exp));
        } else {
            self.text("() => ");
            self.prop_exp(directive, Some(exp));
            self.out.push(boundary.end());
        }
        self.text(", ");
    }
}
