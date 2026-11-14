//! Which identifiers of a template expression refer to the component: a port
//! of `codegen/template/bindingReferences.ts`.
//!
//! The original walks TypeScript's syntax tree, with one rule per kind of
//! node and, for every other kind, a walk over the node's children. This
//! walks oxc's tree. Each rule below names the TypeScript node it stands for,
//! and where TypeScript's shape differs from oxc's (one `BinaryExpression`
//! for every operator, an `Identifier` for every name) the rule follows
//! TypeScript's.
//!
//! A kind of node with no rule here marks the expression unsupported, so the
//! file goes to the JavaScript codegen: the original's fallback visits
//! whatever children TypeScript gives that node, which is only reproduced for
//! the kinds listed.

use oxc_ast::ast::{
    Argument, ArrayExpressionElement, AssignmentTarget, AssignmentTargetMaybeDefault,
    AssignmentTargetProperty, BinaryOperator, BindingPattern, ChainElement, Class, ClassElement,
    Expression, ForStatementInit, ForStatementLeft, FormalParameters, Function, FunctionBody,
    IdentifierReference, MethodDefinition, ObjectPropertyKind, Program, PropertyKey, PropertyKind,
    SimpleAssignmentTarget, Statement, TSMethodSignature, TSPropertySignature, TSThisParameter,
    TSType, TSTypeAnnotation, TSTypeName, TSTypeParameterDeclaration, TSTypeParameterInstantiation,
    TSTypeQuery, TSTypeQueryExprName, UnaryOperator, VariableDeclaration, VariableDeclarator,
};
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

use super::context::TemplateContext;

/// What kind of place an identifier is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Plain,
    /// `{ name }`: the identifier is a property's name and its value.
    Shorthand,
    /// The identifier is what `new` is called on.
    NewOperand,
}

/// An identifier the walk found (`DeclarationItem`, without the ones the
/// original marks skipped).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference {
    /// Where the identifier starts and ends in the parsed text.
    pub start: usize,
    pub end: usize,
    pub kind: ReferenceKind,
    /// The identifier is somewhere TypeScript narrows its type.
    pub is_narrowing: bool,
    /// The identifier is in `typeof name`, or a computed name in a type.
    pub in_type_query: bool,
}

