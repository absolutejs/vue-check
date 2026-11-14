//! `createTemplateCodegenContext`: what the template codegen keeps track of
//! while it runs (`codegen/template/context.ts`).

use std::collections::HashSet;

use crate::codegen::{
    Code, END_OF_LINE, Features, NEW_LINE, Source, Verification, generate_camelized,
};
use crate::template::ast::NodeId;

/// `isGloballyAllowed` (`@vue/shared`): the globals a template can use.
fn is_globally_allowed(name: &str) -> bool {
    matches!(
        name,
        "Infinity"
            | "undefined"
            | "NaN"
            | "isFinite"
            | "isNaN"
            | "parseFloat"
            | "parseInt"
            | "decodeURI"
            | "decodeURIComponent"
            | "encodeURI"
            | "encodeURIComponent"
            | "Math"
            | "Number"
            | "Date"
            | "Array"
            | "Object"
            | "Boolean"
            | "String"
            | "RegExp"
            | "Map"
            | "Set"
            | "JSON"
            | "Intl"
            | "BigInt"
            | "console"
            | "Error"
            | "Symbol"
    )
}

/// Words that can't be the name of an imported binding, and that would be a
/// syntax error as a bare element of the array of names to import.
fn is_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "arguments"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "eval"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

/// What the directive comments before a node say (`<!-- @vue-ignore -->`).
#[derive(Clone, Debug, Default)]
pub struct CommentInfo {
    pub ignore_error: bool,
    /// `@vue-expect-error`: the comment.
    pub expect_error: Option<NodeId>,
    /// `@vue-generic {T}`: the type arguments, and where they are.
    pub generic: Option<(String, usize)>,
}

/// The accesses of one name from one source block: where, and the text there
/// (which can be the hyphenated name). A `Map` in the original: an offset
/// accessed again keeps its place.
#[derive(Debug)]
struct SourceAccesses {
    source: Source,
    offsets: Vec<(usize, String)>,
}

/// A branch condition in force, and the names it reads.
#[derive(Clone, Debug)]
pub struct Condition {
    pub text: String,
    pub accesses: Vec<String>,
}

/// A `<slot>` the template has.
#[derive(Clone, Debug)]
pub struct Slot {
    pub name: String,
    pub offset: Option<usize>,
    pub tag_range: (usize, usize),
    pub props_var: String,
}

/// The component an element's slots belong to.
#[derive(Clone, Debug)]
pub struct ComponentEntry {
    pub ctx_var: String,
    pub is_ctx_var_used: bool,
}

#[derive(Debug, Default)]
pub struct TemplateContext {
    // Directive comments.
    pub comment_stack: Vec<CommentInfo>,
    pub comment_buffer: Vec<NodeId>,

    /// The names declared in each open scope, innermost last.
    pub scopes: Vec<HashSet<String>>,

    /// Every name read from the component (`accessLog`).
    pub access_log: Vec<String>,
    /// `contextAccesses`: per name, in the order first read, the accesses
    /// from each source block.
    context_accesses: Vec<(String, Vec<SourceAccesses>)>,
    /// `dotValueAccesses`: the names read where TypeScript narrows.
    pub dot_value_accesses: Vec<String>,

    pub conditions: Vec<Condition>,
    /// `hoistVars`: each variable hoisted, and the name it is hoisted under.
    hoist_vars: Vec<(String, String)>,
    variable_id: usize,
    /// `templateRefs`: per `ref` name, each element's type and where.
    pub template_refs: Vec<(String, Vec<(String, usize)>)>,

    pub components: Vec<ComponentEntry>,
    /// `$slots`, `$attrs`, `$refs`, `$el`, when they are inferred.
    pub dollar_vars: HashSet<String>,
    pub generated_types: Vec<String>,
    pub inherited_attr_vars: Vec<String>,
    pub single_root_el_types: Vec<String>,
    pub single_root_nodes: HashSet<NodeId>,
    /// `singleRootNodes.has(null)`: the template doesn't always have one root.
    pub has_no_single_root: bool,
    pub slots: Vec<Slot>,
    pub dynamic_slots: Vec<(String, String)>,
    pub in_v_for: bool,

    /// The next `__combineToken`.
    next_token: u32,
    /// Set when the original's result can't be reproduced: why.
    pub unsupported: Option<&'static str>,
}

impl TemplateContext {
    /// A context whose combine tokens start after `first_token`, so that two
    /// pieces of generated code that end up in one file don't share any.
    pub fn starting_at(first_token: u32) -> Self {
        Self {
            next_token: first_token,
            ..Self::default()
        }
    }

    /// A new `Symbol()` for `__combineToken`.
    pub fn token(&mut self) -> u32 {
        self.next_token += 1;
        self.next_token
    }

    pub fn give_up(&mut self, reason: &'static str) {
        self.unsupported.get_or_insert(reason);
    }

    /// `getInternalVariable`.
    pub fn internal_variable(&mut self) -> String {
        let name = format!("__VLS_{}", self.variable_id);
        self.variable_id += 1;
        name
    }

    /// `getHoistVariable`.
    pub fn hoist_variable(&mut self, original: &str) -> String {
        if let Some((_, name)) = self.hoist_vars.iter().find(|(known, _)| known == original) {
            return name.clone();
        }
        let name = self.internal_variable();
        self.hoist_vars.push((original.to_owned(), name.clone()));
        name
    }

