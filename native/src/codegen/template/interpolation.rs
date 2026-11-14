//! A template expression as TypeScript: a port of
//! `codegen/template/interpolation.ts`.
//!
//! The expression's text is copied, mapped to where it came from, with each
//! identifier that refers to the component rewritten to read it from the
//! component's context.

use std::collections::HashSet;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ArrowFunctionExpression, BindingPattern, Function, FunctionBody, Program, Statement,
    StaticBlock,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;

use super::binding_references::{ReferenceKind, references};
use super::context::TemplateContext;
use crate::codegen::{
    Boundary, Code, CoreVersion, END_OF_LINE, Features, NEW_LINE, Source, Verification, is_identifier,
    ref_brand_argument,
};

/// What deciding how an identifier is read depends on (`IdentifierOptions`).
#[derive(Debug)]
pub struct IdentifierOptions<'o> {
    pub setup_refs: &'o HashSet<String>,
    pub setup_consts: &'o HashSet<String>,
    pub setup_bindings: &'o HashSet<String>,
    /// The bindings read with `.value` everywhere.
    pub dot_value_bindings: &'o HashSet<String>,
    /// The bindings each function body asserts again. Empty where the
    /// original has none.
    pub reassert_bindings: &'o HashSet<String>,
    /// `vueCompilerOptions.lib`.
    pub lib: &'o str,
    /// Whether the component's script is TypeScript.
    pub is_ts: bool,
    pub core: CoreVersion,
}

