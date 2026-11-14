//! What Vue's language tools read from a `<script>` block: a port of
//! `@vue/language-core`'s `parsers/scriptRanges.ts`,
//! `parsers/scriptSetupRanges.ts` and `parsers/utils.ts` (3.3.14).
//!
//! The original reads TypeScript's syntax tree. This reads oxc's, and gives
//! the same answers: which names a block declares, where its
//! `export default` and its macros (`defineProps`, `defineModel`, ...) are,
//! and where the pieces of each start and end. The two trees name and nest
//! things differently, so each rule here says which TypeScript node it stands
//! for.
//!
//! A block that oxc reports a syntax error in is not analyzed: TypeScript
//! recovers from errors in its own way, which only TypeScript reproduces.

pub mod trivia;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrowFunctionExpression, BindingPattern, CallExpression, ChainElement, Declaration,
    ExportDefaultDeclarationKind, Expression, Function, ImportDeclaration,
    ImportDeclarationSpecifier, ImportOrExportKind, MethodDefinition, ObjectExpression,
    ObjectProperty, ObjectPropertyKind, Program, PropertyKey, PropertyKind, Statement, TSSignature,
    TSType, VariableDeclaration, VariableDeclarationKind, VariableDeclarator,
};
use oxc_ast::{AstKind, Comment};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;

use crate::js;

/// An oxc offset as an index into the text.
fn at(offset: u32) -> usize {
    usize::try_from(offset).unwrap_or(usize::MAX)
}

/// A range of the block's text: `[start, end)`, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

impl From<Span> for Range {
    fn from(span: Span) -> Self {
        Self {
            start: at(span.start),
            end: at(span.end),
        }
    }
}

/// `BindingFlag`.
pub mod binding_flag {
    pub const NONE: u8 = 0;
    /// Reassigned in a closure by the template codegen: imports, `let`, `var`.
    pub const VARIABLE: u8 = 1;
    pub const CONST: u8 = 1 << 1;
    pub const COMPONENT: u8 = 1 << 2;
}

/// The names a block declares at its top level, in the order the original's
/// `Map` keeps them: by first declaration, with the last declaration's flags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bindings(pub Vec<(String, u8)>);

impl Bindings {
    fn set(&mut self, name: &str, flags: u8) {
        if let Some(existing) = self.0.iter_mut().find(|(known, _)| known == name) {
            existing.1 = flags;
        } else {
            self.0.push((name.to_owned(), flags));
        }
    }
}

/// The options language-core's analysis depends on (`vueCompilerOptions`).
#[derive(Clone, Debug)]
pub struct ScriptOptions {
    /// The extensions of a component file (`.vue`).
    pub extensions: Vec<String>,
    pub define_props: Vec<String>,
    pub define_slots: Vec<String>,
    pub define_emits: Vec<String>,
    pub define_expose: Vec<String>,
    pub define_model: Vec<String>,
    pub define_options: Vec<String>,
    pub with_defaults: Vec<String>,
    pub use_attrs: Vec<String>,
    pub use_css_module: Vec<String>,
    pub use_slots: Vec<String>,
    pub use_template_ref: Vec<String>,
}

impl Default for ScriptOptions {
    /// `getDefaultCompilerOptions`.
    fn default() -> Self {
        let one = |name: &str| vec![name.to_owned()];
        Self {
            extensions: one(".vue"),
            define_props: one("defineProps"),
            define_slots: one("defineSlots"),
            define_emits: one("defineEmits"),
            define_expose: one("defineExpose"),
            define_model: one("defineModel"),
            define_options: one("defineOptions"),
            with_defaults: one("withDefaults"),
            use_attrs: one("useAttrs"),
            use_css_module: one("useCssModule"),
            use_slots: one("useSlots"),
            use_template_ref: vec!["useTemplateRef".to_owned(), "templateRef".to_owned()],
        }
    }
}

/// The language of a `<script>` block that is analyzed at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptLang {
    Js,
    Jsx,
    Ts,
    Tsx,
}

impl ScriptLang {
    /// `validLangs`.
    pub fn from_lang(lang: &str) -> Option<Self> {
        match lang {
            "js" => Some(Self::Js),
            "jsx" => Some(Self::Jsx),
            "ts" => Some(Self::Ts),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }

    fn source_type(self) -> SourceType {
        match self {
            Self::Js => SourceType::mjs(),
            Self::Jsx => SourceType::jsx(),
            Self::Ts => SourceType::ts(),
            Self::Tsx => SourceType::tsx(),
        }
    }
}

/// The component options of an `export default`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentOptions {
    pub is_object_literal: bool,
    pub expression: Range,
    pub args: Range,
    pub components: Option<Range>,
    pub directives: Option<Range>,
    pub name: Option<Range>,
    /// The text of the `inheritAttrs` option's value.
    pub inherit_attrs: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportDefault {
    /// The statement, from a `/* */` comment before it when it has one.
    pub range: Range,
    pub expression: Range,
    pub is_object_literal: bool,
    pub options: Option<ComponentOptions>,
}

/// `parseScriptRanges`' result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptRanges {
    pub bindings: Bindings,
    pub export_default: Option<ExportDefault>,
}

