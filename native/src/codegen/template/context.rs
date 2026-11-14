//! `createTemplateCodegenContext`: what the template codegen keeps track of
//! while it runs (`codegen/template/context.ts`).

use std::collections::HashSet;

use crate::codegen::Source;

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

/// One access of a name from the component's context: where, and the text
/// there (which can be the hyphenated name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access {
    pub name: String,
    pub source: Source,
    pub offset: Option<usize>,
    pub source_name: String,
    pub dot_value: bool,
}

#[derive(Debug, Default)]
pub struct TemplateContext {
    /// The names declared in each open scope, innermost last.
    pub scopes: Vec<HashSet<String>>,
    /// `$slots`, `$attrs`, `$refs`, `$el`, when they are inferred.
    pub dollar_vars: HashSet<String>,
    /// Every `accessVariable` call, in order. The original keeps them as
    /// `accessLog`, `contextAccesses` and `dotValueAccesses`, which are all
    /// read off this list.
    pub accesses: Vec<Access>,
    /// The next `__combineToken`.
    next_token: u32,
    /// Set when the original's result can't be reproduced: why.
    pub unsupported: Option<&'static str>,
}

impl TemplateContext {
    /// A new `Symbol()` for `__combineToken`.
    pub fn token(&mut self) -> u32 {
        self.next_token += 1;
        self.next_token
    }

    pub fn give_up(&mut self, reason: &'static str) {
        self.unsupported.get_or_insert(reason);
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

    /// `scope.declare(...)` on the innermost scope.
    pub fn declare(&mut self, name: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned());
        }
    }

    /// `ctx.accessVariable`.
    pub fn access_variable(
        &mut self,
        source: Source,
        name: &str,
        offset: Option<usize>,
        dot_value: bool,
    ) {
        self.accesses.push(Access {
            name: name.to_owned(),
            source,
            offset,
            source_name: name.to_owned(),
            dot_value,
        });
    }

    /// `shouldIdentifierSkipped`: a name that isn't the component's: one in
    /// scope, an allowed global, a literal, or one of the codegen's own.
    pub fn should_identifier_skipped(&self, text: &str) -> bool {
        self.scopes.iter().any(|scope| scope.contains(text))
            || is_globally_allowed(text)
            || matches!(text, "true" | "false" | "null" | "this" | "require")
            || text.starts_with("__VLS_")
    }
}