    /// `generateHoistVariables`: one `var` for all of them, which avoids
    /// TS4081.
    pub fn generate_hoist_variables(&self, is_ts: bool, out: &mut Vec<Code>) {
        if self.hoist_vars.is_empty() {
            return;
        }
        out.push("var ".into());
        for (index, (original, hoisted)) in self.hoist_vars.iter().enumerate() {
            if index > 0 {
                out.push(", ".into());
            }
            // A non-null assertion is TypeScript's own syntax.
            out.push(
                if is_ts {
                    format!("{hoisted} = {original}!")
                } else {
                    format!("{hoisted} = __VLS_nonNull({original})")
                }
                .into(),
            );
        }
        out.push(END_OF_LINE.into());
    }

    /// `addTemplateRef`.
    pub fn add_template_ref(&mut self, name: &str, type_exp: String, offset: usize) {
        match self
            .template_refs
            .iter_mut()
            .find(|(known, _)| known == name)
        {
            Some((_, refs)) => refs.push((type_exp, offset)),
            None => self
                .template_refs
                .push((name.to_owned(), vec![(type_exp, offset)])),
        }
    }

    pub fn add_generated_type(&mut self, name: &str) {
        if !self.has_generated_type(name) {
            self.generated_types.push(name.to_owned());
        }
    }

    pub fn has_generated_type(&self, name: &str) -> bool {
        self.generated_types.iter().any(|known| known == name)
    }

    /// `ctx.scope()`: opens a scope.
    pub fn open_scope(&mut self) {
        self.scopes.push(HashSet::new());
    }

    /// `scope.end()`, where the original drops the code it returns: closes
    /// the innermost scope.
    pub fn close_scope(&mut self) {
        self.scopes.pop();
    }

    /// `yield* scope.end()`: closes the innermost scope and writes the names
    /// to import.
    pub fn end_scope(&mut self, out: &mut Vec<Code>) {
        self.scopes.pop();
        self.generate_auto_import(out);
    }

    /// `scope.declare(...)` on the innermost scope.
    pub fn declare(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned());
        }
    }

    pub fn is_in_scope(&self, name: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains(name))
    }

    /// `ctx.accessVariable`: a name read from the component. `source_name`
    /// is the text at `offset`, when it isn't the name.
    pub fn access_variable(
        &mut self,
        source: Source,
        name: &str,
        offset: Option<usize>,
        dot_value: bool,
        source_name: Option<&str>,
    ) {
        self.access_log.push(name.to_owned());
        let index = self
            .context_accesses
            .iter()
            .position(|(known, _)| known == name)
            .unwrap_or_else(|| {
                self.context_accesses.push((name.to_owned(), Vec::new()));
                self.context_accesses.len() - 1
            });
        if let Some((_, sources)) = self.context_accesses.get_mut(index) {
            let position = sources
                .iter()
                .position(|known| known.source == source)
                .unwrap_or_else(|| {
                    sources.push(SourceAccesses {
                        source,
                        offsets: Vec::new(),
                    });
                    sources.len() - 1
                });
            if let (Some(offset), Some(accesses)) = (offset, sources.get_mut(position)) {
                let text = source_name.unwrap_or(name).to_owned();
                match accesses
                    .offsets
                    .iter_mut()
                    .find(|(known, _)| *known == offset)
                {
                    Some(existing) => existing.1 = text,
                    None => accesses.offsets.push((offset, text)),
                }
            }
        }
        if dot_value && !self.dot_value_accesses.iter().any(|known| known == name) {
            self.dot_value_accesses.push(name.to_owned());
        }
    }

    /// `contextAccesses.keys()`.
    pub fn context_access_names(&self) -> Vec<String> {
        self.context_accesses
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// `generateAutoImport`: every name read from the component since the
    /// last time, as an array, which is where an editor completes an import.
    fn generate_auto_import(&mut self, out: &mut Vec<Code>) {
        // Once a name was read, the original writes the array at every scope's
        // end, empty or not.
        if self.context_accesses.is_empty() {
            return;
        }
        out.push(format!("// @ts-ignore{NEW_LINE}").into());
        out.push("[".into());
        let mut accesses = std::mem::take(&mut self.context_accesses);
        for (name, sources) in &mut accesses {
            for accessed in sources {
                if !is_reserved_word(name) {
                    for (offset, source_name) in &accessed.offsets {
                        // `codeFeatures.importCompletionOnly`
                        let token = self.token();
                        generate_camelized(
                            out,
                            token,
                            source_name,
                            accessed.source,
                            *offset,
                            Features::new(Verification::Off),
                        );
                        out.push(",".into());
                    }
                }
                accessed.offsets.clear();
            }
        }
        self.context_accesses = accesses;
        out.push(format!("]{END_OF_LINE}").into());
    }

    /// `generateConditionGuards`: inside a handler, the conditions of the
    /// branches it is in, which TypeScript doesn't carry into a function.
    pub fn generate_condition_guards(&self, out: &mut Vec<Code>) {
        for condition in &self.conditions {
            if !condition.accesses.is_empty()
                && condition.accesses.iter().all(|name| self.is_in_scope(name))
            {
                continue;
            }
            out.push(format!("if (!{}) throw 0{END_OF_LINE}", condition.text).into());
        }
    }

    /// `shouldIdentifierSkipped`: a name that isn't the component's: one in
    /// scope, an allowed global, a literal, or one of the codegen's own.
    pub fn should_identifier_skipped(&self, text: &str) -> bool {
        self.is_in_scope(text)
            || is_globally_allowed(text)
            || matches!(text, "true" | "false" | "null" | "this" | "require")
            || text.starts_with("__VLS_")
    }
}