/// `CallExpressionRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallRange {
    pub call_exp: Range,
    pub exp: Range,
    pub arg: Option<Range>,
    pub type_arg: Option<Range>,
}

/// A macro call, the name it is assigned to, and its statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacroCall {
    pub call: CallRange,
    pub name: Option<String>,
    pub statement: Range,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefineProps {
    pub call: MacroCall,
    /// `const { a, b = 1 } = defineProps()`: each name, and its default.
    pub destructured: Option<Vec<(String, Option<Range>)>>,
    pub destructured_rest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefineEmits {
    pub call: MacroCall,
    pub has_union_type_arg: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefineModel {
    pub arg: Option<Range>,
    pub local_name: Option<Range>,
    pub name: Option<Range>,
    pub r#type: Option<Range>,
    pub modifier_type: Option<Range>,
    pub runtime_type: Option<Range>,
    pub default_value: Option<Range>,
    pub default_props_arg: Option<Range>,
    pub required: bool,
    pub comments: Option<Range>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefineOptions {
    pub name: Option<String>,
    pub inherit_attrs: Option<String>,
}

/// `useTemplateRef`, and the name it is assigned to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseTemplateRef {
    pub call: CallRange,
    pub name: Option<String>,
}

/// `parseScriptSetupRanges`' result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptSetupRanges {
    pub bindings: Bindings,
    pub leading_comment_end_offset: usize,
    pub import_section_end_offset: usize,
    pub define_model: Vec<DefineModel>,
    pub define_props: Option<DefineProps>,
    pub with_defaults: Option<CallRange>,
    pub define_emits: Option<DefineEmits>,
    pub define_slots: Option<MacroCall>,
    pub define_expose: Option<CallRange>,
    pub define_options: Option<DefineOptions>,
    pub use_attrs: Vec<CallRange>,
    pub use_css_module: Vec<CallRange>,
    pub use_slots: Vec<CallRange>,
    pub use_template_ref: Vec<UseTemplateRef>,
}

/// Why a block wasn't analyzed.
pub type Unsupported = &'static str;

/// Parses a block and hands its tree to `read`.
fn with_program<T>(
    text: &str,
    lang: ScriptLang,
    read: impl FnOnce(&Program<'_>) -> Result<T, Unsupported>,
) -> Result<T, Unsupported> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, lang.source_type()).parse();
    if parsed.panicked || !parsed.diagnostics.is_empty() {
        // For finding out why a file was declined.
        if std::env::var_os("VUE_CHECK_NATIVE_EXPLAIN").is_some() {
            for diagnostic in &parsed.diagnostics {
                eprintln!("syntax error: {diagnostic}");
            }
        }
        return Err("a <script> block has a syntax error");
    }
    read(&parsed.program)
}

/// `parseScriptRanges`.
pub fn parse_script_ranges(
    text: &str,
    lang: ScriptLang,
    options: &ScriptOptions,
) -> Result<ScriptRanges, Unsupported> {
    with_program(text, lang, |program| {
        let mut export_default = None;
        for statement in &program.body {
            // TypeScript's `ExportAssignment`: `export default <expression>`
            // and `export = <expression>`.
            let (span, expression) = match statement {
                Statement::ExportDefaultDeclaration(export) => {
                    match export.declaration.as_expression() {
                        Some(expression) => (export.span, expression),
                        None => continue,
                    }
                }
                Statement::TSExportAssignment(export) => (export.span, &export.expression),
                _ => continue,
            };
            let mut range = Range::from(span);
            // From the last `/* */` comment before it.
            let position = trivia::full_start(text, &program.comments, range.start);
            if let Some(comment) = trivia::leading_comment_ranges(text, position)
                .iter()
                .rev()
                .find(|comment| comment.is_multi_line)
            {
                range.start = comment.start;
            }
            export_default = Some(ExportDefault {
                range,
                expression: expression.span().into(),
                is_object_literal: matches!(expression, Expression::ObjectExpression(_)),
                options: parse_options_from_expression(text, expression),
            });
        }
        Ok(ScriptRanges {
            bindings: parse_bindings(text, program, &options.extensions),
            export_default,
        })
    })
}

