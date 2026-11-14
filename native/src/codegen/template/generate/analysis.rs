//! What the template codegen reads from TypeScript's tree of a piece of a
//! template, other than its identifiers: the names a `v-for` binds, whether
//! a handler is one expression, a slot's parameters, the class names in a
//! `:class`. Each is read from oxc's tree of the same text.

use oxc_ast::ast::{
    ArrayExpressionElement, BindingPattern, Expression, FormalParameters, ObjectExpression,
    ObjectPropertyKind, Program, PropertyKey, PropertyKind, Statement,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

use super::super::interpolation::with_program;

fn at(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

fn range(span: Span) -> (usize, usize) {
    (at(span.start), at(span.end))
}

fn names_of(pattern: &BindingPattern, text: &str, names: &mut Vec<String>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => {
            let (start, end) = range(identifier.span);
            names.push(text.get(start..end).unwrap_or("").to_owned());
        }
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                names_of(&property.value, text, names);
            }
            if let Some(rest) = &object.rest {
                names_of(&rest.argument, text, names);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                names_of(element, text, names);
            }
            if let Some(rest) = &array.rest {
                names_of(&rest.argument, text, names);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => names_of(&assignment.left, text, names),
    }
}

/// `collectDefaultInitializers`: the defaults in a pattern, a nested
/// pattern's before the one of the element it is in.
fn default_initializers(pattern: &BindingPattern, out: &mut Vec<(usize, usize)>) {
    let element = |pattern: &BindingPattern, out: &mut Vec<(usize, usize)>| match pattern {
        BindingPattern::AssignmentPattern(assignment) => {
            default_initializers(&assignment.left, out);
            out.push(range(assignment.right.span()));
        }
        other => default_initializers(other, out),
    };
    match pattern {
        BindingPattern::BindingIdentifier(_) => {}
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                element(&property.value, out);
            }
            if let Some(rest) = &object.rest {
                default_initializers(&rest.argument, out);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for item in array.elements.iter().flatten() {
                element(item, out);
            }
            if let Some(rest) = &array.rest {
                default_initializers(&rest.argument, out);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => {
            default_initializers(&assignment.left, out);
        }
    }
}

/// The left side of a `v-for`, parsed as `const [<left>]`.
pub struct ForBindings {
    pub names: Vec<String>,
    /// Where each default is in the left side's text, in order.
    pub default_ranges: Vec<(usize, usize)>,
}

pub fn for_bindings(left: &str) -> Option<ForBindings> {
    const WRAP: &str = "const [";
    // The original parses `const [<left>]`, which TypeScript accepts and oxc
    // doesn't (a destructuring needs an initializer). One is added: it is
    // after everything that is read from the tree.
    let text = format!("{WRAP}{left}] = 0");
    with_program(&text, |program| {
        let [Statement::VariableDeclaration(declaration)] = program.body.as_slice() else {
            return None;
        };
        let [declarator] = declaration.declarations.as_slice() else {
            return None;
        };
        if declarator.type_annotation.is_some() {
            return None;
        }
        let mut names = Vec::new();
        names_of(&declarator.id, &text, &mut names);
        let mut ranges = Vec::new();
        default_initializers(&declarator.id, &mut ranges);
        let mut default_ranges = ranges
            .into_iter()
            .map(|(start, end)| {
                Some((start.checked_sub(WRAP.len())?, end.checked_sub(WRAP.len())?))
            })
            .collect::<Option<Vec<_>>>()?;
        default_ranges.sort_by_key(|(start, _)| *start);
        Some(ForBindings {
            names,
            default_ranges,
        })
    })
    .flatten()
}

/// How a `v-on` handler is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandlerShape {
    /// `isCompoundExpression`: statements, or an expression that isn't a
    /// function or a reference to one, which is wrapped in a function.
    pub is_compound: bool,
    /// `isSingleExpression`: one expression, with no semicolon after it.
    pub is_single_expression: bool,
}

pub fn handler_shape(content: &str) -> Option<HandlerShape> {
    if let Some(shape) = with_program(content, |program| handler_shape_of(program, content)) {
        return Some(shape);
    }
    // `function (event) { ... }`: a function declaration with no name, for
    // TypeScript, which then isn't wrapped in a function. oxc takes it as an
    // expression only, in parentheses, which is how it is generated.
    let is_anonymous_function = with_program(&format!("({content})"), |program| {
        matches!(program.body.as_slice(), [Statement::ExpressionStatement(statement)]
            if matches!(&statement.expression, Expression::ParenthesizedExpression(inner)
                if matches!(&inner.expression, Expression::FunctionExpression(function) if function.id.is_none())))
    });
    (is_anonymous_function == Some(true) && crate::js::trim_start(content).starts_with("function"))
        .then_some(HandlerShape {
            is_compound: false,
            is_single_expression: false,
        })
}

fn handler_shape_of(program: &Program, content: &str) -> HandlerShape {
    // TypeScript has a directive (`'use strict'`) as an expression statement.
    let statements = program.directives.len() + program.body.len();
    if statements == 0 {
        return HandlerShape {
            is_compound: false,
            is_single_expression: false,
        };
    }
    // Whether the last token is a `;`: comments and whitespace aside.
    let end = crate::script::trivia::full_start(content, &program.comments, content.len());
    let ends_with_semicolon = content.get(..end).is_some_and(|code| code.ends_with(';'));
    if statements != 1 || ends_with_semicolon {
        return HandlerShape {
            is_compound: true,
            is_single_expression: false,
        };
    }
    let is_reference = match program.body.first() {
        Some(Statement::ExpressionStatement(statement)) => {
            let mut expression = &statement.expression;
            loop {
                expression = match expression {
                    Expression::ParenthesizedExpression(inner) => &inner.expression,
                    Expression::TSAsExpression(inner) => &inner.expression,
                    Expression::TSTypeAssertion(inner) => &inner.expression,
                    Expression::TSNonNullExpression(inner) => &inner.expression,
                    Expression::TSSatisfiesExpression(inner) => &inner.expression,
                    other => break is_function_or_reference(other),
                };
            }
        }
        Some(Statement::FunctionDeclaration(_)) => {
            return HandlerShape {
                is_compound: false,
                is_single_expression: false,
            };
        }
        _ => false,
    };
    HandlerShape {
        is_compound: !is_reference,
        // One statement that is an expression statement (a directive is one).
        is_single_expression: !program.directives.is_empty()
            || matches!(
                program.body.first(),
                Some(Statement::ExpressionStatement(_))
            ),
    }
}

/// An arrow function, an identifier, or a property or element access.
fn is_function_or_reference(expression: &Expression) -> bool {
    match expression {
        Expression::ArrowFunctionExpression(_)
        | Expression::Identifier(_)
        | Expression::StaticMemberExpression(_)
        | Expression::ComputedMemberExpression(_)
        | Expression::PrivateFieldExpression(_) => true,
        // `a?.b` is a property access for TypeScript; `a?.()` is a call.
        Expression::ChainExpression(chain) => !matches!(
            chain.expression,
            oxc_ast::ast::ChainElement::CallExpression(_)
                | oxc_ast::ast::ChainElement::TSNonNullExpression(_)
        ),
        _ => false,
    }
}

/// A slot's parameters, parsed as `(<parameters>) => {}`.
pub struct SlotParameters {
    /// Whether the text is one arrow function, as it should be.
    pub is_arrow: bool,
    /// Per parameter: where its name ends, and where its type ends.
    pub parameters: Vec<(usize, Option<usize>)>,
    /// `collectBindingNames` over the whole tree: every identifier in it but
    /// the ones inside a pattern that aren't names.
    pub declared: Vec<String>,
}

/// Every identifier under a node, as TypeScript has them: names of every kind.
struct Identifiers<'t> {
    text: &'t str,
    names: Vec<String>,
}