fn at(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// Parses a template expression as the original does (`getTypeScriptAST`: a
/// TypeScript file) and hands its tree to `read`. `None` when it has a
/// syntax error, which TypeScript recovers from in its own way.
pub(super) fn with_program<T>(text: &str, read: impl FnOnce(&Program<'_>) -> T) -> Option<T> {
    let allocator = Allocator::default();
    // TypeScript's parser takes a `return` anywhere (`@click="if (done) return; next()"`).
    let options = ParseOptions {
        allow_return_outside_function: true,
        ..ParseOptions::default()
    };
    // As a script, where `await` is a name: the original parses the
    // expression as a file with no import in it, in which TypeScript reads
    // `await (value)` as a call of something named `await`. (A real `await`
    // outside an `async` function is then a syntax error here, and the file
    // goes to the original.) Not oxc's "module or script, whichever parses"
    // mode: that one can return a tree with part of the text missing and no
    // error.
    // In a script, JavaScript also takes `<!--` and `-->` as the start of a
    // comment. TypeScript never does (`a<!--b` is `a < !--b`), so an
    // expression with either goes to the original.
    if text.contains("<!--") || text.contains("-->") {
        return None;
    }
    let parsed = Parser::new(&allocator, text, SourceType::ts().with_script(true))
        .with_options(options)
        .parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        if std::env::var_os("VUE_CHECK_NATIVE_EXPLAIN").is_some() {
            for diagnostic in &parsed.diagnostics {
                eprintln!("template expression `{text}`: {diagnostic}");
            }
        }
        return None;
    }
    Some(read(&parsed.program))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyKind {
    /// `{ ... }`
    Block,
    /// `() => expression`
    Expression,
    /// A parameter's default, or a computed name in a parameter.
    Parameter,
}

/// A function scope: where its code starts and ends in the expression.
#[derive(Clone, Copy, Debug)]
struct Body {
    start: usize,
    end: usize,
    kind: BodyKind,
}

/// `collectFunctionBodies`: the function scopes of an expression, outermost
/// first: what a parameter list evaluates, then the body.
struct BodyCollector<'t> {
    text: &'t str,
    bodies: Vec<Body>,
}

impl<'a> BodyCollector<'_> {
    fn parameter_expression(&mut self, span: Span, visit: impl FnOnce(&mut Self)) {
        self.bodies.push(Body {
            start: at(span.start),
            end: at(span.end),
            kind: BodyKind::Parameter,
        });
        visit(self);
    }

    /// `visitBinding`: the computed names and defaults of a pattern.
    fn binding(&mut self, pattern: &BindingPattern<'a>) {
        let element = |collector: &mut Self, pattern: &BindingPattern<'a>| match pattern {
            BindingPattern::AssignmentPattern(assignment) => {
                collector.binding(&assignment.left);
                collector.parameter_expression(assignment.right.span(), |collector| {
                    collector.visit_expression(&assignment.right);
                });
            }
            other => collector.binding(other),
        };
        match pattern {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    if property.computed
                        && let Some(key) = property.key.as_expression()
                    {
                        self.parameter_expression(key.span(), |collector| {
                            collector.visit_expression(key);
                        });
                    }
                    element(self, &property.value);
                }
                if let Some(rest) = &object.rest {
                    self.binding(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for item in array.elements.iter().flatten() {
                    element(self, item);
                }
                if let Some(rest) = &array.rest {
                    self.binding(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => element(self, &assignment.left),
        }
    }

    /// A block body: from after its directives to its closing brace. A body
    /// with nothing after its directives isn't one.
    fn block_body(
        &mut self,
        open_brace: usize,
        close_brace: usize,
        last_directive_end: Option<usize>,
        is_empty: bool,
    ) {
        if is_empty {
            return;
        }
        self.bodies.push(Body {
            start: last_directive_end.unwrap_or(open_brace + 1),
            end: close_brace,
            kind: BodyKind::Block,
        });
    }

    fn function_body(&mut self, body: &FunctionBody<'a>) {
        self.block_body(
            at(body.span.start),
            at(body.span.end).saturating_sub(1),
            body.directives
                .last()
                .map(|directive| at(directive.span.end)),
            body.statements.is_empty(),
        );
        walk::walk_function_body(self, body);
    }
}

impl<'a> Visit<'a> for BodyCollector<'_> {
    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        let Some(body) = &function.body else {
            walk::walk_function(self, function, flags);
            return;
        };
        if let Some(type_parameters) = &function.type_parameters {
            self.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &function.this_param {
            self.visit_ts_this_parameter(this_param);
        }
        for param in &function.params.items {
            self.visit_decorators(&param.decorators);
            self.binding(&param.pattern);
            if let Some(annotation) = &param.type_annotation {
                self.visit_ts_type_annotation(annotation);
            }
            if let Some(initializer) = &param.initializer {
                self.parameter_expression(initializer.span(), |collector| {
                    collector.visit_expression(initializer);
                });
            }
        }
        if let Some(rest) = &function.params.rest {
            self.binding(&rest.rest.argument);
            if let Some(annotation) = &rest.type_annotation {
                self.visit_ts_type_annotation(annotation);
            }
        }
        if let Some(return_type) = &function.return_type {
            self.visit_ts_type_annotation(return_type);
        }
        self.function_body(body);
    }

    fn visit_arrow_function_expression(&mut self, function: &ArrowFunctionExpression<'a>) {
        if let Some(type_parameters) = &function.type_parameters {
            self.visit_ts_type_parameter_declaration(type_parameters);
        }
        for param in &function.params.items {
            self.binding(&param.pattern);
            if let Some(annotation) = &param.type_annotation {
                self.visit_ts_type_annotation(annotation);
            }
            if let Some(initializer) = &param.initializer {
                self.parameter_expression(initializer.span(), |collector| {
                    collector.visit_expression(initializer);
                });
            }
        }
        if let Some(rest) = &function.params.rest {
            self.binding(&rest.rest.argument);
            if let Some(annotation) = &rest.type_annotation {
                self.visit_ts_type_annotation(annotation);
            }
        }
        if let Some(return_type) = &function.return_type {
            self.visit_ts_type_annotation(return_type);
        }
        if function.expression
            && let Some(Statement::ExpressionStatement(body)) = function.body.statements.first()
        {
            let span = body.expression.span();
            self.bodies.push(Body {
                start: at(span.start),
                end: at(span.end),
                kind: BodyKind::Expression,
            });
            self.visit_expression(&body.expression);
        } else {
            self.function_body(&function.body);
        }
    }

    fn visit_static_block(&mut self, block: &StaticBlock<'a>) {
        // The block's braces, after `static`.
        let start = at(block.span.start);
        let open_brace = self
            .text
            .get(start..)
            .and_then(|rest| rest.find('{'))
            .map_or(start, |found| start + found);
        // TypeScript finds the directives of any block; a static block has
        // none worth the name.
        let directives = block
            .body
            .iter()
            .take_while(|statement| {
                matches!(statement, Statement::ExpressionStatement(statement)
                    if matches!(statement.expression, oxc_ast::ast::Expression::StringLiteral(_)))
            })
            .count();
        self.block_body(
            open_brace,
            at(block.span.end).saturating_sub(1),
            directives
                .checked_sub(1)
                .and_then(|last| block.body.get(last))
                .map(|statement| at(statement.span().end)),
            directives == block.body.len(),
        );
        walk::walk_static_block(self, block);
    }
}

/// A function body being generated: its code is held back until its end,
/// when the bindings it reads are known.
struct Frame {
    body: Body,
    codes: Vec<Code>,
    reasserts: Vec<String>,
}

struct Interpolation<'i, 'o> {
    options: &'i IdentifierOptions<'o>,
    source: Source,
    features: Features,
    code: &'i str,
    start: usize,
    /// Where each function body starts and ends, in the order they are met.
    marks: Vec<(usize, Body, bool)>,
    mark_index: usize,
    frames: Vec<Frame>,
}

/// An identifier of the expression that refers to the component, and where
/// it is in the expression.
struct Identifier {
    name: String,
    offset: usize,
    kind: ReferenceKind,
    is_narrowing: bool,
    in_type_query: bool,
}

/// `forEachIdentifiers`: the identifiers that refer to the component.
fn for_each_identifiers(
    context: &mut TemplateContext,
    code: &str,
    text: &str,
    prefix: &str,
    in_narrowing: bool,
    version: CoreVersion,
) -> Vec<Identifier> {
    let plain = |name: &str| Identifier {
        name: name.to_owned(),
        offset: 0,
        kind: ReferenceKind::Plain,
        is_narrowing: in_narrowing,
        in_type_query: false,
    };
    // An empty expression isn't parsed: TypeScript recovers from the `()` it
    // becomes by putting an identifier with no name in it, which is then
    // read from the context like any other (`__VLS_ctx.`). A call's
    // parentheses, a function's braces or nothing at all need no expression.
    if crate::js::trim(code).is_empty() {
        if prefix == "(" {
            // Unless a scope has declared a name that is empty.
            return if context.should_identifier_skipped("") {
                Vec::new()
            } else {
                vec![plain("")]
            };
        }
        if !(prefix.is_empty() || prefix.ends_with('(') || prefix.ends_with('{')) {
            context.give_up("an empty template expression in a place the port has no rule for");
        }
        return Vec::new();
    }
    if is_identifier(code) {
        return if context.should_identifier_skipped(code) {
            Vec::new()
        } else {
            vec![plain(code)]
        };
    }
    let Some(found) = with_program(text, |program| {
        if version < CoreVersion::V3_3_12 {
            super::binding_references_old::references(program, text, context)
        } else {
            references(program, text, context, in_narrowing)
        }
    }) else {
        context.give_up("a template expression has a syntax error");
        return Vec::new();
    };
    found
        .into_iter()
        .filter_map(|reference| {
            Some(Identifier {
                // An identifier of the prefix isn't the expression's.
                offset: reference.start.checked_sub(prefix.len())?,
                name: text.get(reference.start..reference.end)?.to_owned(),
                kind: reference.kind,
                is_narrowing: reference.is_narrowing,
                in_type_query: reference.in_type_query,
            })
        })
        .collect()
}

/// `generateInterpolation` as 3.3.11 and before have it: the text between
/// the identifiers as it is, a `<script setup>` ref read with `.value`, and
/// every other identifier read from the context.
#[expect(clippy::too_many_arguments, reason = "the original's own parameters")]
fn generate_interpolation_3_3_11(
    options: &IdentifierOptions,
    context: &mut TemplateContext,
    source: Source,
    features: Features,
    code: &str,
    start: usize,
    prefix: &str,
    suffix: &str,
    out: &mut Vec<Code>,
) {
    let mapped = |text: &str, offset: usize| Code::Mapped {
        text: text.to_owned(),
        source,
        offset,
        features,
    };
    if !prefix.is_empty() {
        out.push(prefix.into());
    }
    let text = format!("{prefix}{code}{suffix}");
    let found = for_each_identifiers(context, code, &text, prefix, false, options.core);
    let mut previous_end = 0;
    for Identifier {
        name, offset, kind, ..
    } in found
    {
        let end = offset + name.len();
        if kind == ReferenceKind::Shorthand {
            out.push(mapped(
                code.get(previous_end..end).unwrap_or(""),
                start + previous_end,
            ));
            out.push(": ".into());
        } else if previous_end < offset {
            out.push(mapped(
                code.get(previous_end..offset).unwrap_or(""),
                start + previous_end,
            ));
        }
        if options.setup_refs.contains(&name) {
            out.push(mapped(&name, start + offset));
            out.push(".value".into());
        } else {
            let token = context.token();
            let boundary = Boundary::start(
                out,
                token,
                source,
                start + offset,
                start + end,
                Features::new(Verification::On),
            );
            if context.dollar_vars.contains(&name) {
                out.push("__VLS_dollars".into());
            } else {
                context.access_variable(source, &name, Some(start + offset), false, None);
                out.push("__VLS_ctx".into());
            }
            out.push(".".into());
            out.push(mapped(&name, start + offset));
            out.push(boundary.end());
        }
        previous_end = end;
    }
    if previous_end < code.len() {
        out.push(mapped(
            code.get(previous_end..).unwrap_or(""),
            start + previous_end,
        ));
    }
    if !suffix.is_empty() {
        out.push(suffix.into());
    }
}

/// `generateInterpolation`.
#[expect(clippy::too_many_arguments, reason = "the original's own parameters")]
pub fn generate_interpolation(
    options: &IdentifierOptions,
    context: &mut TemplateContext,
    source: Source,
    features: Features,
    code: &str,
    start: usize,
    prefix: &str,
    suffix: &str,
    in_narrowing: bool,
    out: &mut Vec<Code>,
) {
    if options.core < CoreVersion::V3_3_12 {
        generate_interpolation_3_3_11(
            options, context, source, features, code, start, prefix, suffix, out,
        );
        return;
    }
    if !prefix.is_empty() {
        out.push(prefix.into());
    }
    let text = format!("{prefix}{code}{suffix}");
    let is_plain_identifier = is_identifier(code) || crate::js::trim(code).is_empty();

    // The function bodies, when some binding has to be asserted again inside
    // each: assertion narrowing doesn't flow into a function.
    let mut bodies = Vec::new();
    // 3.3.14's: before it, nothing is asserted again inside a function.
    if options.core.is_3_3_14() && !options.reassert_bindings.is_empty() && !is_plain_identifier {
        let collected = with_program(&text, |program| {
            let mut collector = BodyCollector {
                text: &text,
                bodies: Vec::new(),
            };
            collector.visit_program(program);
            collector.bodies
        });
        match collected {
            Some(collected) => bodies = collected,
            None => context.give_up("a template expression has a syntax error"),
        }
        for body in &mut bodies {
            body.start = body.start.saturating_sub(prefix.len());
            body.end = body.end.saturating_sub(prefix.len());
        }
    }
    // Ends before starts at the same offset: the inner end and the outer
    // start come first (the sort is stable).
    let mut marks: Vec<(usize, Body, bool)> = bodies
        .iter()
        .rev()
        .map(|body| (body.end, *body, false))
        .chain(bodies.iter().map(|body| (body.start, *body, true)))
        .collect();
    marks.sort_by_key(|(offset, ..)| *offset);

    let found = for_each_identifiers(context, code, &text, prefix, in_narrowing, options.core);

    let mut interpolation = Interpolation {
        options,
        source,
        features,
        code,
        start,
        marks,
        mark_index: 0,
        frames: Vec::new(),
    };
    let mut previous_end = 0;
    for Identifier {
        name,
        offset,
        kind,
        is_narrowing,
        in_type_query,
    } in found
    {
        if kind == ReferenceKind::Shorthand {
            interpolation.generate_code_until(context, previous_end, offset + name.len(), out);
            interpolation.write(vec![": ".into()], out);
        } else {
            interpolation.generate_code_until(context, previous_end, offset, out);
        }
        if let Some(frame) = interpolation.frames.last_mut()
            && options.reassert_bindings.contains(&name)
            && !options.setup_consts.contains(&name)
            && !options.setup_refs.contains(&name)
            && !context.should_identifier_skipped(&name)
            && !frame.reasserts.contains(&name)
        {
            frame.reasserts.push(name.clone());
        }
        let codes = vec![Code::Mapped {
            text: name.clone(),
            source,
            offset: start + offset,
            // `__shorthandExpression` is for an editor.
            features,
        }];
        let mut generated = Vec::new();
        generate_identifier(
            options,
            context,
            codes,
            &name,
            source,
            start + offset,
            start + offset + name.len(),
            is_narrowing,
            in_type_query,
            kind == ReferenceKind::NewOperand,
            &mut generated,
        );
        interpolation.write(generated, out);
        previous_end = offset + name.len();
    }
    interpolation.generate_code_until(context, previous_end, code.len(), out);

    if !suffix.is_empty() {
        out.push(suffix.into());
    }
}

impl Interpolation<'_, '_> {
    /// Into the function body being held back, or out.
    fn write(&mut self, codes: Vec<Code>, out: &mut Vec<Code>) {
        match self.frames.last_mut() {
            Some(frame) => frame.codes.extend(codes),
            None => out.extend(codes),
        }
    }

    fn generate_code_until(
        &mut self,
        context: &mut TemplateContext,
        mut from: usize,
        end: usize,
        out: &mut Vec<Code>,
    ) {
        while let Some(&(offset, body, enter)) =
            self.marks.get(self.mark_index).filter(|mark| mark.0 <= end)
        {
            self.mark_index += 1;
            let codes = self.non_identifier_code(context, from, offset);
            self.write(codes, out);
            from = offset;
            if enter {
                self.frames.push(Frame {
                    body,
                    codes: Vec::new(),
                    reasserts: Vec::new(),
                });
            } else if let Some(frame) = self.frames.pop() {
                let codes = self.function_body(context, frame);
                self.write(codes, out);
            } else {
                // The original throws here: a body that ends before it starts.
                context.give_up("a function body in a template expression ends before it starts");
            }
        }
        let codes = self.non_identifier_code(context, from, end);
        self.write(codes, out);
    }

    /// `generateNonIdentifierCode`: the code between two identifiers. Its
    /// first character only verifies, so that what an editor does at the
    /// identifier before it doesn't also apply here.
    fn non_identifier_code(
        &self,
        context: &mut TemplateContext,
        from: usize,
        to: usize,
    ) -> Vec<Code> {
        let Some(code) = self.code.get(from..to).filter(|code| !code.is_empty()) else {
            return Vec::new();
        };
        let mapped = |text: &str, offset: usize, features: Features| Code::Mapped {
            text: text.to_owned(),
            source: self.source,
            offset: self.start + offset,
            features,
        };
        if from == 0 {
            return vec![mapped(code, from, self.features)];
        }
        // The original cuts off one UTF-16 unit, which is half of a character
        // outside the basic plane.
        let first_char = code.chars().next();
        if first_char.is_some_and(|char| char.len_utf16() > 1) {
            context.give_up("a template expression is cut inside a character");
        }
        let first = first_char.map_or(0, char::len_utf8);
        let (head, tail) = code.split_at(first);
        let mut codes = vec![mapped(head, from, self.features.verification_only())];
        if !tail.is_empty() {
            codes.push(mapped(tail, from + first, self.features));
        }
        codes
    }

    /// `generateFunctionBody`: a body's code, after the assertions of the
    /// bindings it reads.
    fn function_body(&self, context: &mut TemplateContext, frame: Frame) -> Vec<Code> {
        let Frame {
            body,
            codes,
            reasserts,
        } = frame;
        if reasserts.is_empty() {
            return codes;
        }
        let brand = ref_brand_argument(self.options.lib, self.options.is_ts);
        let asserted = reasserts
            .iter()
            .map(|name| Code::from(format!("__VLS_withDotValue({name}, {brand}){END_OF_LINE}")));
        let mut out = Vec::new();
        if body.kind == BodyKind::Block {
            out.push(NEW_LINE.into());
            out.extend(asserted);
            out.extend(codes);
            return out;
        }
        // An error on the generated call or on `return` maps back to the
        // expression.
        let verification = self.features.verification_only();
        let (start, end) = (self.start + body.start, self.start + body.end);
        if body.kind == BodyKind::Parameter {
            let boundary = Boundary::start(
                &mut out,
                context.token(),
                self.source,
                start,
                end,
                verification,
            );
            out.push(format!("(() => {{{NEW_LINE}").into());
            out.extend(asserted);
            out.push("return (".into());
            out.extend(codes);
            out.push(format!("){END_OF_LINE}}})()").into());
            out.push(boundary.end());
        } else {
            out.push(format!("{{{NEW_LINE}").into());
            out.extend(asserted);
            let boundary = Boundary::start(
                &mut out,
                context.token(),
                self.source,
                start,
                end,
                verification,
            );
            out.push("return".into());
            out.push(boundary.end());
            out.push(" (".into());
            out.extend(codes);
            out.push(format!("){END_OF_LINE}}}").into());
        }
        out
    }
}

/// `generateIdentifier`: how an identifier is read.
///
/// - a constant of `<script setup>`, a local or a global: as it is;
/// - a template ref: `.value`;
/// - a binding narrowed somewhere in the template: `.value` everywhere;
/// - another binding: through `__VLS_unwrap`, which keeps its type;
/// - anything else: from the component's context, `__VLS_ctx.name`.
#[expect(clippy::too_many_arguments, reason = "the original's own parameters")]
pub fn generate_identifier(
    options: &IdentifierOptions,
    context: &mut TemplateContext,
    codes: Vec<Code>,
    name: &str,
    source: Source,
    start: usize,
    end: usize,
    is_narrowing: bool,
    in_type_query: bool,
    is_new_operand: bool,
    out: &mut Vec<Code>,
) {
    let verification = Features::new(Verification::On);
    let dot_value = |context: &mut TemplateContext, out: &mut Vec<Code>| {
        out.push(".".into());
        let boundary = Boundary::start(out, context.token(), source, start, end, verification);
        out.push("value".into());
        out.push(boundary.end());
    };
    if options.setup_consts.contains(name) || context.should_identifier_skipped(name) {
        out.extend(codes);
    } else if options.setup_refs.contains(name) {
        out.extend(codes);
        dot_value(context, out);
    } else if options.setup_bindings.contains(name) {
        // The first pass records the narrowing accesses here; the second
        // reads them back as `dot_value_bindings`.
        context.access_variable(
            source,
            name,
            Some(start),
            in_type_query || is_narrowing,
            None,
        );
        if in_type_query || options.dot_value_bindings.contains(name) {
            out.extend(codes);
            dot_value(context, out);
        } else {
            // `new __VLS_unwrap(Foo)()` would call the result: parentheses.
            if is_new_operand {
                out.push("(".into());
            }
            out.push("__VLS_unwrap(".into());
            out.extend(codes);
            out.push(format!(", {})", ref_brand_argument(options.lib, options.is_ts)).into());
            if is_new_operand {
                out.push(")".into());
            }
        }
    } else {
        let boundary = Boundary::start(out, context.token(), source, start, end, verification);
        if context.dollar_vars.contains(name) {
            out.push("__VLS_dollars".into());
        } else {
            context.access_variable(source, name, Some(start), false, None);
            out.push("__VLS_ctx".into());
        }
        out.push(".".into());
        out.extend(codes);
        out.push(boundary.end());
    }
}