/// `getUnwrappedExpression`: inside parentheses, `as`, `<T>`, `!` and
/// `satisfies`.
fn unwrapped<'e, 'a>(mut expression: &'e Expression<'a>) -> &'e Expression<'a> {
    loop {
        expression = match expression {
            Expression::ParenthesizedExpression(inner) => &inner.expression,
            Expression::TSAsExpression(inner) => &inner.expression,
            Expression::TSTypeAssertion(inner) => &inner.expression,
            Expression::TSNonNullExpression(inner) => &inner.expression,
            Expression::TSSatisfiesExpression(inner) => &inner.expression,
            other => return other,
        };
    }
}

fn source(text: &str, span: Span) -> &str {
    text.get(at(span.start)..at(span.end)).unwrap_or("")
}

/// TypeScript's `PropertyAssignment` whose name is an identifier: `name:
/// value`, not a method, an accessor, a shorthand or a computed name.
fn property_assignment<'p, 'a>(
    property: &'p ObjectPropertyKind<'a>,
) -> Option<(&'p str, &'p ObjectProperty<'a>)> {
    let ObjectPropertyKind::ObjectProperty(property) = property else {
        return None;
    };
    if property.kind != PropertyKind::Init || property.method || property.shorthand {
        return None;
    }
    match &property.key {
        PropertyKey::StaticIdentifier(name) => Some((name.name.as_str(), property)),
        _ => None,
    }
}

/// `parseOptionsFromExtression`: the options object of a component, written
/// as it is or as the first argument of a call (`defineComponent({ ... })`).
fn parse_options_from_expression(text: &str, expression: &Expression) -> Option<ComponentOptions> {
    let expression = unwrapped(expression);
    // An optional call (`define?.({})`) is a call for TypeScript, with no
    // node around it.
    let call = match expression {
        Expression::CallExpression(call) => Some(&**call),
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => Some(&**call),
            _ => None,
        },
        _ => None,
    };
    let object: &ObjectExpression = match (expression, call) {
        (Expression::ObjectExpression(object), _) => object,
        (_, Some(call)) => match call.arguments.first() {
            Some(Argument::ObjectExpression(object)) => object,
            _ => return None,
        },
        _ => return None,
    };
    let mut options = ComponentOptions {
        is_object_literal: matches!(expression, Expression::ObjectExpression(_)),
        expression: expression.span().into(),
        args: object.span.into(),
        components: None,
        directives: None,
        name: None,
        inherit_attrs: None,
    };
    for (name, property) in object.properties.iter().filter_map(property_assignment) {
        match (name, &property.value) {
            ("components", Expression::ObjectExpression(value)) => {
                options.components = Some(value.span.into());
            }
            ("directives", Expression::ObjectExpression(value)) => {
                options.directives = Some(value.span.into());
            }
            ("name", Expression::StringLiteral(value)) => options.name = Some(value.span.into()),
            ("inheritAttrs", value) => {
                options.inherit_attrs = Some(source(text, value.span()).to_owned());
            }
            _ => {}
        }
    }
    Some(options)
}

/// A name a declaration binds, whether it is a rest element, and its default.
struct BoundName {
    span: Span,
    is_rest: bool,
    initializer: Option<Span>,
}

/// `collectBindingIdentifiers`: every name a pattern binds, in order.
fn collect_binding_identifiers(
    pattern: &BindingPattern,
    names: &mut Vec<BoundName>,
    is_rest: bool,
    initializer: Option<Span>,
) {
    // A default (`name = value`) belongs to the name it is on.
    let element = |pattern: &BindingPattern, names: &mut Vec<BoundName>| match pattern {
        BindingPattern::AssignmentPattern(assignment) => {
            collect_binding_identifiers(
                &assignment.left,
                names,
                false,
                Some(assignment.right.span()),
            );
        }
        other => collect_binding_identifiers(other, names, false, None),
    };
    match pattern {
        BindingPattern::BindingIdentifier(identifier) => names.push(BoundName {
            span: identifier.span,
            is_rest,
            initializer,
        }),
        BindingPattern::ObjectPattern(object) => {
            for property in &object.properties {
                element(&property.value, names);
            }
            if let Some(rest) = &object.rest {
                collect_binding_identifiers(&rest.argument, names, true, None);
            }
        }
        BindingPattern::ArrayPattern(array) => {
            for item in array.elements.iter().flatten() {
                element(item, names);
            }
            if let Some(rest) = &array.rest {
                collect_binding_identifiers(&rest.argument, names, true, None);
            }
        }
        BindingPattern::AssignmentPattern(assignment) => {
            collect_binding_identifiers(&assignment.left, names, is_rest, initializer);
        }
    }
}