impl Identifiers<'_> {
    fn push(&mut self, span: Span) {
        let (start, end) = range(span);
        self.names
            .push(self.text.get(start..end).unwrap_or("").to_owned());
    }
}

impl<'a> Visit<'a> for Identifiers<'_> {
    fn visit_identifier_reference(&mut self, identifier: &oxc_ast::ast::IdentifierReference<'a>) {
        self.push(identifier.span);
    }

    fn visit_identifier_name(&mut self, identifier: &oxc_ast::ast::IdentifierName<'a>) {
        self.push(identifier.span);
    }

    fn visit_binding_identifier(&mut self, identifier: &oxc_ast::ast::BindingIdentifier<'a>) {
        self.push(identifier.span);
    }

    fn visit_label_identifier(&mut self, identifier: &oxc_ast::ast::LabelIdentifier<'a>) {
        self.push(identifier.span);
    }
}

fn parameters_of(
    params: &FormalParameters,
    text: &str,
) -> (Vec<(usize, Option<usize>)>, Vec<String>) {
    let mut parameters = Vec::new();
    let mut identifiers = Identifiers {
        text,
        names: Vec::new(),
    };
    for param in &params.items {
        names_of(&param.pattern, text, &mut identifiers.names);
        if let Some(annotation) = &param.type_annotation {
            identifiers.visit_ts_type_annotation(annotation);
        }
        if let Some(initializer) = &param.initializer {
            identifiers.visit_expression(initializer);
        }
        parameters.push((
            at(param.pattern.span().end),
            param
                .type_annotation
                .as_ref()
                .map(|annotation| at(annotation.span.end)),
        ));
    }
    if let Some(rest) = &params.rest {
        names_of(&rest.rest.argument, text, &mut identifiers.names);
        if let Some(annotation) = &rest.type_annotation {
            identifiers.visit_ts_type_annotation(annotation);
        }
        parameters.push((
            at(rest.rest.argument.span().end),
            rest.type_annotation
                .as_ref()
                .map(|annotation| at(annotation.span.end)),
        ));
    }
    (parameters, identifiers.names)
}

