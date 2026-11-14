//! What the styles contribute to the generated code: a port of
//! `codegen/style` (the scoped classes, the CSS modules, and the expressions
//! of `v-bind()`).

use std::collections::HashSet;

use super::template::context::TemplateContext;
use super::template::interpolation::{IdentifierOptions, generate_interpolation};
use super::{
    Boundary, Code, END_OF_LINE, Features, NEW_LINE, Source, Verification, generate_escaped,
    is_identifier,
};
use crate::style::StyleText;

/// A `<style>` block, as the codegen reads it (`IRStyle`).
#[derive(Clone, Debug)]
pub struct Style {
    pub scoped: bool,
    /// `module`, or `module="name"`: the name, and where it is in the file.
    pub module: Option<(String, usize)>,
    pub bindings: Vec<StyleText>,
    pub class_names: Vec<StyleText>,
}

/// `resolveStyleClassNames`: which styles' class names are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveClassNames {
    None,
    Scoped,
    All,
}

#[derive(Debug)]
pub struct StyleOptions<'a> {
    pub core: crate::codegen::CoreVersion,
    pub styles: &'a [Style],
    pub is_ts: bool,
    pub lib: &'a str,
    pub resolve_class_names: ResolveClassNames,
    pub strict_css_modules: bool,
    pub setup_refs: &'a HashSet<String>,
    pub setup_consts: &'a HashSet<String>,
    pub setup_bindings: &'a HashSet<String>,
    pub dot_value_bindings: &'a HashSet<String>,
}

/// What `generateStyle` returns, as far as the script codegen reads it.
#[derive(Debug)]
pub struct GeneratedStyle {
    pub codes: Vec<Code>,
    pub generated_types: Vec<String>,
    pub dot_value_accesses: Vec<String>,
    pub unsupported: Option<&'static str>,
}

const NAVIGATION: Features = Features::new(Verification::Off);
const NAVIGATION_AND_VERIFICATION: Features = Features::new(Verification::On);

struct Generator<'a> {
    options: &'a StyleOptions<'a>,
    ctx: TemplateContext,
    out: Vec<Code>,
}

/// `generateStyle`. `first_token` keeps its combine tokens apart from the
/// template's.
pub fn generate_style(options: &StyleOptions, first_token: u32) -> GeneratedStyle {
    let mut generator = Generator {
        options,
        ctx: TemplateContext::starting_at(first_token),
        out: Vec::new(),
    };
    generator.ctx.core = options.core;
    generator.ctx.open_scope();
    generator.scoped_classes();
    generator.modules();
    generator.css_vars();
    generator.ctx.end_scope(&mut generator.out);
    GeneratedStyle {
        codes: generator.out,
        generated_types: generator.ctx.generated_types.clone(),
        dot_value_accesses: generator.ctx.dot_value_accesses.clone(),
        unsupported: generator.ctx.unsupported,
    }
}

