//! Which identifiers of a template expression refer to the component, as
//! `@vue/language-core` 3.3.11 and before find them: `forEachDeclarations` in
//! their `codegen/template/interpolation.ts`. 3.3.12 replaced it
//! (`binding_references.rs`).
//!
//! It has a rule for a few kinds of node and walks the children of every
//! other kind, in TypeScript's order. This walks oxc's tree, with the
//! children of each kind it knows written out; a kind it doesn't know marks
//! the expression unsupported, so the file goes to the JavaScript codegen.

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentTarget, AssignmentTargetMaybeDefault,
    AssignmentTargetProperty, BindingPattern, ChainElement, Expression, ForStatementInit,
    ForStatementLeft, FormalParameters, FunctionBody, ObjectPropertyKind, Program, PropertyKey,
    PropertyKind, SimpleAssignmentTarget, Statement, TSType, TSTypeParameterInstantiation,
    TSTypeQuery, TSTypeQueryExprName, VariableDeclaration,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

use super::binding_references::{Reference, ReferenceKind};
use super::context::TemplateContext;

fn at(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// `forEachDeclarations` over a whole parsed text. The scope the original's
/// caller opens around it is opened and closed here.
pub fn references(program: &Program, text: &str, context: &mut TemplateContext) -> Vec<Reference> {
    let mut walker = Walker {
        text,
        context,
        references: Vec::new(),
    };
    walker.context.open_scope();
    for statement in &program.body {
        walker.statement(statement);
    }
    walker.context.close_scope();
    walker.references
}

struct Walker<'t, 'c> {
    text: &'t str,
    context: &'c mut TemplateContext,
    references: Vec<Reference>,
}

/// Every name a pattern binds (`collectBindingNames`).
fn binding_names(pattern: &BindingPattern, names: &mut Vec<Span>) {
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => names.push(identifier.span),
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                binding_names(&property.value, names);
            }
            if let Some(rest) = &object.rest {
                binding_names(&rest.argument, names);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for element in array.elements.iter().flatten() {
                binding_names(element, names);
            }
            if let Some(rest) = &array.rest {
                binding_names(&rest.argument, names);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => binding_names(&assignment.left, names),
    }
}

impl<'a> Walker<'_, '_> {
    fn unsupported(&mut self, reason: &'static str) {
        self.context.give_up(reason);
    }

    fn identifier(&mut self, span: Span, kind: ReferenceKind) {
        let name = self.text.get(at(span.start)..at(span.end)).unwrap_or("");
        if self.context.should_identifier_skipped(name) {
            return;
        }
        self.references.push(Reference {
            start: at(span.start),
            end: at(span.end),
            kind,
            is_narrowing: false,
            in_type_query: false,
        });
    }

    fn declare_pattern(&mut self, pattern: &BindingPattern) {
        let mut names = Vec::new();
        binding_names(pattern, &mut names);
        for span in names {
            let name = self
                .text
                .get(at(span.start)..at(span.end))
                .unwrap_or("")
                .to_owned();
            self.context.declare(&name);
        }
    }

    /// A statement has no rule of its own, apart from a block and a variable
    /// declaration: its children, in order.
    fn statement(&mut self, statement: &Statement<'a>) {
        match statement {
            Statement::ExpressionStatement(statement) => self.expression(&statement.expression),
            // `ts.isBlock`
            Statement::BlockStatement(block) => self.block(&block.body),
            Statement::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            Statement::IfStatement(statement) => {
                self.expression(&statement.test);
                self.statement(&statement.consequent);
                if let Some(alternate) = &statement.alternate {
                    self.statement(alternate);
                }
            }
            Statement::ReturnStatement(statement) => {
                if let Some(argument) = &statement.argument {
                    self.expression(argument);
                }
            }
            Statement::ThrowStatement(statement) => self.expression(&statement.argument),
            Statement::WhileStatement(statement) => {
                self.expression(&statement.test);
                self.statement(&statement.body);
            }
            Statement::DoWhileStatement(statement) => {
                self.statement(&statement.body);
                self.expression(&statement.test);
            }
            Statement::ForStatement(statement) => {
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        self.variable_declaration(declaration);
                    }
                    Some(init) => match init.as_expression() {
                        Some(expression) => self.expression(expression),
                        None => self.unsupported("a for statement's start the port doesn't walk"),
                    },
                    None => {}
                }
                if let Some(test) = &statement.test {
                    self.expression(test);
                }
                if let Some(update) = &statement.update {
                    self.expression(update);
                }
                self.statement(&statement.body);
            }
            Statement::ForInStatement(statement) => {
                self.for_left(&statement.left);
                self.expression(&statement.right);
                self.statement(&statement.body);
            }
            Statement::ForOfStatement(statement) => {
                self.for_left(&statement.left);
                self.expression(&statement.right);
                self.statement(&statement.body);
            }
            Statement::TryStatement(statement) => {
                self.block(&statement.block.body);
                if let Some(handler) = &statement.handler {
                    // The catch clause's variable is a variable declaration.
                    if let Some(parameter) = &handler.param {
                        self.declare_pattern(&parameter.pattern);
                        self.binding(&parameter.pattern);
                    }
                    self.block(&handler.body.body);
                }
                if let Some(finalizer) = &statement.finalizer {
                    self.block(&finalizer.body);
                }
            }
            Statement::EmptyStatement(_) | Statement::DebuggerStatement(_) => {}
            Statement::BreakStatement(statement) if statement.label.is_none() => {}
            Statement::ContinueStatement(statement) if statement.label.is_none() => {}
            _ => self.unsupported("a statement in a template expression the port doesn't walk"),
        }
    }

    fn for_left(&mut self, left: &ForStatementLeft<'a>) {
        match left {
            ForStatementLeft::VariableDeclaration(declaration) => {
                self.variable_declaration(declaration);
            }
            left => match left.as_assignment_target() {
                Some(target) => self.assignment_target(target),
                None => self.unsupported("a for statement's variable the port doesn't walk"),
            },
        }
    }

    /// `ts.isBlock`: a scope of its own.
    fn block(&mut self, statements: &[Statement<'a>]) {
        self.context.open_scope();
        for statement in statements {
            self.statement(statement);
        }
        self.context.close_scope();
    }

    /// `ts.isVariableDeclaration`, for each declaration of the list.
    fn variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        for declarator in &declaration.declarations {
            self.declare_pattern(&declarator.id);
            if let Some(annotation) = &declarator.type_annotation {
                self.type_node(&annotation.type_annotation);
            }
            self.binding(&declarator.id);
            if let Some(init) = &declarator.init {
                self.expression(init);
            }
        }
    }

    /// `forEachDeclarationsInBinding`, for a name that is a pattern: each
    /// element's own pattern, then its default.
    fn binding(&mut self, pattern: &BindingPattern<'a>) {
        match pattern {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    self.binding(&property.value);
                }
                if let Some(rest) = &object.rest {
                    self.binding(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for element in array.elements.iter().flatten() {
                    self.binding(element);
                }
                if let Some(rest) = &array.rest {
                    self.binding(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => {
                self.binding(&assignment.left);
                self.expression(&assignment.right);
            }
        }
    }

    /// `forEachDeclarationsInFunction`: the parameters are in scope in the
    /// body; the return type and the type parameters aren't looked at.
    fn function(&mut self, parameters: &FormalParameters<'a>, body: Option<&FunctionBody<'a>>) {
        self.context.open_scope();
        for parameter in &parameters.items {
            self.declare_pattern(&parameter.pattern);
            if let Some(annotation) = &parameter.type_annotation {
                self.type_node(&annotation.type_annotation);
            }
            self.binding(&parameter.pattern);
            if let Some(initializer) = &parameter.initializer {
                self.expression(initializer);
            }
        }
        if let Some(rest) = &parameters.rest {
            self.declare_pattern(&rest.rest.argument);
            if let Some(annotation) = &rest.type_annotation {
                self.type_node(&annotation.type_annotation);
            }
            self.binding(&rest.rest.argument);
        }
        if let Some(body) = body {
            self.block(&body.statements);
        }
        self.context.close_scope();
    }

    fn arguments(&mut self, arguments: &[Argument<'a>]) {
        for argument in arguments {
            match argument {
                Argument::SpreadElement(spread) => self.expression(&spread.argument),
                argument => {
                    if let Some(expression) = argument.as_expression() {
                        self.expression(expression);
                    }
                }
            }
        }
    }

    fn type_arguments(&mut self, arguments: &TSTypeParameterInstantiation<'a>) {
        for argument in &arguments.params {
            self.type_node(argument);
        }
    }

    /// `forEachDeclarationsInTypeNode`: what each `typeof` in the type names.
    fn type_node(&mut self, node: &TSType<'a>) {
        self.visit_ts_type(node);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of node, as the original's chain of tests"
    )]
    fn expression(&mut self, expression: &Expression<'a>) {
        match expression {
            Expression::Identifier(identifier) => {
                self.identifier(identifier.span, ReferenceKind::Plain);
            }
            // `ts.isPropertyAccessExpression`: not the property's name.
            Expression::StaticMemberExpression(member) => self.expression(&member.object),
            Expression::PrivateFieldExpression(member) => self.expression(&member.object),
            Expression::ComputedMemberExpression(member) => {
                self.expression(&member.object);
                self.expression(&member.expression);
            }
            // An optional chain has no node of its own in TypeScript.
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    self.expression(&call.callee);
                    if let Some(arguments) = &call.type_arguments {
                        self.type_arguments(arguments);
                    }
                    self.arguments(&call.arguments);
                }
                ChainElement::TSNonNullExpression(inner) => self.expression(&inner.expression),
                ChainElement::StaticMemberExpression(member) => self.expression(&member.object),
                ChainElement::PrivateFieldExpression(member) => self.expression(&member.object),
                ChainElement::ComputedMemberExpression(member) => {
                    self.expression(&member.object);
                    self.expression(&member.expression);
                }
            },
            Expression::CallExpression(call) => {
                self.expression(&call.callee);
                if let Some(arguments) = &call.type_arguments {
                    self.type_arguments(arguments);
                }
                self.arguments(&call.arguments);
            }
            Expression::ImportExpression(import) => {
                self.expression(&import.source);
                if let Some(options) = &import.options {
                    self.expression(options);
                }
            }
            Expression::NewExpression(new) => {
                self.expression(&new.callee);
                if let Some(arguments) = &new.type_arguments {
                    self.type_arguments(arguments);
                }
                self.arguments(&new.arguments);
            }
            Expression::TaggedTemplateExpression(tagged) => {
                self.expression(&tagged.tag);
                if let Some(arguments) = &tagged.type_arguments {
                    self.type_arguments(arguments);
                }
                for inner in &tagged.quasi.expressions {
                    self.expression(inner);
                }
            }
            Expression::TemplateLiteral(template) => {
                for inner in &template.expressions {
                    self.expression(inner);
                }
            }
            Expression::ParenthesizedExpression(inner) => self.expression(&inner.expression),
            Expression::TSNonNullExpression(inner) => self.expression(&inner.expression),
            Expression::TSAsExpression(inner) => {
                self.expression(&inner.expression);
                self.type_node(&inner.type_annotation);
            }
            Expression::TSSatisfiesExpression(inner) => {
                self.expression(&inner.expression);
                self.type_node(&inner.type_annotation);
            }
            Expression::TSTypeAssertion(inner) => {
                self.type_node(&inner.type_annotation);
                self.expression(&inner.expression);
            }
            Expression::TSInstantiationExpression(inner) => {
                self.expression(&inner.expression);
                self.type_arguments(&inner.type_arguments);
            }
            Expression::BinaryExpression(binary) => {
                self.expression(&binary.left);
                self.expression(&binary.right);
            }
            Expression::LogicalExpression(logical) => {
                self.expression(&logical.left);
                self.expression(&logical.right);
            }
            Expression::AssignmentExpression(assignment) => {
                self.assignment_target(&assignment.left);
                self.expression(&assignment.right);
            }
            Expression::SequenceExpression(sequence) => {
                for inner in &sequence.expressions {
                    self.expression(inner);
                }
            }
            Expression::ConditionalExpression(conditional) => {
                self.expression(&conditional.test);
                self.expression(&conditional.consequent);
                self.expression(&conditional.alternate);
            }
            Expression::UnaryExpression(unary) => self.expression(&unary.argument),
            Expression::UpdateExpression(update) => self.simple_target(&update.argument),
            Expression::AwaitExpression(inner) => self.expression(&inner.argument),
            Expression::YieldExpression(inner) => {
                if let Some(argument) = &inner.argument {
                    self.expression(argument);
                }
            }
            // `ts.isArrowFunction`, `ts.isFunctionExpression`
            Expression::ArrowFunctionExpression(arrow) => {
                self.context.open_scope();
                for parameter in &arrow.params.items {
                    self.declare_pattern(&parameter.pattern);
                    if let Some(annotation) = &parameter.type_annotation {
                        self.type_node(&annotation.type_annotation);
                    }
                    self.binding(&parameter.pattern);
                    if let Some(initializer) = &parameter.initializer {
                        self.expression(initializer);
                    }
                }
                if let Some(rest) = &arrow.params.rest {
                    self.declare_pattern(&rest.rest.argument);
                    if let Some(annotation) = &rest.type_annotation {
                        self.type_node(&annotation.type_annotation);
                    }
                    self.binding(&rest.rest.argument);
                }
                if arrow.expression {
                    // The body is the expression of its one statement.
                    for statement in &arrow.body.statements {
                        self.statement(statement);
                    }
                } else {
                    self.block(&arrow.body.statements);
                }
                self.context.close_scope();
            }
            Expression::FunctionExpression(function) => {
                self.function(&function.params, function.body.as_deref());
            }
            // `ts.isObjectLiteralExpression`
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::SpreadProperty(spread) => {
                            self.expression(&spread.argument);
                        }
                        ObjectPropertyKind::ObjectProperty(property) => {
                            if property.shorthand {
                                self.identifier(property.key.span(), ReferenceKind::Shorthand);
                            } else if property.method || property.kind != PropertyKind::Init {
                                // A method or an accessor: a function, whose
                                // name isn't looked at.
                                if let Expression::FunctionExpression(function) = &property.value {
                                    self.function(&function.params, function.body.as_deref());
                                }
                            } else {
                                if property.computed {
                                    self.property_key(&property.key);
                                }
                                self.expression(&property.value);
                            }
                        }
                    }
                }
            }
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            self.expression(&spread.argument);
                        }
                        ArrayExpressionElement::Elision(_) => {}
                        element => {
                            if let Some(expression) = element.as_expression() {
                                self.expression(expression);
                            }
                        }
                    }
                }
            }
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::ThisExpression(_)
            | Expression::Super(_) => {}
            _ => self.unsupported("an expression in a template the port doesn't walk"),
        }
    }

    fn property_key(&mut self, key: &PropertyKey<'a>) {
        match key.as_expression() {
            Some(expression) => self.expression(expression),
            None => self.unsupported("a computed name the port doesn't walk"),
        }
    }

    /// What is assigned to is an expression to TypeScript.
    fn simple_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                self.identifier(identifier.span, ReferenceKind::Plain);
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.expression(&member.object);
            }
            SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.expression(&member.object);
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.expression(&member.object);
                self.expression(&member.expression);
            }
            SimpleAssignmentTarget::TSNonNullExpression(inner) => {
                self.expression(&inner.expression);
            }
            SimpleAssignmentTarget::TSAsExpression(inner) => {
                self.expression(&inner.expression);
                self.type_node(&inner.type_annotation);
            }
            SimpleAssignmentTarget::TSSatisfiesExpression(inner) => {
                self.expression(&inner.expression);
                self.type_node(&inner.type_annotation);
            }
            SimpleAssignmentTarget::TSTypeAssertion(inner) => {
                self.type_node(&inner.type_annotation);
                self.expression(&inner.expression);
            }
        }
    }

    fn assignment_target(&mut self, target: &AssignmentTarget<'a>) {
        match target {
            // An array literal to TypeScript: its elements.
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    self.target_maybe_default(element);
                }
                if let Some(rest) = &array.rest {
                    self.assignment_target(&rest.target);
                }
            }
            // An object literal to TypeScript.
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        // `{ name }` and `{ name = default }`: the name alone.
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(property) => {
                            self.identifier(property.binding.span, ReferenceKind::Shorthand);
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(property) => {
                            if property.computed {
                                self.property_key(&property.name);
                            }
                            self.target_maybe_default(&property.binding);
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.assignment_target(&rest.target);
                }
            }
            target => match target.as_simple_assignment_target() {
                Some(simple) => self.simple_target(simple),
                None => self.unsupported("an assignment the port doesn't walk"),
            },
        }
    }

    fn target_maybe_default(&mut self, target: &AssignmentTargetMaybeDefault<'a>) {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                self.assignment_target(&with_default.binding);
                self.expression(&with_default.init);
            }
            target => match target.as_assignment_target() {
                Some(target) => self.assignment_target(target),
                None => self.unsupported("an assignment the port doesn't walk"),
            },
        }
    }
}

impl<'a> Visit<'a> for Walker<'_, '_> {
    /// `ts.isTypeQueryNode`: the first name of what `typeof` is of, and
    /// nothing inside it.
    fn visit_ts_type_query(&mut self, query: &TSTypeQuery<'a>) {
        match &query.expr_name {
            TSTypeQueryExprName::IdentifierReference(identifier) => {
                self.identifier(identifier.span, ReferenceKind::Plain);
            }
            TSTypeQueryExprName::QualifiedName(name) => {
                let mut left = &name.left;
                loop {
                    match left {
                        oxc_ast::ast::TSTypeName::IdentifierReference(identifier) => {
                            self.identifier(identifier.span, ReferenceKind::Plain);
                            break;
                        }
                        oxc_ast::ast::TSTypeName::QualifiedName(inner) => left = &inner.left,
                        oxc_ast::ast::TSTypeName::ThisExpression(_) => break,
                    }
                }
            }
            _ => self.unsupported("a typeof in a template the port doesn't walk"),
        }
    }
}