pub fn slot_parameters(text: &str) -> Option<SlotParameters> {
    with_program(text, |program| {
        if let [Statement::ExpressionStatement(statement)] = program.body.as_slice()
            && program.directives.is_empty()
            && let Expression::ArrowFunctionExpression(arrow) = &statement.expression
        {
            // The original also counts the identifiers of a return type or a
            // type parameter, which a slot's parameters don't have.
            if arrow.type_parameters.is_some()
                || arrow.return_type.is_some()
                || !arrow.body.statements.is_empty()
            {
                return None;
            }
            let (parameters, declared) = parameters_of(&arrow.params, text);
            return Some(SlotParameters {
                is_arrow: true,
                parameters,
                declared,
            });
        }
        None
    })
    .flatten()
}

/// The class names a `:class` expression has, as the original finds them.
#[derive(Default)]
pub struct ClassNames {
    /// The names written as an object's keys: each name and where it starts.
    /// They are generated first.
    pub keys: Vec<(String, usize)>,
    /// The string literals: each one's text and where it starts.
    pub literals: Vec<(String, usize)>,
    /// Set when a literal has an escape in it: the original then counts from
    /// the literal's value, which isn't its text.
    pub has_escape: bool,
}

struct ClassWalker<'t> {
    text: &'t str,
    found: ClassNames,
}

impl ClassWalker<'_> {
    fn literal(&mut self, value: &str, span: Span) {
        // `literal.end - literal.text.length - 1`: after the opening quote.
        let (start, end) = range(span);
        let inner = self
            .text
            .get(start + 1..end.saturating_sub(1))
            .unwrap_or("");
        if inner != value {
            self.found.has_escape = true;
        }
        self.found.literals.push((value.to_owned(), start + 1));
    }

    /// `ts.isStringLiteralLike`: a string, or a template with nothing in it.
    fn string_like(&mut self, expression: &Expression) -> bool {
        match expression {
            Expression::StringLiteral(literal) => {
                self.literal(literal.value.as_str(), literal.span);
                true
            }
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => {
                match template
                    .quasis
                    .first()
                    .and_then(|quasi| quasi.value.cooked.as_ref())
                {
                    Some(cooked) => self.literal(cooked.as_str(), template.span),
                    None => self.found.has_escape = true,
                }
                true
            }
            _ => false,
        }
    }

    fn array(&mut self, elements: &[ArrayExpressionElement]) {
        for element in elements {
            let Some(expression) = element.as_expression() else {
                continue;
            };
            if !self.string_like(expression)
                && let Expression::ObjectExpression(object) = expression
            {
                self.object(object);
            }
        }
    }

    fn object(&mut self, object: &ObjectExpression) {
        for property in &object.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            if property.kind != PropertyKind::Init || property.method {
                continue;
            }
            match &property.key {
                // `{ name }` and `{ name: value }`
                PropertyKey::StaticIdentifier(name) => {
                    let (start, end) = range(name.span);
                    self.found
                        .keys
                        .push((self.text.get(start..end).unwrap_or("").to_owned(), start));
                }
                PropertyKey::StringLiteral(literal)
                    if !property.computed && !property.shorthand =>
                {
                    self.literal(literal.value.as_str(), literal.span);
                }
                key if property.computed => {
                    if let Some(expression) = key.as_expression() {
                        self.string_like(expression);
                    }
                }
                _ => {}
            }
        }
    }
}

/// `text` is the expression in parentheses.
pub fn class_names(text: &str) -> Option<ClassNames> {
    with_program(text, |program| {
        let mut walker = ClassWalker {
            text,
            found: ClassNames::default(),
        };
        for statement in &program.body {
            let Statement::ExpressionStatement(statement) = statement else {
                continue;
            };
            let Expression::ParenthesizedExpression(parenthesized) = &statement.expression else {
                continue;
            };
            match &parenthesized.expression {
                Expression::ArrayExpression(array) => walker.array(&array.elements),
                Expression::ObjectExpression(object) => walker.object(object),
                other => {
                    walker.string_like(other);
                }
            }
        }
        walker.found
    })
}