/// A top-level statement as the TypeScript node it is. TypeScript has no node
/// for `export` in front of a declaration: the declaration is the statement,
/// with a modifier.
enum TopLevel<'p, 'a> {
    Variable(&'p VariableDeclaration<'a>),
    /// A function, class or enum declaration, and its name.
    Named(Option<Span>),
    Import(&'p ImportDeclaration<'a>),
    /// `export { a }`, `export * from`.
    Export,
    ImportEquals,
    Empty,
    /// A type alias or interface: whether it is exported.
    TypeDeclaration {
        exported: bool,
    },
    Other,
}

fn top_level<'p, 'a>(statement: &'p Statement<'a>) -> TopLevel<'p, 'a> {
    let declaration = |declaration: &'p Declaration<'a>, exported: bool| match declaration {
        Declaration::VariableDeclaration(variable) => TopLevel::Variable(variable),
        Declaration::FunctionDeclaration(function) => {
            TopLevel::Named(function.id.as_ref().map(|id| id.span))
        }
        Declaration::ClassDeclaration(class) => {
            TopLevel::Named(class.id.as_ref().map(|id| id.span))
        }
        Declaration::TSEnumDeclaration(declaration) => TopLevel::Named(Some(declaration.id.span)),
        Declaration::TSTypeAliasDeclaration(_) | Declaration::TSInterfaceDeclaration(_) => {
            TopLevel::TypeDeclaration { exported }
        }
        Declaration::TSImportEqualsDeclaration(_) => TopLevel::ImportEquals,
        _ => TopLevel::Other,
    };
    match statement {
        Statement::ImportDeclaration(import) => TopLevel::Import(import),
        Statement::ExportAllDeclaration(_) => TopLevel::Export,
        Statement::ExportNamedDeclaration(export) => match &export.declaration {
            Some(exported) => declaration(exported, true),
            None => TopLevel::Export,
        },
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                TopLevel::Named(function.id.as_ref().map(|id| id.span))
            }
            ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                TopLevel::Named(class.id.as_ref().map(|id| id.span))
            }
            ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => {
                TopLevel::TypeDeclaration { exported: true }
            }
            _ => TopLevel::Other,
        },
        Statement::EmptyStatement(_) => TopLevel::Empty,
        other => other
            .as_declaration()
            .map_or(TopLevel::Other, |found| declaration(found, false)),
    }
}

/// `parseBindings`: the names a block declares at its top level.
fn parse_bindings(text: &str, program: &Program, extensions: &[String]) -> Bindings {
    let mut bindings = Bindings::default();
    for statement in &program.body {
        match top_level(statement) {
            TopLevel::Variable(variable) => {
                // `await using` has TypeScript's `Const` flag too.
                let flags = if matches!(
                    variable.kind,
                    VariableDeclarationKind::Const | VariableDeclarationKind::AwaitUsing
                ) {
                    binding_flag::NONE
                } else {
                    binding_flag::VARIABLE
                };
                for declarator in &variable.declarations {
                    let mut names = Vec::new();
                    collect_binding_identifiers(&declarator.id, &mut names, false, None);
                    for name in names {
                        bindings.set(source(text, name.span), flags);
                    }
                }
            }
            TopLevel::Named(Some(name)) => bindings.set(source(text, name), binding_flag::CONST),
            TopLevel::Import(import) => import_bindings(text, import, extensions, &mut bindings),
            _ => {}
        }
    }
    bindings
}

fn import_bindings(
    text: &str,
    import: &ImportDeclaration,
    extensions: &[String],
    bindings: &mut Bindings,
) {
    // `import 'module'` has no clause, and `import type` binds no value.
    let Some(specifiers) = &import.specifiers else {
        return;
    };
    if import.import_kind == ImportOrExportKind::Type {
        return;
    }
    // The module's name as written, between its quotes.
    let module = source(text, import.source.span);
    let module = module.get(1..module.len().saturating_sub(1)).unwrap_or("");
    let is_component = extensions
        .iter()
        .any(|extension| module.ends_with(extension.as_str()));
    let component = binding_flag::CONST | binding_flag::COMPONENT;
    for specifier in specifiers {
        match specifier {
            ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => bindings.set(
                source(text, specifier.local.span),
                if is_component {
                    component
                } else {
                    binding_flag::VARIABLE
                },
            ),
            ImportDeclarationSpecifier::ImportNamespaceSpecifier(specifier) => {
                bindings.set(source(text, specifier.local.span), binding_flag::VARIABLE);
            }
            ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                if specifier.import_kind == ImportOrExportKind::Type {
                    continue;
                }
                // `{ default as Name }`: with no `as`, both names are one.
                let imported = specifier.imported.span();
                let is_default =
                    imported != specifier.local.span && source(text, imported) == "default";
                bindings.set(
                    source(text, specifier.local.span),
                    if is_default && is_component {
                        component
                    } else {
                        binding_flag::VARIABLE
                    },
                );
            }
        }
    }
}