fn at(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// `forEachDeclarations` over a whole parsed text. The scope the original's
/// caller opens around it is opened and closed here.
pub fn references(
    program: &Program,
    text: &str,
    context: &mut TemplateContext,
    in_narrowing: bool,
) -> Vec<Reference> {
    let mut walker = Walker {
        text,
        context,
        references: Vec::new(),
        in_type_query: false,
    };
    walker.context.open_scope();
    // `ts.isSourceFile`
    walker.predeclare_functions(&program.body);
    for statement in &program.body {
        walker.statement(statement, in_narrowing);
    }
    walker.context.close_scope();
    walker.references
}

struct Walker<'t, 'c> {
    text: &'t str,
    context: &'c mut TemplateContext,
    references: Vec<Reference>,
    /// Inside a computed name of a type, where everything found is marked.
    in_type_query: bool,
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
    fn source(&self, span: Span) -> &str {
        self.text.get(at(span.start)..at(span.end)).unwrap_or("")
    }

    fn unsupported(&mut self, reason: &'static str) {
        self.context.give_up(reason);
    }

    /// Yields an identifier, unless it isn't the component's.
    fn identifier(
        &mut self,
        span: Span,
        kind: ReferenceKind,
        is_narrowing: bool,
        in_type_query: bool,
    ) {
        if self.context.should_identifier_skipped(self.source(span)) {
            return;
        }
        self.references.push(Reference {
            start: at(span.start),
            end: at(span.end),
            kind,
            is_narrowing,
            in_type_query: in_type_query || self.in_type_query,
        });
    }

    fn declare(&mut self, span: Span) {
        let name = self.source(span).to_owned();
        self.context.declare(&name);
    }

    fn declare_pattern(&mut self, pattern: &BindingPattern) {
        let mut names = Vec::new();
        binding_names(pattern, &mut names);
        for name in names {
            self.declare(name);
        }
    }

    /// `predeclareFunctionDeclarations`: a function is in scope before its
    /// declaration.
    fn predeclare_functions(&mut self, statements: &[Statement]) {
        for statement in statements {
            if let Statement::FunctionDeclaration(function) = statement
                && let Some(id) = &function.id
            {
                self.declare(id.span);
            }
        }
    }

    /// A block's statements, in a scope of their own (`ts.isBlock`).
    fn block(&mut self, statements: &[Statement<'a>]) {
        self.context.open_scope();
        self.predeclare_functions(statements);
        for statement in statements {
            self.statement(statement, false);
        }
        self.context.close_scope();
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of statement, as the original has one branch per kind"
    )]
    fn statement(&mut self, statement: &Statement<'a>, in_narrowing: bool) {
        match statement {
            Statement::ExpressionStatement(statement) => {
                self.expression(&statement.expression, in_narrowing);
            }
            Statement::BlockStatement(block) => self.block(&block.body),
            Statement::VariableDeclaration(declaration) => self.variable_declaration(declaration),
            Statement::FunctionDeclaration(function) => {
                if let Some(id) = &function.id {
                    self.declare(id.span);
                }
                self.function(function);
            }
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    self.declare(id.span);
                }
                self.class(class);
            }
            Statement::IfStatement(statement) => {
                self.expression(&statement.test, true);
                self.statement(&statement.consequent, false);
                if let Some(alternate) = &statement.alternate {
                    self.statement(alternate, false);
                }
            }
            Statement::WhileStatement(statement) => {
                self.expression(&statement.test, true);
                self.statement(&statement.body, false);
            }
            Statement::DoWhileStatement(statement) => {
                self.statement(&statement.body, false);
                self.expression(&statement.test, true);
            }
            Statement::ForStatement(statement) => {
                self.context.open_scope();
                match &statement.init {
                    Some(ForStatementInit::VariableDeclaration(declaration)) => {
                        for declarator in &declaration.declarations {
                            self.declare_pattern(&declarator.id);
                            if let Some(init) = &declarator.init {
                                self.expression(init, false);
                            }
                        }
                    }
                    Some(init) => {
                        if let Some(init) = init.as_expression() {
                            self.expression(init, false);
                        }
                    }
                    None => {}
                }
                if let Some(test) = &statement.test {
                    self.expression(test, true);
                }
                if let Some(update) = &statement.update {
                    self.expression(update, false);
                }
                self.statement(&statement.body, false);
                self.context.close_scope();
            }
            Statement::ForInStatement(statement) => {
                self.for_in_or_of(&statement.left, &statement.right, &statement.body);
            }
            Statement::ForOfStatement(statement) => {
                self.for_in_or_of(&statement.left, &statement.right, &statement.body);
            }
            Statement::SwitchStatement(statement) => {
                // Its functions are declared in the scope it is in.
                for case in &statement.cases {
                    self.predeclare_functions(&case.consequent);
                }
                self.expression(&statement.discriminant, true);
                for case in &statement.cases {
                    if let Some(test) = &case.test {
                        self.expression(test, false);
                    }
                    for consequent in &case.consequent {
                        self.statement(consequent, false);
                    }
                }
            }
            Statement::LabeledStatement(statement) => self.statement(&statement.body, false),
            // A label isn't a binding, and these have nothing in them.
            Statement::BreakStatement(_)
            | Statement::ContinueStatement(_)
            | Statement::EmptyStatement(_)
            | Statement::DebuggerStatement(_) => {}
            // No rule in the original: their children, in order.
            Statement::ReturnStatement(statement) => {
                if let Some(argument) = &statement.argument {
                    self.expression(argument, false);
                }
            }
            Statement::ThrowStatement(statement) => self.expression(&statement.argument, false),
            Statement::TryStatement(statement) => {
                self.block(&statement.block.body);
                if let Some(handler) = &statement.handler {
                    // `ts.isCatchClause`
                    self.context.open_scope();
                    if let Some(param) = &handler.param {
                        self.declare_pattern(&param.pattern);
                        self.binding_pattern(&param.pattern);
                        if let Some(annotation) = &param.type_annotation {
                            self.type_annotation(annotation);
                        }
                    }
                    self.block(&handler.body.body);
                    self.context.close_scope();
                }
                if let Some(finalizer) = &statement.finalizer {
                    self.block(&finalizer.body);
                }
            }
            Statement::TSInterfaceDeclaration(declaration) => {
                self.visit_ts_interface_declaration(declaration);
            }
            Statement::TSTypeAliasDeclaration(declaration) => {
                self.visit_ts_type_alias_declaration(declaration);
            }
            Statement::TSEnumDeclaration(declaration) => {
                self.declare(declaration.id.span);
                for member in &declaration.body.members {
                    if let Some(initializer) = &member.initializer {
                        self.expression(initializer, false);
                    }
                }
            }
            _ => {
                self.unsupported("a template expression with a statement the port has no rule for");
            }
        }
    }

    /// `ts.isForOfStatement` and `ts.isForInStatement`.
    fn for_in_or_of(
        &mut self,
        left: &ForStatementLeft<'a>,
        right: &Expression<'a>,
        body: &Statement<'a>,
    ) {
        self.context.open_scope();
        let declaration = match left {
            ForStatementLeft::VariableDeclaration(declaration) => Some(declaration),
            _ => None,
        };
        match declaration {
            Some(declaration) => {
                for declarator in &declaration.declarations {
                    if let Some(init) = &declarator.init {
                        self.expression(init, false);
                    }
                }
            }
            None => {
                if let Some(target) = left.as_assignment_target() {
                    self.assignment_target(target);
                }
            }
        }
        // The source is read before the loop's names are in scope.
        self.expression(right, false);
        if let Some(declaration) = declaration {
            for declarator in &declaration.declarations {
                self.declare_pattern(&declarator.id);
            }
        }
        self.statement(body, false);
        self.context.close_scope();
    }

    /// A `VariableStatement`: each declaration's names, then what it reads.
    fn variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        for declarator in &declaration.declarations {
            self.declare_pattern(&declarator.id);
            self.declarator(declarator);
        }
    }

    /// `forEachDeclarationsInBinding` for a `VariableDeclaration`.
    fn declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        self.binding_pattern(&declarator.id);
        if let Some(annotation) = &declarator.type_annotation {
            self.type_annotation(annotation);
        }
        if let Some(init) = &declarator.init {
            self.expression(init, false);
        }
    }

    /// What a binding pattern reads: its computed names and its defaults.
    /// (`forEachDeclarations` on a binding pattern, and
    /// `forEachDeclarationsInBinding` on each of its elements.)
    fn binding_pattern(&mut self, pattern: &BindingPattern<'a>) {
        // `name = value`: the name's own pattern, then the default.
        let element = |walker: &mut Self, pattern: &BindingPattern<'a>| match pattern {
            BindingPattern::AssignmentPattern(assignment) => {
                walker.binding_pattern(&assignment.left);
                walker.expression(&assignment.right, false);
            }
            other => walker.binding_pattern(other),
        };
        match pattern {
            BindingPattern::BindingIdentifier(_) => {}
            BindingPattern::ObjectPattern(object) => {
                for property in &object.properties {
                    if property.computed
                        && let Some(key) = property.key.as_expression()
                    {
                        self.expression(key, true);
                    }
                    element(self, &property.value);
                }
                if let Some(rest) = &object.rest {
                    self.binding_pattern(&rest.argument);
                }
            }
            BindingPattern::ArrayPattern(array) => {
                for item in array.elements.iter().flatten() {
                    element(self, item);
                }
                if let Some(rest) = &array.rest {
                    self.binding_pattern(&rest.argument);
                }
            }
            BindingPattern::AssignmentPattern(assignment) => element(self, &assignment.left),
        }
    }

    /// `forEachDeclarationsInFunction`.
    fn function(&mut self, function: &Function<'a>) {
        self.context.open_scope();
        if function.is_expression()
            && let Some(id) = &function.id
        {
            self.declare(id.span);
        }
        self.function_parts(
            function.type_parameters.as_deref(),
            function.this_param.as_deref(),
            &function.params,
            function.return_type.as_deref(),
        );
        if let Some(body) = &function.body {
            self.function_body(body);
        }
        self.context.close_scope();
    }

    fn function_parts(
        &mut self,
        type_parameters: Option<&TSTypeParameterDeclaration<'a>>,
        this_param: Option<&TSThisParameter<'a>>,
        params: &FormalParameters<'a>,
        return_type: Option<&TSTypeAnnotation<'a>>,
    ) {
        // TypeScript has `this` as a parameter like the others.
        if this_param.is_some() {
            self.context.declare("this");
        }
        for param in &params.items {
            self.declare_pattern(&param.pattern);
        }
        if let Some(rest) = &params.rest {
            self.declare_pattern(&rest.rest.argument);
        }
        if let Some(type_parameters) = type_parameters {
            self.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(annotation) = this_param.and_then(|param| param.type_annotation.as_deref()) {
            self.type_annotation(annotation);
        }
        for param in &params.items {
            self.binding_pattern(&param.pattern);
            if let Some(annotation) = &param.type_annotation {
                self.type_annotation(annotation);
            }
            if let Some(initializer) = &param.initializer {
                self.expression(initializer, false);
            }
        }
        if let Some(rest) = &params.rest {
            self.binding_pattern(&rest.rest.argument);
            if let Some(annotation) = &rest.type_annotation {
                self.type_annotation(annotation);
            }
        }
        if let Some(annotation) = return_type {
            self.type_annotation(annotation);
        }
    }

    /// A function's block body: a `Block`, whose directives are statements
    /// with nothing in them.
    fn function_body(&mut self, body: &FunctionBody<'a>) {
        self.block(&body.statements);
    }

    fn method(&mut self, method: &MethodDefinition<'a>) {
        self.function(&method.value);
    }

    /// `forEachDeclarationsInClass`.
    fn class(&mut self, class: &Class<'a>) {
        if let Some(type_parameters) = &class.type_parameters {
            self.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(super_class) = &class.super_class {
            self.expression(super_class, false);
            if let Some(arguments) = &class.super_type_arguments {
                self.type_arguments(arguments);
            }
        }
        for implemented in &class.implements {
            self.visit_ts_class_implements(implemented);
        }
        for member in &class.body.body {
            // A computed name must be an entity name: no call around it.
            let computed_key = match member {
                ClassElement::MethodDefinition(method) if method.computed => Some(&method.key),
                ClassElement::PropertyDefinition(property) if property.computed => {
                    Some(&property.key)
                }
                ClassElement::AccessorProperty(property) if property.computed => {
                    Some(&property.key)
                }
                _ => None,
            };
            if let Some(key) = computed_key.and_then(PropertyKey::as_expression) {
                self.expression(key, true);
            }
            match member {
                ClassElement::MethodDefinition(method) => self.method(method),
                ClassElement::PropertyDefinition(property) => {
                    if let Some(annotation) = &property.type_annotation {
                        self.type_annotation(annotation);
                    }
                    if let Some(value) = &property.value {
                        self.field_value(value);
                    }
                }
                ClassElement::AccessorProperty(property) => {
                    if let Some(annotation) = &property.type_annotation {
                        self.type_annotation(annotation);
                    }
                    if let Some(value) = &property.value {
                        self.field_value(value);
                    }
                }
                ClassElement::StaticBlock(block) => self.block(&block.body),
                ClassElement::TSIndexSignature(_) => {}
            }
        }
    }

    /// A class field's value. oxc takes `a = b, c` there; TypeScript ends
    /// the field at the comma and reports an error.
    fn field_value(&mut self, value: &Expression<'a>) {
        if matches!(value, Expression::SequenceExpression(_)) {
            self.unsupported("a class field in a template expression with a comma in its value");
        }
        self.expression(value, false);
    }

    fn arguments(&mut self, arguments: &[Argument<'a>], in_narrowing: bool) {
        for argument in arguments {
            match argument {
                // A `SpreadElement` has no rule: its child, not narrowing.
                Argument::SpreadElement(spread) => self.expression(&spread.argument, false),
                other => {
                    if let Some(expression) = other.as_expression() {
                        self.expression(expression, in_narrowing);
                    }
                }
            }
        }
    }

    fn reference(&mut self, identifier: &IdentifierReference, in_narrowing: bool) {
        self.identifier(identifier.span, ReferenceKind::Plain, in_narrowing, false);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm per kind of expression, as the original has one branch per kind"
    )]
    fn expression(&mut self, expression: &Expression<'a>, in_narrowing: bool) {
        match expression {
            Expression::Identifier(identifier) => self.reference(identifier, in_narrowing),
            // `ts.isPropertyAccessExpression`
            Expression::StaticMemberExpression(member) => self.expression(&member.object, true),
            Expression::PrivateFieldExpression(member) => self.expression(&member.object, true),
            // `ts.isElementAccessExpression`
            Expression::ComputedMemberExpression(member) => {
                self.expression(&member.object, true);
                self.expression(&member.expression, false);
            }
            // An optional chain has no node of its own in TypeScript.
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    self.expression(&call.callee, false);
                    if let Some(arguments) = &call.type_arguments {
                        self.type_arguments(arguments);
                    }
                    self.arguments(&call.arguments, in_narrowing);
                }
                ChainElement::TSNonNullExpression(inner) => {
                    self.expression(&inner.expression, in_narrowing);
                }
                ChainElement::StaticMemberExpression(member) => {
                    self.expression(&member.object, true);
                }
                ChainElement::PrivateFieldExpression(member) => {
                    self.expression(&member.object, true);
                }
                ChainElement::ComputedMemberExpression(member) => {
                    self.expression(&member.object, true);
                    self.expression(&member.expression, false);
                }
            },
            Expression::CallExpression(call) => {
                self.expression(&call.callee, false);
                if let Some(arguments) = &call.type_arguments {
                    self.type_arguments(arguments);
                }
                self.arguments(&call.arguments, in_narrowing);
            }
            // `import(source)` is a call of the `import` keyword.
            Expression::ImportExpression(import) => {
                self.expression(&import.source, in_narrowing);
                if let Some(options) = &import.options {
                    self.expression(options, in_narrowing);
                }
            }
            Expression::NewExpression(new) => {
                match &new.callee {
                    Expression::Identifier(identifier) => {
                        self.identifier(identifier.span, ReferenceKind::NewOperand, false, false);
                    }
                    callee => self.expression(callee, false),
                }
                if let Some(arguments) = &new.type_arguments {
                    self.type_arguments(arguments);
                }
                self.arguments(&new.arguments, in_narrowing);
            }
            Expression::TaggedTemplateExpression(tagged) => {
                self.expression(&tagged.tag, false);
                for inner in &tagged.quasi.expressions {
                    self.expression(inner, false);
                }
            }
            // No rule for a template: its spans' expressions.
            Expression::TemplateLiteral(template) => {
                for inner in &template.expressions {
                    self.expression(inner, false);
                }
            }
            Expression::ParenthesizedExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
            }
            Expression::TSNonNullExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
            }
            Expression::TSTypeAssertion(assertion) => {
                self.type_node(&assertion.type_annotation);
                self.expression(&assertion.expression, in_narrowing);
            }
            Expression::TSAsExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
                self.type_node(&inner.type_annotation);
            }
            Expression::TSSatisfiesExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
                self.type_node(&inner.type_annotation);
            }
            // `name<T>` is a type node for TypeScript (`isTypeNode`), so it is
            // walked as one, its expression included.
            Expression::TSInstantiationExpression(inner) => {
                self.visit_expression(&inner.expression);
                self.type_arguments(&inner.type_arguments);
            }
            // TypeScript's `BinaryExpression`, by operator.
            Expression::SequenceExpression(sequence) => {
                // `a, b, c` is `(a, b), c`: only the last one narrows.
                let last = sequence.expressions.len().saturating_sub(1);
                for (index, inner) in sequence.expressions.iter().enumerate() {
                    self.expression(inner, index == last && in_narrowing);
                }
            }
            Expression::LogicalExpression(logical) => {
                self.expression(&logical.left, true);
                self.expression(&logical.right, in_narrowing);
            }
            Expression::AssignmentExpression(assignment) => {
                self.assignment_target(&assignment.left);
                self.expression(&assignment.right, false);
            }
            Expression::BinaryExpression(binary) => {
                let is_equality = matches!(
                    binary.operator,
                    BinaryOperator::StrictEquality
                        | BinaryOperator::StrictInequality
                        | BinaryOperator::Equality
                        | BinaryOperator::Inequality
                );
                self.expression(
                    &binary.left,
                    is_equality || binary.operator == BinaryOperator::Instanceof,
                );
                self.expression(
                    &binary.right,
                    is_equality || binary.operator == BinaryOperator::In,
                );
            }
            // `#name in object`. Outside a class TypeScript reports an error
            // and oxc doesn't always, so a template expression with one is
            // left to the original.
            Expression::PrivateInExpression(private) => {
                self.unsupported("a template expression with a private name in it");
                self.expression(&private.right, true);
            }
            Expression::ConditionalExpression(conditional) => {
                self.expression(&conditional.test, true);
                self.expression(&conditional.consequent, false);
                self.expression(&conditional.alternate, false);
            }
            Expression::UnaryExpression(unary) => {
                // `!`, `delete` and `typeof` narrow; `void` has no rule.
                let narrows = matches!(
                    unary.operator,
                    UnaryOperator::LogicalNot | UnaryOperator::Delete | UnaryOperator::Typeof
                );
                self.expression(&unary.argument, narrows);
            }
            // `++name` and `name++`
            Expression::UpdateExpression(update) => self.simple_target(&update.argument, true),
            // No rule: the operand, not narrowing.
            Expression::AwaitExpression(inner) => self.expression(&inner.argument, false),
            Expression::YieldExpression(inner) => {
                if let Some(argument) = &inner.argument {
                    self.expression(argument, false);
                }
            }
            Expression::ArrowFunctionExpression(function) => {
                self.context.open_scope();
                self.function_parts(
                    function.type_parameters.as_deref(),
                    None,
                    &function.params,
                    function.return_type.as_deref(),
                );
                if function.expression {
                    // An expression body is the body's one statement.
                    if let Some(Statement::ExpressionStatement(body)) =
                        function.body.statements.first()
                    {
                        self.expression(&body.expression, false);
                    }
                } else {
                    self.function_body(&function.body);
                }
                self.context.close_scope();
            }
            Expression::FunctionExpression(function) => self.function(function),
            Expression::ClassExpression(class) => {
                self.context.open_scope();
                if let Some(id) = &class.id {
                    self.declare(id.span);
                }
                self.class(class);
                self.context.close_scope();
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::SpreadProperty(spread) => {
                            self.expression(&spread.argument, false);
                        }
                        ObjectPropertyKind::ObjectProperty(property) => {
                            let is_function =
                                property.method || property.kind != PropertyKind::Init;
                            if property.shorthand && !is_function {
                                // `{ name }`
                                self.identifier(
                                    property.key.span(),
                                    ReferenceKind::Shorthand,
                                    false,
                                    false,
                                );
                                continue;
                            }
                            if property.computed
                                && let Some(key) = property.key.as_expression()
                            {
                                self.expression(key, false);
                            }
                            match &property.value {
                                Expression::FunctionExpression(function) if is_function => {
                                    self.function(function);
                                }
                                value => self.expression(value, false),
                            }
                        }
                    }
                }
            }
            // No rule: each element, not narrowing.
            Expression::ArrayExpression(array) => {
                for element in &array.elements {
                    match element {
                        ArrayExpressionElement::SpreadElement(spread) => {
                            self.expression(&spread.argument, false);
                        }
                        ArrayExpressionElement::Elision(_) => {}
                        other => {
                            if let Some(inner) = other.as_expression() {
                                self.expression(inner, false);
                            }
                        }
                    }
                }
            }
            // Nothing in them: literals, `this`, `super`, `new.target`.
            Expression::BooleanLiteral(_)
            | Expression::NullLiteral(_)
            | Expression::NumericLiteral(_)
            | Expression::BigIntLiteral(_)
            | Expression::RegExpLiteral(_)
            | Expression::StringLiteral(_)
            | Expression::ThisExpression(_)
            | Expression::Super(_)
            | Expression::MetaProperty(_) => {}
            Expression::JSXElement(_)
            | Expression::JSXFragment(_)
            | Expression::V8IntrinsicExpression(_) => {
                self.unsupported("a template expression with JSX in it");
            }
        }
    }

    /// The operand of `++` and `--`, which the original walks as an
    /// expression.
    fn simple_target(&mut self, target: &SimpleAssignmentTarget<'a>, in_narrowing: bool) {
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(identifier) => {
                self.reference(identifier, in_narrowing);
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.expression(&member.object, true);
            }
            SimpleAssignmentTarget::PrivateFieldExpression(member) => {
                self.expression(&member.object, true);
            }
            SimpleAssignmentTarget::ComputedMemberExpression(member) => {
                self.expression(&member.object, true);
                self.expression(&member.expression, false);
            }
            SimpleAssignmentTarget::TSNonNullExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
            }
            SimpleAssignmentTarget::TSAsExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
                self.type_node(&inner.type_annotation);
            }
            SimpleAssignmentTarget::TSSatisfiesExpression(inner) => {
                self.expression(&inner.expression, in_narrowing);
                self.type_node(&inner.type_annotation);
            }
            SimpleAssignmentTarget::TSTypeAssertion(assertion) => {
                self.type_node(&assertion.type_annotation);
                self.expression(&assertion.expression, in_narrowing);
            }
        }
    }

    /// `forEachDeclarationsInAssignmentTarget`: what is assigned to.
    fn assignment_target(&mut self, target: &AssignmentTarget<'a>) {
        match target {
            AssignmentTarget::ArrayAssignmentTarget(array) => {
                for element in array.elements.iter().flatten() {
                    self.target_maybe_default(element);
                }
                if let Some(rest) = &array.rest {
                    self.assignment_target(&rest.target);
                }
            }
            AssignmentTarget::ObjectAssignmentTarget(object) => {
                for property in &object.properties {
                    match property {
                        // `{ name }` and `{ name = value }`
                        AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(property) => {
                            self.identifier(
                                property.binding.span,
                                ReferenceKind::Shorthand,
                                true,
                                false,
                            );
                            if let Some(init) = &property.init {
                                self.expression(init, false);
                            }
                        }
                        AssignmentTargetProperty::AssignmentTargetPropertyProperty(property) => {
                            if property.computed
                                && let Some(key) = property.name.as_expression()
                            {
                                self.expression(key, false);
                            }
                            self.target_maybe_default(&property.binding);
                        }
                    }
                }
                if let Some(rest) = &object.rest {
                    self.assignment_target(&rest.target);
                }
            }
            other => {
                if let Some(simple) = other.as_simple_assignment_target() {
                    self.simple_target(simple, true);
                }
            }
        }
    }

    /// `target = value` inside a pattern: the target, then the default.
    fn target_maybe_default(&mut self, target: &AssignmentTargetMaybeDefault<'a>) {
        match target {
            AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(with_default) => {
                self.assignment_target(&with_default.binding);
                self.expression(&with_default.init, false);
            }
            other => {
                if let Some(target) = other.as_assignment_target() {
                    self.assignment_target(target);
                }
            }
        }
    }

    // ---- Types (`forEachDeclarationsInTypeNode`) ----
    //
    // In a type, only two things refer to a value: `typeof name`, and a
    // computed name. Everything else is walked through to find those.

    fn type_node(&mut self, node: &TSType<'a>) {
        self.visit_ts_type(node);
    }

    fn type_annotation(&mut self, annotation: &TSTypeAnnotation<'a>) {
        self.visit_ts_type(&annotation.type_annotation);
    }

    fn type_arguments(&mut self, arguments: &TSTypeParameterInstantiation<'a>) {
        for argument in &arguments.params {
            self.visit_ts_type(argument);
        }
    }

    /// A computed name in a type: an expression, all of it marked.
    fn computed_type_name(&mut self, key: &PropertyKey<'a>) {
        if let Some(expression) = key.as_expression() {
            let outer = std::mem::replace(&mut self.in_type_query, true);
            self.expression(expression, false);
            self.in_type_query = outer;
        }
    }
}

