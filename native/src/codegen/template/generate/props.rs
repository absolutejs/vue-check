//! An element's attributes and bound props: a port of
//! `codegen/template/elementProps.ts`.

use super::{
    ALL, Generator, NOT_TS2353, VERIFICATION, WITHOUT_HIGHLIGHT_AND_COMPLETION, WITHOUT_NAVIGATION,
};
use crate::codegen::template::interpolation::generate_identifier;
use crate::codegen::{Features, NEW_LINE, Source, generate_unicode, is_identifier};
use crate::js;
use crate::template::ast::{
    Attribute, ConstantType, Directive, Element, ElementType, Prop, SimpleExpression, Text,
};

/// An expression that isn't passed as a prop and is generated after the
/// element, so that it is checked all the same (`FailedPropExpressions`).
pub(super) struct FailedPropExpression<'a> {
    pub source: &'a str,
    pub offset: usize,
    pub prefix: &'static str,
    pub suffix: &'static str,
}

/// `hyphenateAttr`: the hyphenated form of a prop's name. A name that starts
/// with an upper-case letter starts with a hyphen.
fn hyphenate_attr(name: &str) -> String {
    let hyphenated = js::hyphenate(name);
    match name.chars().next() {
        // The original compares the first UTF-16 unit with its lower case.
        Some(first)
            if first.len_utf16() == 1 && !first.to_lowercase().eq(std::iter::once(first)) =>
        {
            format!("-{hyphenated}")
        }
        _ => hyphenated,
    }
}

impl<'a> Generator<'a> {
    /// `normalizeAttributeValue`: a value without its quotes, and where that
    /// starts.
    pub(super) fn attribute_value(&self, value: &Text) -> (&'a str, usize) {
        let source = self.source(&value.loc);
        let quoted = |quote: char| source.starts_with(quote) && source.ends_with(quote);
        if quoted('"') || quoted('\'') {
            (
                source.get(1..source.len().saturating_sub(1)).unwrap_or(""),
                value.loc.start.offset + 1,
            )
        } else {
            (source, value.loc.start.offset)
        }
    }