/// `tsCheckRE`: `/^\/\/(?:\s*@ts-(?:no)?check(?:$|\s)|\/\s*<reference\s)/`.
fn is_ts_check_comment(comment: &str) -> bool {
    let Some(rest) = comment.strip_prefix("//") else {
        return false;
    };
    let starts_with_space = |text: &str| text.chars().next().is_some_and(js::is_whitespace);
    let directive = js::trim_start(rest);
    if let Some(after) = directive
        .strip_prefix("@ts-nocheck")
        .or_else(|| directive.strip_prefix("@ts-check"))
    {
        return after.is_empty() || starts_with_space(after);
    }
    rest.strip_prefix('/')
        .map(js::trim_start)
        .and_then(|reference| reference.strip_prefix("<reference"))
        .is_some_and(starts_with_space)
}

/// `parseScriptSetupRanges`.
pub fn parse_script_setup_ranges(
    text: &str,
    lang: ScriptLang,
    options: &ScriptOptions,
) -> Result<ScriptSetupRanges, Unsupported> {
    with_program(text, lang, |program| {
        let mut ranges = ScriptSetupRanges {
            bindings: parse_bindings(text, program, &options.extensions),
            ..ScriptSetupRanges::default()
        };
        // After the last `// @ts-check` or `/// <reference>` at the top.
        ranges.leading_comment_end_offset = trivia::leading_comment_ranges(text, 0)
            .iter()
            .rev()
            .find(|comment| is_ts_check_comment(text.get(comment.start..comment.end).unwrap_or("")))
            .map_or(0, |comment| comment.end);

        // Where the imports end: at the first statement that isn't an import,
        // an export or an exported type, or at the comments before it. With
        // no such statement it is TypeScript's end-of-file token, which is a
        // child of the file like any statement.
        let first_other = program.body.iter().find(|statement| {
            !matches!(
                top_level(statement),
                TopLevel::Import(_)
                    | TopLevel::Export
                    | TopLevel::Empty
                    | TopLevel::ImportEquals
                    | TopLevel::TypeDeclaration { exported: true }
            )
        });
        let start = first_other.map_or(text.len(), |statement| at(statement.span().start));
        let position = trivia::full_start(text, &program.comments, start);
        ranges.import_section_end_offset = trivia::leading_comment_ranges(text, position)
            .first()
            .map_or(start, |comment| comment.start);

        let mut visitor = MacroVisitor {
            text,
            comments: &program.comments,
            options,
            ancestors: Vec::new(),
            ranges,
            unsupported: None,
        };
        visitor.visit_program(program);
        match visitor.unsupported {
            Some(reason) => Err(reason),
            None => Ok(visitor.ranges),
        }
    })
}

/// Walks a `<script setup>` for its macro calls, as `visitNode` does: through
/// everything but functions.
struct MacroVisitor<'t, 'p, 'a> {
    text: &'t str,
    comments: &'p [Comment],
    options: &'t ScriptOptions,
    /// The nodes around the one being visited, outermost first.
    ancestors: Vec<AstKind<'a>>,
    ranges: ScriptSetupRanges,
    unsupported: Option<Unsupported>,
}

impl<'a> Visit<'a> for MacroVisitor<'_, '_, 'a> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        if let AstKind::CallExpression(call) = kind {
            self.call(call);
        }
        self.ancestors.push(kind);
    }

    fn leave_node(&mut self, _kind: AstKind<'a>) {
        self.ancestors.pop();
    }

    // TypeScript's function-like nodes aren't entered: functions, arrow
    // functions, and the methods and accessors of classes and objects (which
    // are function-like themselves there, computed name included).
    fn visit_function(&mut self, _function: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _function: &ArrowFunctionExpression<'a>) {}

    fn visit_method_definition(&mut self, _method: &MethodDefinition<'a>) {}

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if property.kind == PropertyKind::Init && !property.method {
            walk::walk_object_property(self, property);
        }
    }
}