impl Generator<'_> {
    fn text(&mut self, text: impl Into<String>) {
        self.out.push(Code::Text(text.into()));
    }

    fn mapped(
        &mut self,
        text: impl Into<String>,
        source: Source,
        offset: usize,
        features: Features,
    ) {
        self.out.push(Code::Mapped {
            text: text.into(),
            source,
            offset,
            features,
        });
    }

    fn boundary(
        &mut self,
        source: Source,
        start: usize,
        end: usize,
        features: Features,
    ) -> Boundary {
        let token = self.ctx.token();
        Boundary::start(&mut self.out, token, source, start, end, features)
    }

    /// `generateTypeAlias`.
    fn type_alias(&mut self, name: &str, write_type: impl FnOnce(&mut Self)) {
        if self.options.is_ts {
            self.text(format!("type {name} = "));
            write_type(self);
            self.text(END_OF_LINE);
        } else {
            self.text("/** @typedef {");
            write_type(self);
            self.text(format!("}} {name} */{END_OF_LINE}"));
        }
    }

    /// `generateClassProperty`: `& { 'name': type }`.
    fn class_property(&mut self, source: Source, class_name: &StyleText, property_type: &str) {
        let offset = class_name.offset;
        self.text(format!("{NEW_LINE} & {{ "));
        let boundary = self.boundary(source, offset, offset + class_name.text.len(), NAVIGATION);
        self.text("'");
        // Without its dot.
        self.mapped(
            class_name.text.get(1..).unwrap_or(""),
            source,
            offset + 1,
            boundary.features,
        );
        self.text("'");
        self.out.push(boundary.end());
        self.text(format!(": {property_type}"));
        self.text(" }");
    }

    /// `generateStyleScopedClasses`: the type of the classes a template can
    /// use.
    fn scoped_classes(&mut self) {
        let options = self.options;
        if options.resolve_class_names == ResolveClassNames::None {
            return;
        }
        let scoped: Vec<(usize, &Style)> = options
            .styles
            .iter()
            .enumerate()
            .filter(|(_, style)| {
                options.resolve_class_names == ResolveClassNames::All || style.scoped
            })
            .collect();
        if scoped.is_empty() {
            return;
        }
        self.ctx.add_generated_type("__VLS_StyleScopedClasses");

        // A class declared again is a reference to the first one.
        let mut visited: HashSet<&str> = HashSet::new();
        let mut deferred: Vec<(Source, &str, usize)> = Vec::new();
        self.type_alias("__VLS_StyleScopedClasses", |generator| {
            generator.text("{}");
            for (index, style) in &scoped {
                let source = Source::Style(*index);
                for class_name in &style.class_names {
                    if visited.insert(class_name.text.as_str()) {
                        generator.class_property(source, class_name, "boolean");
                    } else {
                        deferred.push((
                            source,
                            class_name.text.get(1..).unwrap_or(""),
                            class_name.offset + 1,
                        ));
                    }
                }
            }
        });
        if options.is_ts {
            // A reference in a JSDoc comment isn't a use (TS6196).
            self.text(format!(
                "void ({{}} as __VLS_StyleScopedClasses){END_OF_LINE}"
            ));
        }
        for (source, class_name, offset) in deferred {
            self.scoped_class_reference(source, class_name, offset);
        }
    }

    /// `generateStyleScopedClassReference`, for a class of a style.
    fn scoped_class_reference(&mut self, source: Source, class_name: &str, offset: usize) {
        if class_name.is_empty() {
            self.text("/** @type {__VLS_StyleScopedClasses['");
            // The original has `'template'` here whatever the block.
            self.mapped(
                "",
                Source::Template,
                offset,
                Features::new(Verification::Off),
            );
            self.text(format!("']}} */{END_OF_LINE}"));
            return;
        }
        self.text("/** @type {__VLS_StyleScopedClasses[");
        // `codeFeatures.navigationAndCompletion`
        let boundary = self.boundary(
            source,
            offset,
            offset + class_name.len(),
            Features::new(Verification::Off),
        );
        self.text("'");
        let token = self.ctx.token();
        generate_escaped(
            &mut self.out,
            token,
            class_name,
            source,
            offset,
            boundary.features,
        );
        self.text("'");
        self.out.push(boundary.end());
        self.text(format!("]}} */{END_OF_LINE}"));
    }

    /// `generateStyleModules`: the type of `$style` and of each named module.
    fn modules(&mut self) {
        let options = self.options;
        let modules: Vec<(usize, &Style, &(String, usize))> = options
            .styles
            .iter()
            .enumerate()
            .filter_map(|(index, style)| Some((index, style, style.module.as_ref()?)))
            .collect();
        if modules.is_empty() {
            return;
        }
        self.ctx.add_generated_type("__VLS_StyleModules");
        self.type_alias("__VLS_StyleModules", |generator| {
            generator.text(format!("{{{NEW_LINE}"));
            for (index, style, (name, offset)) in &modules {
                if name.is_empty() {
                    generator.mapped(
                        "$style",
                        Source::Main,
                        *offset,
                        Features::new(Verification::On),
                    );
                } else if is_identifier(name) {
                    generator.mapped(
                        name.clone(),
                        Source::Main,
                        *offset,
                        NAVIGATION_AND_VERIFICATION,
                    );
                } else {
                    let boundary = generator.boundary(
                        Source::Main,
                        *offset,
                        offset + name.len(),
                        NAVIGATION_AND_VERIFICATION,
                    );
                    generator.text("'");
                    generator.mapped(name.clone(), Source::Main, *offset, boundary.features);
                    generator.text("'");
                    generator.out.push(boundary.end());
                }
                generator.text(": ");
                if !options.strict_css_modules {
                    generator.text("Record<string, string> & ");
                }
                generator.text("__VLS_PrettifyGlobal<{}");
                for class_name in &style.class_names {
                    generator.class_property(Source::Style(*index), class_name, "string");
                }
                generator.text(format!(">{END_OF_LINE}"));
            }
            generator.text("}");
        });
    }

    /// `generateCssVars`: the expressions of `v-bind()`.
    fn css_vars(&mut self) {
        let options = self.options;
        let no_reasserts = HashSet::new();
        let identifier_options = IdentifierOptions {
            setup_refs: options.setup_refs,
            setup_consts: options.setup_consts,
            setup_bindings: options.setup_bindings,
            dot_value_bindings: options.dot_value_bindings,
            reassert_bindings: &no_reasserts,
            lib: options.lib,
            is_ts: options.is_ts,
            core: options.core,
        };
        for (index, style) in options.styles.iter().enumerate() {
            for binding in &style.bindings {
                generate_interpolation(
                    &identifier_options,
                    &mut self.ctx,
                    Source::Style(index),
                    Features::new(Verification::On),
                    &binding.text,
                    binding.offset,
                    "(",
                    ")",
                    false,
                    &mut self.out,
                );
                self.text(END_OF_LINE);
            }
        }
    }
}