    fn failed(
        &self,
        expression: &SimpleExpression,
        prefix: &'static str,
        suffix: &'static str,
    ) -> FailedPropExpression<'a> {
        FailedPropExpression {
            source: self.source(&expression.loc),
            offset: expression.loc.start.offset,
            prefix,
            suffix,
        }
    }

    /// `getPropsCodeFeatures`.
    fn props_features(check_unknown_props: bool) -> Features {
        if check_unknown_props {
            VERIFICATION
        } else {
            NOT_TS2353
        }
    }

    /// `getShouldCamelize`: a hyphenated prop of a component is its
    /// camel-case prop.
    fn should_camelize(
        &mut self,
        node: &Element,
        is_static_or_attribute: bool,
        name: &str,
    ) -> bool {
        matches!(node.tag_type, ElementType::Component | ElementType::Slot)
            && is_static_or_attribute
            && hyphenate_attr(name) == name
            && (node.tag_type == ElementType::Slot || !self.is_html_attribute(name))
    }

    /// `getModelPropName`, for the default `experimentalModelPropName`: which
    /// prop `v-model` binds. `None` is the original's `undefined`.
    fn model_prop_name(node: &Element) -> Option<&'static str> {
        let has_text_type = || {
            node.props.iter().any(|prop| {
                matches!(prop, Prop::Attribute(attribute)
                    if attribute.name == "type"
                        && attribute.value.as_ref().is_some_and(|value| value.content == "text"))
            })
        };
        match node.tag.as_str() {
            "input" if has_text_type() => Some("value"),
            "input" => None,
            "textarea" | "select" => Some("value"),
            _ => Some("modelValue"),
        }
    }

    /// `generateElementProps`.
    #[expect(
        clippy::too_many_lines,
        reason = "one function in the original, in the same order"
    )]
    pub(super) fn element_props(
        &mut self,
        node: &'a Element,
        props: &[&'a Prop],
        check_unknown_props: bool,
        mut failed: Option<&mut Vec<FailedPropExpression<'a>>>,
        attrs_type: Option<&str>,
    ) {
        let is_component = node.tag_type == ElementType::Component;
        let mut fail = |expression: FailedPropExpression<'a>| {
            if let Some(failed) = failed.as_deref_mut() {
                failed.push(expression);
            }
        };

        // The event listeners first.
        for prop in props {
            let Prop::Directive(directive) = prop else {
                continue;
            };
            if directive.name != "on" {
                continue;
            }
            let arg_source = directive.arg.as_ref().map(|arg| self.source(&arg.loc));
            match (&directive.arg, arg_source) {
                (Some(arg), Some(source)) if !source.starts_with('[') && !source.ends_with(']') => {
                    if is_component {
                        self.text(format!(
                            "...{{ '{}': {} }},",
                            js::camelize(&format!("on-{source}")),
                            self.as_type("any")
                        ));
                    } else {
                        self.text("...{ ");
                        self.event_arg(source, arg.loc.start.offset, "on", None);
                        self.text(": ");
                        let args_type = match attrs_type {
                            Some(attrs_type) => format!(
                                "__VLS_EventArgs<{attrs_type}, '{}'>",
                                super::events::event_arg_name(source, "on")
                            ),
                            None => "any[]".to_owned(),
                        };
                        self.event_expression(directive, &args_type);
                        self.text("},");
                    }
                    self.text(NEW_LINE);
                }
                (Some(arg), Some(source)) if source.starts_with('[') && source.ends_with(']') => {
                    if let Some(exp) = &directive.exp {
                        fail(self.failed(arg, "(", ")"));
                        fail(self.failed(exp, "() => {", "}"));
                    }
                }
                (None, _) => {
                    if let Some(exp) = &directive.exp {
                        fail(self.failed(exp, "(", ")"));
                    }
                }
                _ => {}
            }
        }

        for prop in props {
            match prop {
                Prop::Directive(directive)
                    if (directive.name == "bind" && directive.arg.is_some())
                        || directive.name == "model" =>
                {
                    let mut prop_name = match &directive.arg {
                        Some(arg) if arg.const_type == ConstantType::CanStringify => {
                            Some(arg.content.as_str())
                        }
                        Some(arg) => Some(self.source(&arg.loc)),
                        None => Self::model_prop_name(node),
                    };
                    if prop_name.is_none_or(|name| self.is_data_attribute(name)) {
                        if let Some(exp) = &directive.exp
                            && exp.const_type != ConstantType::CanStringify
                        {
                            fail(self.failed(exp, "(", ")"));
                        }
                        continue;
                    }
                    // `.prop` and `.attr` have put a `.` or a `^` on the name.
                    if directive.name == "bind"
                        && directive.modifiers.iter().any(|modifier| {
                            modifier.content == "prop" || modifier.content == "attr"
                        })
                    {
                        prop_name = prop_name.map(|name| {
                            let mut chars = name.chars();
                            chars.next();
                            chars.as_str()
                        });
                    }
                    let Some(prop_name) = prop_name else { continue };

                    let should_spread = prop_name == "style" || prop_name == "class";
                    let is_static = directive.arg.as_ref().is_some_and(|arg| arg.is_static);
                    let should_camelize = self.should_camelize(node, is_static, prop_name);
                    let features = Self::props_features(check_unknown_props);

                    if should_spread {
                        self.text("...{ ");
                    }
                    let boundary = self.boundary(
                        directive.loc.start.offset,
                        directive.loc.end.offset,
                        VERIFICATION,
                    );
                    if let Some(arg) = &directive.arg {
                        self.object_property(
                            prop_name,
                            arg.loc.start.offset,
                            features,
                            should_camelize,
                            false,
                        );
                    } else {
                        let start = directive.loc.start.offset;
                        let name_boundary = self.boundary(
                            start,
                            start + "v-model".len(),
                            WITHOUT_HIGHLIGHT_AND_COMPLETION,
                        );
                        self.text(prop_name);
                        self.out.push(name_boundary.end());
                    }
                    self.text(": ");
                    let arg_loc = directive
                        .arg
                        .as_ref()
                        .map_or(&directive.loc, |arg| &arg.loc);
                    let value_boundary =
                        self.boundary(arg_loc.start.offset, arg_loc.end.offset, VERIFICATION);
                    self.prop_exp(directive, directive.exp.as_ref());
                    self.out.push(value_boundary.end());
                    self.out.push(boundary.end());
                    if should_spread {
                        self.text(" }");
                    }
                    self.text(format!(",{NEW_LINE}"));

                    if is_component && directive.name == "model" && !directive.modifiers.is_empty()
                    {
                        let property_name = match &directive.arg {
                            Some(arg) if !arg.is_static => {
                                format!("[__VLS_tryAsConstant(`${{{}}}Modifiers`)]", arg.content)
                            }
                            Some(_) => format!("{}Modifiers", js::camelize(prop_name)),
                            None => "modelModifiers".to_owned(),
                        };
                        self.modifiers(directive, &property_name, true);
                        self.text(NEW_LINE);
                    }
                }
                Prop::Attribute(attribute) => self.attribute(node, attribute, check_unknown_props),
                // `v-bind="object"`
                Prop::Directive(directive)
                    if directive.name == "bind" && directive.arg.is_none() =>
                {
                    let Some(exp) = &directive.exp else { continue };
                    if self.source(&exp.loc) == "$attrs" {
                        fail(self.failed(exp, "(", ")"));
                    } else {
                        let boundary =
                            self.boundary(exp.loc.start.offset, exp.loc.end.offset, VERIFICATION);
                        self.text("...");
                        self.prop_exp(directive, Some(exp));
                        self.out.push(boundary.end());
                        self.text(format!(",{NEW_LINE}"));
                    }
                }
                Prop::Directive(_) => {}
            }
        }
    }

    fn attribute(&mut self, node: &Element, attribute: &'a Attribute, check_unknown_props: bool) {
        if self.is_data_attribute(&attribute.name) {
            return;
        }
        let name = attribute.name.as_str();
        let should_spread = name == "style" || name == "class";
        let should_camelize = self.should_camelize(node, true, name);
        let features = Self::props_features(check_unknown_props);
        let start = attribute.loc.start.offset;

        if should_spread {
            self.text("...{ ");
        }
        let boundary = self.boundary(start, attribute.loc.end.offset, VERIFICATION);
        let prefix = self
            .options
            .template
            .get(start..)
            .and_then(|rest| rest.chars().next());
        if matches!(prefix, Some('.' | '#')) {
            // Pug's shorthand: every character maps to the shorthand.
            for char in name.chars() {
                self.mapped(char.to_string(), start, features);
            }
        } else {
            self.object_property(name, start, features, should_camelize, false);
        }
        self.text(": ");
        if name == "style" {
            self.text("{}");
        } else if let Some(value) = &attribute.value {
            // `generateAttrValue`
            let quote = if self.source(&value.loc).starts_with('\'') {
                "'"
            } else {
                "\""
            };
            let (content, offset) = self.attribute_value(value);
            self.text(quote);
            let token = self.ctx.token();
            generate_unicode(&mut self.out, token, content, offset, WITHOUT_NAVIGATION);
            self.text(quote);
        } else {
            self.text("true");
        }
        self.out.push(boundary.end());
        if should_spread {
            self.text(" }");
        }
        self.text(format!(",{NEW_LINE}"));
    }

    /// `generatePropExp`: a bound prop's value.
    pub(super) fn prop_exp(&mut self, directive: &Directive, exp: Option<&SimpleExpression>) {
        let Some(exp) = exp else {
            self.text("{}");
            return;
        };
        let source = self.source(&exp.loc);
        let exp_start = exp.loc.start.offset;
        // `:name` is `:name="name"`: the expression is where the argument is.
        let is_shorthand = directive.arg.as_ref().map(|arg| arg.loc.start.offset)
            == directive.exp.as_ref().map(|exp| exp.loc.start.offset);
        if !is_shorthand {
            self.interpolation(ALL, source, exp_start, "(", ")", false);
            return;
        }
        let variable_name = js::camelize(source);
        if is_identifier(&variable_name) {
            // `__shorthandExpression` is for an editor.
            let codes = self.buffered(|generator| {
                generator.camelized(source, exp_start, WITHOUT_HIGHLIGHT_AND_COMPLETION);
            });
            let options = self.identifier_options();
            generate_identifier(
                &options,
                &mut self.ctx,
                codes,
                &variable_name,
                Source::Template,
                exp_start,
                exp.loc.end.offset,
                false,
                false,
                false,
                &mut self.out,
            );
        }
    }
}