/// Whether a node is a statement for TypeScript's `isStatement`, given the
/// node around it.
fn is_statement(kind: &AstKind, parent: Option<&AstKind>) -> bool {
    match kind {
        // In `for (const a of b)` it is a declaration list, not a statement.
        AstKind::VariableDeclaration(_) => !matches!(
            parent,
            Some(
                AstKind::ForStatement(_) | AstKind::ForInStatement(_) | AstKind::ForOfStatement(_)
            )
        ),
        // The block of a `try`, `catch` or `finally` isn't a statement.
        AstKind::BlockStatement(_) => !matches!(
            parent,
            Some(AstKind::TryStatement(_) | AstKind::CatchClause(_))
        ),
        AstKind::Class(class) => class.is_declaration(),
        AstKind::ExpressionStatement(_)
        | AstKind::IfStatement(_)
        | AstKind::DoWhileStatement(_)
        | AstKind::WhileStatement(_)
        | AstKind::ForStatement(_)
        | AstKind::ForInStatement(_)
        | AstKind::ForOfStatement(_)
        | AstKind::ContinueStatement(_)
        | AstKind::BreakStatement(_)
        | AstKind::ReturnStatement(_)
        | AstKind::WithStatement(_)
        | AstKind::SwitchStatement(_)
        | AstKind::LabeledStatement(_)
        | AstKind::ThrowStatement(_)
        | AstKind::TryStatement(_)
        | AstKind::DebuggerStatement(_)
        | AstKind::EmptyStatement(_)
        | AstKind::TSInterfaceDeclaration(_)
        | AstKind::TSTypeAliasDeclaration(_)
        | AstKind::TSEnumDeclaration(_)
        | AstKind::TSModuleDeclaration(_)
        | AstKind::ImportDeclaration(_)
        | AstKind::TSImportEqualsDeclaration(_)
        | AstKind::ExportAllDeclaration(_)
        | AstKind::ExportNamedDeclaration(_)
        | AstKind::ExportDefaultDeclaration(_)
        | AstKind::TSExportAssignment(_)
        | AstKind::TSNamespaceExportDeclaration(_) => true,
        _ => false,
    }
}

impl<'a> MacroVisitor<'_, '_, 'a> {
    fn range_text(&self, span: Span) -> &str {
        source(self.text, span)
    }