impl<'a> Visit<'a> for Walker<'_, '_> {
    /// `typeof a.b.c`: the name it starts with, and nothing of its type
    /// arguments.
    fn visit_ts_type_query(&mut self, query: &TSTypeQuery<'a>) {
        let mut name = match &query.expr_name {
            TSTypeQueryExprName::IdentifierReference(identifier) => {
                self.identifier(identifier.span, ReferenceKind::Plain, false, true);
                return;
            }
            TSTypeQueryExprName::QualifiedName(qualified) => &qualified.left,
            // `typeof this` and `typeof import()` name no binding.
            _ => return,
        };
        loop {
            match name {
                TSTypeName::IdentifierReference(identifier) => {
                    self.identifier(identifier.span, ReferenceKind::Plain, false, true);
                    return;
                }
                TSTypeName::QualifiedName(qualified) => name = &qualified.left,
                TSTypeName::ThisExpression(_) => return,
            }
        }
    }

    /// A computed name anywhere under a type: in a type literal, or in an
    /// expression a type contains. (A name that isn't computed has nothing in
    /// it: an identifier that is no reference, or a literal.)
    fn visit_property_key(&mut self, key: &PropertyKey<'a>) {
        self.computed_type_name(key);
    }

    fn visit_ts_property_signature(&mut self, signature: &TSPropertySignature<'a>) {
        if signature.computed {
            self.computed_type_name(&signature.key);
        }
        if let Some(annotation) = &signature.type_annotation {
            self.visit_ts_type_annotation(annotation);
        }
    }

    fn visit_ts_method_signature(&mut self, signature: &TSMethodSignature<'a>) {
        if signature.computed {
            self.computed_type_name(&signature.key);
        }
        if let Some(type_parameters) = &signature.type_parameters {
            self.visit_ts_type_parameter_declaration(type_parameters);
        }
        if let Some(this_param) = &signature.this_param {
            self.visit_ts_this_parameter(this_param);
        }
        self.visit_formal_parameters(&signature.params);
        if let Some(return_type) = &signature.return_type {
            self.visit_ts_type_annotation(return_type);
        }
    }
}