    /// The nodes around the call, innermost first, as TypeScript has them:
    /// it has no node for an optional chain.
    fn parents(&self) -> impl Iterator<Item = &AstKind<'a>> {
        self.ancestors
            .iter()
            .rev()
            .filter(|kind| !matches!(kind, AstKind::ChainExpression(_)))
    }

    /// The declaration the call is the whole initializer of
    /// (`const name = call()`).
    fn declarator_of(
        parent: Option<&AstKind<'a>>,
        span: Span,
    ) -> Option<&'a VariableDeclarator<'a>> {
        match parent {
            Some(AstKind::VariableDeclarator(declarator))
                if declarator
                    .init
                    .as_ref()
                    .is_some_and(|init| init.span() == span) =>
            {
                Some(declarator)
            }
            _ => None,
        }
    }

    /// The statement around the call, and where it starts for TypeScript:
    /// at `export`, when it is exported.
    fn statement(&self) -> Option<(AstKind<'a>, usize)> {
        let mut index = self.ancestors.len();
        while index > 0 {
            index -= 1;
            let kind = self.ancestors.get(index)?;
            let parent = index
                .checked_sub(1)
                .and_then(|parent| self.ancestors.get(parent));
            if !is_statement(kind, parent) {
                continue;
            }
            let start = match parent {
                Some(
                    export @ (AstKind::ExportNamedDeclaration(_)
                    | AstKind::ExportDefaultDeclaration(_)),
                ) => export.span().start,
                _ => kind.span().start,
            };
            return Some((*kind, at(start)));
        }
        None
    }

    /// `getStatementRange`: from the statement's first child to its last.
    fn statement_range(&mut self, call: &CallExpression) -> Range {
        match self.statement() {
            // Without its semicolon.
            Some((AstKind::VariableDeclaration(declaration), start)) => Range {
                start,
                end: declaration
                    .declarations
                    .last()
                    .map_or(at(declaration.span.end), |last| at(last.span.end)),
            },
            Some((AstKind::ExpressionStatement(statement), _)) => {
                statement.expression.span().into()
            }
            Some(_) => {
                // The original takes the range of the statement's children,
                // which is different for every kind of statement.
                self.unsupported.get_or_insert(
                    "a macro inside a statement that isn't a declaration or an expression",
                );
                call.span.into()
            }
            None => call.span.into(),
        }
    }

    /// `getClosestMultiLineCommentRange`: the last `/* */` comment before the
    /// statement the call is in.
    fn closest_multi_line_comment(&self) -> Option<Range> {
        let start = self.statement().map_or(0, |(_, start)| start);
        let position = trivia::full_start(self.text, self.comments, start);
        trivia::leading_comment_ranges(self.text, position)
            .iter()
            .rev()
            .find(|comment| comment.is_multi_line)
            .map(|comment| Range {
                start: comment.start,
                end: comment.end,
            })
    }

    fn call_range(call: &CallExpression) -> CallRange {
        CallRange {
            call_exp: call.span.into(),
            exp: call.callee.span().into(),
            arg: call
                .arguments
                .first()
                .map(|argument| argument.span().into()),
            type_arg: call
                .type_arguments
                .as_ref()
                .and_then(|arguments| arguments.params.first())
                .map(|argument| argument.span().into()),
        }
    }

    /// The name the call is assigned to: `const name = call()`.
    fn assigned_name(&self, call: &CallExpression) -> Option<String> {
        let declarator = Self::declarator_of(self.parents().next(), call.span)?;
        match &declarator.id {
            BindingPattern::BindingIdentifier(identifier) => {
                Some(self.range_text(identifier.span).to_owned())
            }
            _ => None,
        }
    }

    fn macro_call(&mut self, call: &CallExpression) -> MacroCall {
        MacroCall {
            call: Self::call_range(call),
            name: self.assigned_name(call),
            statement: self.statement_range(call),
        }
    }

    fn call(&mut self, call: &CallExpression<'a>) {
        let Expression::Identifier(callee) = &call.callee else {
            return;
        };
        let name = self.range_text(callee.span).to_owned();
        let options = self.options;
        let is = |names: &[String]| names.contains(&name);
        if is(&options.define_model) {
            let model = self.define_model(call);
            self.ranges.define_model.push(model);
        } else if is(&options.define_props) {
            let props = self.define_props(call);
            self.ranges.define_props = Some(props);
        } else if is(&options.with_defaults) {
            self.ranges.with_defaults = Some(CallRange {
                call_exp: call.span.into(),
                exp: call.callee.span().into(),
                arg: call.arguments.get(1).map(|argument| argument.span().into()),
                type_arg: None,
            });
        } else if is(&options.define_emits) {
            let has_union_type_arg = has_union_type_arg(call);
            self.ranges.define_emits = Some(DefineEmits {
                call: self.macro_call(call),
                has_union_type_arg,
            });
        } else if is(&options.define_slots) {
            self.ranges.define_slots = Some(self.macro_call(call));
        } else if is(&options.define_expose) {
            self.ranges.define_expose = Some(Self::call_range(call));
        } else if is(&options.define_options)
            && let Some(Argument::ObjectExpression(object)) = call.arguments.first()
        {
            let mut define_options = DefineOptions::default();
            for (name, property) in object.properties.iter().filter_map(property_assignment) {
                match (name, &property.value) {
                    ("inheritAttrs", value) => {
                        define_options.inherit_attrs =
                            Some(self.range_text(value.span()).to_owned());
                    }
                    ("name", Expression::StringLiteral(value)) => {
                        define_options.name = Some(value.value.as_str().to_owned());
                    }
                    _ => {}
                }
            }
            self.ranges.define_options = Some(define_options);
        } else if is(&options.use_attrs) {
            self.ranges.use_attrs.push(Self::call_range(call));
        } else if is(&options.use_css_module) {
            self.ranges.use_css_module.push(Self::call_range(call));
        } else if is(&options.use_slots) {
            self.ranges.use_slots.push(Self::call_range(call));
        } else if is(&options.use_template_ref)
            && call
                .type_arguments
                .as_ref()
                .is_none_or(|arguments| arguments.params.is_empty())
        {
            self.ranges.use_template_ref.push(UseTemplateRef {
                call: Self::call_range(call),
                name: self.assigned_name(call),
            });
        }
    }

    fn define_props(&mut self, call: &CallExpression<'a>) -> DefineProps {
        let mut props = DefineProps {
            call: self.macro_call(call),
            destructured: None,
            destructured_rest: None,
        };
        let mut parents = self.parents();
        let parent = parents.next();
        let grand = parents.next();
        if let Some(declarator) = Self::declarator_of(parent, call.span) {
            if let BindingPattern::ObjectPattern(_) = &declarator.id {
                let mut names = Vec::new();
                collect_binding_identifiers(&declarator.id, &mut names, false, None);
                let mut destructured: Vec<(String, Option<Range>)> = Vec::new();
                for name in names {
                    let text = self.range_text(name.span).to_owned();
                    if name.is_rest {
                        props.destructured_rest = Some(text);
                        continue;
                    }
                    let initializer = name.initializer.map(Range::from);
                    // A `Map`: a name bound twice keeps its place.
                    if let Some(existing) =
                        destructured.iter_mut().find(|(known, _)| *known == text)
                    {
                        existing.1 = initializer;
                    } else {
                        destructured.push((text, initializer));
                    }
                }
                props.destructured = Some(destructured);
            }
        } else if let Some(AstKind::CallExpression(outer)) = parent
            && self
                .options
                .with_defaults
                .iter()
                .any(|name| name == self.range_text(outer.callee.span()))
        {
            // `const props = withDefaults(defineProps(), { ... })`
            if let Some(declarator) = Self::declarator_of(grand, outer.span) {
                props.call.name = Some(self.range_text(declarator.id.span()).to_owned());
            }
        }
        props
    }

    fn define_model(&mut self, call: &CallExpression<'a>) -> DefineModel {
        let mut model = DefineModel {
            arg: Some(call.span.into()),
            comments: self.closest_multi_line_comment(),
            ..DefineModel::default()
        };
        if let Some(declarator) = Self::declarator_of(self.parents().next(), call.span)
            && let BindingPattern::BindingIdentifier(identifier) = &declarator.id
        {
            model.local_name = Some(identifier.span.into());
        }
        if let Some(arguments) = &call.type_arguments {
            model.r#type = arguments
                .params
                .first()
                .map(|argument| argument.span().into());
            model.modifier_type = arguments
                .params
                .get(1)
                .map(|argument| argument.span().into());
        }

        // `defineModel('name', { ... })`, `defineModel('name')` or
        // `defineModel({ ... })`.
        let is_string = |argument: &Argument| match argument {
            Argument::StringLiteral(_) => true,
            Argument::TemplateLiteral(template) => template.expressions.is_empty(),
            _ => false,
        };
        let first = call.arguments.first();
        let (prop_name, options) = match call.arguments.get(1) {
            Some(second) => (first, Some(second)),
            None if first.is_some_and(is_string) => (first, None),
            None => (None, first),
        };
        if let Some(prop_name) = prop_name.filter(|argument| is_string(argument)) {
            model.name = Some(prop_name.span().into());
        }
        let Some(Argument::ObjectExpression(options)) = options else {
            return model;
        };
        for property in &options.properties {
            let ObjectPropertyKind::ObjectProperty(property) = property else {
                continue;
            };
            let PropertyKey::StaticIdentifier(name) = &property.key else {
                continue;
            };
            // `name: value`, or the method `default() {}`.
            let is_assignment =
                property.kind == PropertyKind::Init && !property.method && !property.shorthand;
            let is_default_method =
                property.kind == PropertyKind::Init && property.method && name.name == "default";
            if !is_assignment && !is_default_method {
                continue;
            }
            match name.name.as_str() {
                "type" => model.runtime_type = Some(property.value.span().into()),
                "default" => {
                    if is_assignment {
                        model.default_value = Some(property.value.span().into());
                    }
                    model.default_props_arg = first_untyped_parameter(&property.value);
                }
                "required" => {
                    if matches!(&property.value, Expression::BooleanLiteral(literal) if literal.value)
                    {
                        model.required = true;
                    }
                }
                _ => {}
            }
        }
        model
    }
}

/// The first parameter of a function, when it has no type and isn't a rest
/// parameter: where `defineModel`'s `default` receives the props.
fn first_untyped_parameter(value: &Expression) -> Option<Range> {
    let (this_param, params) = match value {
        Expression::FunctionExpression(function) => {
            (function.this_param.as_ref(), &function.params)
        }
        Expression::ArrowFunctionExpression(function) => (None, &function.params),
        _ => return None,
    };
    // TypeScript counts `this` as the first parameter.
    if let Some(this_param) = this_param {
        return this_param
            .type_annotation
            .is_none()
            .then(|| this_param.span.into());
    }
    let first = params.items.first()?;
    first.type_annotation.is_none().then(|| first.span.into())
}

/// Whether `defineEmits<{ (event: 'a' | 'b'): void }>()` has a call signature
/// whose first parameter is a union.
fn has_union_type_arg(call: &CallExpression) -> bool {
    let Some(TSType::TSTypeLiteral(literal)) = call
        .type_arguments
        .as_ref()
        .and_then(|arguments| arguments.params.first())
    else {
        return false;
    };
    literal.members.iter().any(|member| {
        let TSSignature::TSCallSignatureDeclaration(signature) = member else {
            return false;
        };
        // TypeScript counts `this` as the first parameter.
        let first = match &signature.this_param {
            Some(this_param) => this_param.type_annotation.as_ref(),
            None => signature
                .params
                .items
                .first()
                .and_then(|parameter| parameter.type_annotation.as_ref()),
        };
        first.is_some_and(|annotation| matches!(annotation.type_annotation, TSType::TSUnionType(_)))
    })
}
