//! The template as TypeScript: a port of `codegen/template/index.ts` and
//! `templateChild.ts`, `vIf.ts`, `vFor.ts`, `objectProperty.ts` and
//! `propertyAccess.ts`. The rest of `codegen/template` is in the modules
//! beside this one.
//!
//! The original's functions are generators that yield into one stream. Here
//! they are methods that append to `self.out`. Where the original collects a
//! generator's output to use it twice or later, `buffered` swaps the stream
//! for an empty one.

mod analysis;
mod directives;
mod element;
mod events;
mod props;
mod scoped_classes;
mod slots;

use std::collections::HashSet;

use super::context::{CommentInfo, Condition, TemplateContext};
use super::interpolation::{IdentifierOptions, generate_interpolation};
use crate::codegen::{
    Boundary, Code, END_OF_LINE, Features, NEW_LINE, Source, Verification, as_type,
    generate_camelized, generate_string_literal_key, is_identifier, to_string,
};
use crate::js;
use crate::codegen::CoreVersion;
use crate::template::ast::{Element, ElementType, Loc, Node, NodeId, Prop, Root};

/// The compiler options the template codegen reads (`vueCompilerOptions`).
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the original's own options, one to one"
)]
pub struct CompilerFlags {
    pub core: CoreVersion,
    pub lib: String,
    pub check_unknown_props: bool,
    pub check_unknown_events: bool,
    pub check_unknown_directives: bool,
    pub check_unknown_components: bool,
    pub strict_v_model: bool,
    pub infer_template_dollar_slots: bool,
    pub infer_template_dollar_attrs: bool,
    pub infer_template_dollar_refs: bool,
    pub infer_template_dollar_el: bool,
    pub infer_component_dollar_el: bool,
    pub infer_component_dollar_refs: bool,
    pub fallthrough_attributes: bool,
    pub check_required_fallthrough_attributes: bool,
    pub fallthrough_component_names: Vec<String>,
    /// The patterns of `dataAttributes` and `htmlAttributes`, which the
    /// original matches with picomatch.
    pub data_attributes: Vec<String>,
    pub html_attributes: Vec<String>,
}

impl Default for CompilerFlags {
    /// The options of a project that sets none (`getDefaultCompilerOptions`,
    /// through `CompilerOptionsResolver.build`).
    fn default() -> Self {
        Self {
            core: CoreVersion::default(),
            lib: "vue".to_owned(),
            check_unknown_props: false,
            check_unknown_events: false,
            check_unknown_directives: false,
            check_unknown_components: false,
            strict_v_model: false,
            infer_template_dollar_slots: false,
            infer_template_dollar_attrs: false,
            infer_template_dollar_refs: false,
            infer_template_dollar_el: false,
            infer_component_dollar_el: false,
            infer_component_dollar_refs: false,
            fallthrough_attributes: false,
            check_required_fallthrough_attributes: false,
            // As `CompilerOptionsResolver.build` leaves them: hyphenated.
            fallthrough_component_names: ["transition", "keep-alive", "teleport", "suspense"]
                .map(str::to_owned)
                .to_vec(),
            data_attributes: Vec::new(),
            html_attributes: vec!["aria-*".to_owned()],
        }
    }
}

/// A picomatch pattern made of text and `*`, which is every pattern the
/// defaults have. Returns `None` for a pattern with anything else in it.
fn glob_matches(pattern: &str, text: &str) -> Option<bool> {
    if pattern.contains(['?', '[', ']', '{', '}', '(', ')', '!', '+', '@', '\\', '/']) {
        return None;
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return Some(false);
    };
    let mut parts: Vec<&str> = parts.collect();
    let Some(last) = parts.pop() else {
        // No `*`: the whole text.
        return Some(rest.is_empty());
    };
    for part in parts {
        match rest.find(part) {
            Some(found) => rest = rest.get(found + part.len()..).unwrap_or(""),
            None => return Some(false),
        }
    }
    // A `*` doesn't match a `/`, nor a leading `.`.
    Some(
        rest.ends_with(last)
            && !text.contains('/')
            && !(pattern.starts_with('*') && text.starts_with('.')),
    )
}

/// `TemplateCodegenOptions`.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the original's own options, one to one"
)]
pub struct TemplateOptions<'a> {
    /// The template block's content.
    pub template: &'a str,
    /// The text the template's tree was parsed from: the content, with the
    /// `>` the `vue-template-html` plugin may add.
    pub parsed: &'a str,
    /// `template.lang === 'html'`.
    pub is_html: bool,
    pub is_ts: bool,
    pub is_vapor: bool,
    pub compiler: &'a CompilerFlags,
    pub imported_components: &'a HashSet<String>,
    pub setup_refs: &'a HashSet<String>,
    pub setup_consts: &'a HashSet<String>,
    pub setup_bindings: &'a HashSet<String>,
    pub dot_value_bindings: &'a HashSet<String>,
    pub reassert_bindings: &'a HashSet<String>,
    pub has_define_slots: bool,
    pub props_assign_name: Option<&'a str>,
    pub slots_assign_name: Option<&'a str>,
    pub inherit_attrs: bool,
    pub component_name: &'a str,
}

/// What `generateTemplate` returns, as far as the script codegen reads it.
#[derive(Debug)]
pub struct GeneratedTemplate {
    pub codes: Vec<Code>,
    pub generated_types: Vec<String>,
    /// `contextAccesses.keys()`.
    pub context_access_names: Vec<String>,
    pub dot_value_accesses: Vec<String>,
    pub unsupported: Option<&'static str>,
}

// The original's `codeFeatures`, as far as a type check sees them.
const ALL: Features = Features::new(Verification::On);
const VERIFICATION: Features = Features::new(Verification::On);
const NAVIGATION: Features = Features::new(Verification::Off);
const COMPLETION: Features = Features::new(Verification::Off);
const WITHOUT_HIGHLIGHT: Features = Features::new(Verification::On);
const WITHOUT_HIGHLIGHT_AND_COMPLETION: Features = Features::new(Verification::On);
const WITHOUT_COMPLETION: Features = Features::new(Verification::On);
const WITHOUT_NAVIGATION: Features = Features::new(Verification::On);
const IMPORT_COMPLETION_ONLY: Features = Features::new(Verification::Off);
const NAVIGATION_AND_COMPLETION: Features = Features::new(Verification::Off);
const NAVIGATION_AND_VERIFICATION: Features = Features::new(Verification::On);
const NOT_TS6133: Features = Features::new(Verification::NotTs6133);
const NOT_TS2353: Features = Features::new(Verification::NotTs2353OrTs2561);

struct Generator<'a> {
    options: &'a TemplateOptions<'a>,
    root: &'a Root,
    ctx: TemplateContext,
    out: Vec<Code>,
    /// How much of `out` has had its features resolved against the directive
    /// comments in force (`resolveCodeFeatures`).
    resolved: usize,
}

/// `generateTemplate`.
pub fn generate_template(options: &TemplateOptions, root: Option<&Root>) -> GeneratedTemplate {
    let empty = Root {
        nodes: Vec::new(),
        children: Vec::new(),
        loc: Loc::default(),
        errors: Vec::new(),
        components: Vec::new(),
    };
    let mut generator = Generator {
        options,
        root: root.unwrap_or(&empty),
        ctx: TemplateContext::default(),
        out: Vec::new(),
        resolved: 0,
    };
    generator.ctx.core = options.compiler.core;
    generator.worker(root.is_some());
    generator.resolve_features();
    GeneratedTemplate {
        codes: generator.out,
        generated_types: generator.ctx.generated_types.clone(),
        context_access_names: generator.ctx.context_access_names(),
        dot_value_accesses: generator.ctx.dot_value_accesses.clone(),
        unsupported: generator.ctx.unsupported,
    }
}

/// `directiveCommentRE`: `/^<!--\s*@vue-(?<name>[-\w]+)\b(?<content>[\s\S]*)-->$/`.
fn directive_comment(source: &str) -> Option<(&str, &str)> {
    let rest = js::trim_start(source.strip_prefix("<!--")?).strip_prefix("@vue-")?;
    let is_word = |char: char| char.is_ascii_alphanumeric() || char == '_';
    let run = rest
        .find(|char: char| !(is_word(char) || char == '-'))
        .unwrap_or(rest.len());
    // The longest name that ends at a word boundary, with `-->` after it.
    (1..=run).rev().find_map(|length| {
        let (name, after) = rest.split_at_checked(length)?;
        let last_is_word = name.chars().next_back().is_some_and(is_word);
        let next_is_word = after.chars().next().is_some_and(is_word);
        if last_is_word == next_is_word {
            return None;
        }
        Some((name, after.strip_suffix("-->")?))
    })
}

impl<'a> Generator<'a> {
    // ---- Writing ----

    fn text(&mut self, text: impl Into<String>) {
        self.out.push(Code::Text(text.into()));
    }

    fn mapped(&mut self, text: impl Into<String>, offset: usize, features: Features) {
        self.out.push(Code::Mapped {
            text: text.into(),
            source: Source::Template,
            offset,
            features,
        });
    }

    fn boundary(&mut self, start: usize, end: usize, features: Features) -> Boundary {
        let token = self.ctx.token();
        Boundary::start(&mut self.out, token, Source::Template, start, end, features)
    }

    fn camelized(&mut self, code: &str, offset: usize, features: Features) {
        let token = self.ctx.token();
        generate_camelized(
            &mut self.out,
            token,
            code,
            Source::Template,
            offset,
            features,
        );
    }

    fn string_literal_key(&mut self, code: &str, offset: usize, features: Features) {
        let token = self.ctx.token();
        generate_string_literal_key(&mut self.out, token, code, offset, features);
    }

    /// What `write` generates, apart from the stream.
    fn buffered(&mut self, write: impl FnOnce(&mut Self)) -> Vec<Code> {
        let stream = std::mem::take(&mut self.out);
        write(self);
        std::mem::replace(&mut self.out, stream)
    }

    fn identifier_options(&self) -> IdentifierOptions<'a> {
        IdentifierOptions {
            setup_refs: self.options.setup_refs,
            setup_consts: self.options.setup_consts,
            setup_bindings: self.options.setup_bindings,
            dot_value_bindings: self.options.dot_value_bindings,
            reassert_bindings: self.options.reassert_bindings,
            lib: &self.options.compiler.lib,
            is_ts: self.options.is_ts,
            core: self.options.compiler.core,
        }
    }

    /// `generateInterpolation` of a piece of the template.
    fn interpolation(
        &mut self,
        features: Features,
        code: &str,
        start: usize,
        prefix: &str,
        suffix: &str,
        in_narrowing: bool,
    ) {
        let options = self.identifier_options();
        generate_interpolation(
            &options,
            &mut self.ctx,
            Source::Template,
            features,
            code,
            start,
            prefix,
            suffix,
            in_narrowing,
            &mut self.out,
        );
    }

    /// `generateTypedVar`: `let name!: T;`, or a `JSDoc` cast in JavaScript.
    fn typed_var(&mut self, kind: &str, name: &str, write_type: impl FnOnce(&mut Self)) {
        if self.options.is_ts {
            self.text(format!("{kind} {name}!: "));
            write_type(self);
            self.text(END_OF_LINE);
        } else {
            self.text(format!("var {name} = /** @type {{"));
            write_type(self);
            self.text(format!("}} */ ({{}}){END_OF_LINE}"));
        }
    }

    /// `generateTypeAlias`: `type name = T;`, or a `JSDoc` typedef.
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

    // ---- Reading the tree ----

    /// A location's text.
    fn source(&self, loc: &Loc) -> &'a str {
        self.options
            .parsed
            .get(loc.source.start..loc.source.end)
            .unwrap_or("")
    }

    /// `getElementTagOffsets`: where the tag's name is, in its start tag and
    /// its end tag.
    fn tag_offsets(&mut self, node: &Element) -> (usize, Option<usize>) {
        let start = js::index_of(self.options.template, &node.tag, node.loc.start.offset)
            .unwrap_or_else(|| {
                self.ctx
                    .give_up("an element's tag isn't found in the template");
                node.loc.start.offset
            });
        let mut end = None;
        if !node.is_self_closing
            && self.options.is_html
            && let Some(last) = self.source(&node.loc).rfind(node.tag.as_str())
        {
            let offset = node.loc.start.offset + last;
            if offset > start {
                end = Some(offset);
            }
        }
        (start, end)
    }

    fn is_data_attribute(&mut self, name: &str) -> bool {
        self.matches_any(&self.options.compiler.data_attributes, name)
    }

    fn is_html_attribute(&mut self, name: &str) -> bool {
        self.matches_any(&self.options.compiler.html_attributes, name)
    }

    fn matches_any(&mut self, patterns: &'a [String], name: &str) -> bool {
        patterns.iter().any(|pattern| {
            glob_matches(pattern, name).unwrap_or_else(|| {
                self.ctx
                    .give_up("an attribute pattern the port doesn't match as picomatch does");
                false
            })
        })
    }

    // ---- Directive comments (`ctx.enter`, `ctx.exit`, `resolveCodeFeatures`) ----

    /// The codes written since the last time take the directive comments in
    /// force: under `@vue-ignore` nothing is reported, and under
    /// `@vue-expect-error` what is reported is counted, not shown.
    fn resolve_features(&mut self) {
        let info = self.ctx.comment_stack.last();
        let verification = match info {
            Some(info) if info.ignore_error => Some(Verification::Off),
            Some(info) if info.expect_error.is_some() => Some(Verification::Stateful),
            _ => None,
        };
        if let Some(verification) = verification {
            for code in self.out.iter_mut().skip(self.resolved) {
                if let Code::Mapped { features, .. } = code
                    && features.verification != Verification::Off
                {
                    features.verification = verification;
                }
            }
        }
        self.resolved = self.out.len();
    }

    /// `ctx.enter`: whether the node is generated. A comment is kept for the
    /// node after it.
    fn enter(&mut self, node: Option<NodeId>) -> bool {
        if let Some(id) = node
            && matches!(self.root.nodes.get(id.0), Some(Node::Comment(_)))
        {
            self.ctx.comment_buffer.push(id);
            return false;
        }
        let mut info = CommentInfo::default();
        for comment_id in std::mem::take(&mut self.ctx.comment_buffer) {
            let Some(Node::Comment(comment)) = self.root.nodes.get(comment_id.0) else {
                continue;
            };
            let source = self.source(&comment.loc);
            let Some((name, content)) = directive_comment(source) else {
                continue;
            };
            match name {
                "skip" => return false,
                "ignore" => info.ignore_error = true,
                "expect-error" => info.expect_error = Some(comment_id),
                "generic" => {
                    let text = js::trim(content);
                    if text.starts_with('{') && text.ends_with('}') {
                        // `slice(1, -1)`: a lone `{` gives nothing.
                        let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or("");
                        let offset = comment.loc.start.offset + source.find('{').unwrap_or(0) + 1;
                        info.generic = Some((inner.to_owned(), offset));
                    }
                }
                _ => {}
            }
        }
        self.resolve_features();
        self.ctx.comment_stack.push(info);
        true
    }

    /// `ctx.exit`.
    fn exit(&mut self) {
        self.resolve_features();
        let info = self.ctx.comment_stack.pop();
        self.ctx.comment_buffer.clear();
        let comment = info
            .and_then(|info| info.expect_error)
            .and_then(|id| self.root.nodes.get(id.0));
        if let Some(Node::Comment(comment)) = comment {
            // Reported when nothing under it was.
            let boundary = self.boundary(
                comment.loc.start.offset,
                comment.loc.end.offset,
                Features::new(Verification::Stateful),
            );
            self.text("// @ts-expect-error");
            self.out.push(boundary.end());
            self.text(format!("{NEW_LINE}{END_OF_LINE}"));
        }
    }

    // ---- `index.ts` ----

    fn worker(&mut self, has_ast: bool) {
        let options = self.options;
        let compiler = options.compiler;
        self.ctx.open_scope();
        if let Some(name) = options.slots_assign_name {
            self.ctx.declare(name);
        }
        if let Some(name) = options.props_assign_name {
            self.ctx.declare(name);
        }
        for (inferred, name) in [
            (compiler.infer_template_dollar_slots, "$slots"),
            (compiler.infer_template_dollar_attrs, "$attrs"),
            (compiler.infer_template_dollar_refs, "$refs"),
            (compiler.infer_template_dollar_el, "$el"),
        ] {
            if inferred {
                self.ctx.dollar_vars.insert(name.to_owned());
            }
        }
        if has_ast {
            self.root_node();
        }
        self.ctx
            // Before 3.3.14 the assertion is written in JavaScript too.
            .generate_hoist_variables(
                options.is_ts || !options.compiler.core.is_3_3_14(),
                &mut self.out,
            );
        self.slots_type();
        self.inherited_attrs_type();
        self.template_refs_type();
        self.root_el_type();

        if !self.ctx.dollar_vars.is_empty() {
            // 3.3.14 hands it out of the template's function.
            if options.compiler.core.is_3_3_14() {
                self.ctx.add_generated_type("__VLS_dollars");
            }
            self.typed_var("var", "__VLS_dollars", |generator| {
                let lib = &compiler.lib;
                generator.text(format!("{{{NEW_LINE}"));
                if generator.ctx.dollar_vars.contains("$slots") {
                    let slots = if generator.ctx.has_generated_type("__VLS_Slots") {
                        "__VLS_Slots"
                    } else {
                        "{}"
                    };
                    generator.text(format!("$slots: {slots}{END_OF_LINE}"));
                }
                if generator.ctx.dollar_vars.contains("$attrs") {
                    generator.text(format!(
                        "$attrs: import('{lib}').ComponentPublicInstance['$attrs']"
                    ));
                    if generator.ctx.has_generated_type("__VLS_InheritedAttrs") {
                        generator.text(" & __VLS_InheritedAttrs");
                    }
                    generator.text(END_OF_LINE);
                }
                if generator.ctx.dollar_vars.contains("$refs") {
                    let refs = if generator.ctx.has_generated_type("__VLS_TemplateRefs") {
                        "__VLS_TemplateRefs"
                    } else {
                        "{}"
                    };
                    generator.text(format!("$refs: {refs}{END_OF_LINE}"));
                }
                if generator.ctx.dollar_vars.contains("$el") {
                    let el = if generator.ctx.has_generated_type("__VLS_RootEl") {
                        "__VLS_RootEl"
                    } else {
                        "any"
                    };
                    generator.text(format!("$el: {el}{END_OF_LINE}"));
                }
                generator.text(format!(
                    "}} & {{ [K in keyof import('{lib}').ComponentPublicInstance]: unknown }}"
                ));
            });
        }
        self.ctx.end_scope(&mut self.out);
    }

    fn slots_type(&mut self) {
        if self.options.has_define_slots {
            self.ctx.add_generated_type("__VLS_Slots");
            return;
        }
        if self.ctx.slots.is_empty() && self.ctx.dynamic_slots.is_empty() {
            return;
        }
        self.ctx.add_generated_type("__VLS_Slots");
        self.type_alias("__VLS_Slots", |generator| {
            generator.text("{}");
            for (exp_var, props_var) in generator.ctx.dynamic_slots.clone() {
                generator.text(format!(
                    "{NEW_LINE}& {{ [K in NonNullable<typeof {exp_var}>]?: (props: typeof {props_var}) => any }}"
                ));
            }
            for slot in generator.ctx.slots.clone() {
                generator.text(format!("{NEW_LINE}& {{ "));
                match slot.offset {
                    Some(offset) if !slot.name.is_empty() => {
                        generator.object_property(&slot.name, offset, NAVIGATION, false, false);
                    }
                    _ => {
                        let boundary = generator.boundary(slot.tag_range.0, slot.tag_range.1, NAVIGATION);
                        generator.text("default");
                        generator.out.push(boundary.end());
                    }
                }
                generator.text(format!("?: (props: typeof {}) => any }}", slot.props_var));
            }
        });
    }

    fn inherited_attrs_type(&mut self) {
        if self.ctx.inherited_attr_vars.is_empty() {
            return;
        }
        self.ctx.add_generated_type("__VLS_InheritedAttrs");
        let attrs = self
            .ctx
            .inherited_attr_vars
            .iter()
            .map(|name| format!("typeof {name}"))
            .collect::<Vec<_>>()
            .join(" & ");
        let required = self.options.compiler.check_required_fallthrough_attributes;
        self.type_alias("__VLS_InheritedAttrs", |generator| {
            generator.text(if required {
                attrs
            } else {
                format!("Partial<{attrs}>")
            });
        });
    }

    fn template_refs_type(&mut self) {
        let compiler = self.options.compiler;
        if self.ctx.template_refs.is_empty()
            || !(compiler.infer_template_dollar_refs
                || compiler.infer_component_dollar_refs
                || !self.options.setup_refs.is_empty())
        {
            return;
        }
        self.ctx.add_generated_type("__VLS_TemplateRefs");
        self.type_alias("__VLS_TemplateRefs", |generator| {
            generator.text("{}");
            for (name, refs) in generator.ctx.template_refs.clone() {
                generator.text(format!("{NEW_LINE}& "));
                if refs.len() >= 2 {
                    generator.text("(");
                }
                for (index, (type_exp, offset)) in refs.iter().enumerate() {
                    if index > 0 {
                        generator.text(" | ");
                    }
                    generator.text("{ ");
                    generator.object_property(&name, *offset, NAVIGATION, false, false);
                    generator.text(format!(": {type_exp} }}"));
                }
                if refs.len() >= 2 {
                    generator.text(")");
                }
            }
        });
    }

    fn root_el_type(&mut self) {
        if self.ctx.single_root_el_types.is_empty() || self.ctx.has_no_single_root {
            return;
        }
        self.ctx.add_generated_type("__VLS_RootEl");
        self.type_alias("__VLS_RootEl", |generator| {
            for type_text in generator.ctx.single_root_el_types.clone() {
                generator.text(format!("{NEW_LINE}| {type_text}"));
            }
        });
    }

    // ---- `templateChild.ts` ----

    /// `generateTemplateChild` of the root.
    fn root_node(&mut self) {
        if !self.enter(None) {
            return;
        }
        let root = self.root;
        self.collect_single_root_nodes(&root.children, false);
        for child in &root.children {
            self.template_child(*child, true, false);
        }
        self.exit();
    }

    /// `collectSingleRootNodes`: the elements that are the template's one
    /// root, through `v-if` branches and the components attributes fall
    /// through.
    fn collect_single_root_nodes(&mut self, children: &[NodeId], treat_template_as_fragment: bool) {
        let root = self.root;
        let children: Vec<NodeId> = children
            .iter()
            .copied()
            .filter(|id| !matches!(root.nodes.get(id.0), Some(Node::Comment(_))))
            .collect();
        let [child] = children.as_slice() else {
            if children.len() > 1 {
                self.ctx.has_no_single_root = true;
            }
            return;
        };
        match root.nodes.get(child.0) {
            Some(Node::If(if_node)) => {
                for branch in &if_node.branches {
                    if let Some(Node::IfBranch(branch)) = root.nodes.get(branch.0) {
                        self.collect_single_root_nodes(&branch.children, true);
                    }
                }
            }
            Some(Node::Element(element)) => {
                if element.tag_type == ElementType::Template && treat_template_as_fragment {
                    self.collect_single_root_nodes(&element.children, false);
                    return;
                }
                self.ctx.single_root_nodes.insert(*child);
                let tag = js::hyphenate(&element.tag);
                if self
                    .options
                    .compiler
                    .fallthrough_component_names
                    .contains(&tag)
                {
                    self.collect_single_root_nodes(&element.children, false);
                }
            }
            _ => {}
        }
    }

    /// `generateTemplateChild`.
    fn template_child(&mut self, id: NodeId, enter_node: bool, treat_template_as_fragment: bool) {
        if enter_node && !self.enter(Some(id)) {
            return;
        }
        let root = self.root;
        match root.nodes.get(id.0) {
            Some(Node::Element(node)) => {
                if node.tag_type == ElementType::Slot {
                    self.slot_outlet(node);
                } else {
                    let slot_dir = node.props.iter().find_map(|prop| match prop {
                        Prop::Directive(directive) if directive.name == "slot" => {
                            Some(&**directive)
                        }
                        _ => None,
                    });
                    let is_template = node.tag_type == ElementType::Template;
                    if is_template && !self.ctx.components.is_empty() && slot_dir.is_some() {
                        // `ctx.components.at(-1)!()`: the component's context is used.
                        let ctx_var = self.ctx.components.last_mut().map(|component| {
                            component.is_ctx_var_used = true;
                            component.ctx_var.clone()
                        });
                        self.v_slot(node, slot_dir, &ctx_var.unwrap_or_default());
                    } else if is_template && treat_template_as_fragment {
                        self.fragment(node);
                    } else if node.tag_type == ElementType::Component {
                        self.component(id, node);
                    } else {
                        self.plain_element(id, node);
                    }
                }
            }
            // `{{ a }} {{ b }}`
            Some(Node::Compound(compound)) => {
                for child in &compound.children {
                    if let crate::template::ast::CompoundChild::Node(child) = child {
                        self.template_child(*child, false, false);
                    }
                }
            }
            Some(Node::Interpolation(node)) => {
                let (content, start) = self.parse_interpolation_node(&node.content.loc);
                self.interpolation(ALL, content, start, "(", &format!("){END_OF_LINE}"), false);
            }
            Some(Node::If(node)) => self.v_if(&node.branches),
            Some(Node::For(node)) => self.v_for(node),
            _ => {}
        }
        if enter_node {
            self.exit();
        }
    }

    /// `parseInterpolationNode`: the expression of `{{ }}`, with the
    /// whitespace around it.
    fn parse_interpolation_node(&self, loc: &Loc) -> (&'a str, usize) {
        let template = self.options.template;
        let mut start = loc.start.offset;
        let mut end = loc.end.offset;
        while let Some(char) = template
            .get(..start)
            .and_then(|before| before.chars().next_back())
        {
            if !js::is_whitespace(char) {
                break;
            }
            start -= char.len_utf8();
        }
        while let Some(char) = template.get(end..).and_then(|after| after.chars().next()) {
            if !js::is_whitespace(char) {
                break;
            }
            end += char.len_utf8();
        }
        (template.get(start..end).unwrap_or(""), start)
    }

    // ---- `vIf.ts` ----

    fn v_if(&mut self, branches: &'a [NodeId]) {
        let root = self.root;
        let original_conditions = self.ctx.conditions.len();
        for (index, branch_id) in branches.iter().enumerate() {
            let Some(Node::IfBranch(branch)) = root.nodes.get(branch_id.0) else {
                continue;
            };
            self.text(if index == 0 {
                "if "
            } else if branch.condition.is_some() {
                "else if "
            } else {
                "else "
            });
            let mut added_condition = false;
            if let Some(condition) = &branch.condition {
                let access_mark = self.ctx.access_log.len();
                let codes = self.buffered(|generator| {
                    generator.interpolation(
                        ALL,
                        &condition.content,
                        condition.loc.start.offset,
                        "(",
                        ")",
                        true,
                    );
                });
                let text = to_string(&codes);
                self.out.extend(codes);
                self.ctx.conditions.push(Condition {
                    text,
                    accesses: self
                        .ctx
                        .access_log
                        .get(access_mark..)
                        .unwrap_or(&[])
                        .to_vec(),
                });
                added_condition = true;
                self.text(" ");
            }
            self.text(format!("{{{NEW_LINE}"));
            for child in &branch.children {
                self.template_child(*child, index != 0, true);
            }
            self.text(format!("}}{NEW_LINE}"));
            if added_condition && let Some(condition) = self.ctx.conditions.last_mut() {
                // The branches after this one are where it didn't hold.
                condition.text = format!("!{}", condition.text);
            }
        }
        self.ctx.conditions.truncate(original_conditions);
    }

    // ---- `vFor.ts` ----

    fn v_for(&mut self, node: &'a crate::template::ast::For) {
        let result = &node.parse_result;
        // `parseVForNode`: the left side, from its first part to its last.
        let first = result
            .value
            .as_ref()
            .or(result.key.as_ref())
            .or(result.index.as_ref());
        let last = result
            .index
            .as_ref()
            .or(result.key.as_ref())
            .or(result.value.as_ref());
        let left = first.zip(last).and_then(|(first, last)| {
            let (start, end) = (first.loc.start.offset, last.loc.end.offset);
            let text = self.source(&node.loc).get(
                start.checked_sub(node.loc.start.offset)?
                    ..end.checked_sub(node.loc.start.offset)?,
            )?;
            (!text.is_empty()).then_some((start, text))
        });
        self.ctx.open_scope();
        let mut binding_names = Vec::new();
        let mut default_ranges = Vec::new();
        if let Some((_, text)) = left {
            match analysis::for_bindings(text) {
                Some(bindings) => {
                    binding_names = bindings.names;
                    default_ranges = bindings.default_ranges;
                }
                None => self
                    .ctx
                    .give_up("the left side of a v-for isn't a list of bindings"),
            }
        }

        // The source is read before the loop's names are in scope; a default
        // in the left side is read after, and sees the names before it.
        let source_alias = self.ctx.internal_variable();
        // Keeps a literal source from widening to `number[]`.
        self.text(format!("const {source_alias} = __VLS_tryAsConstant("));
        self.interpolation(
            ALL,
            &result.source.content,
            result.source.loc.start.offset,
            "(",
            ")",
            false,
        );
        self.text(")");
        self.text(END_OF_LINE);

        for name in &binding_names {
            self.ctx.declare(name);
        }

        self.text("for (const [");
        if let Some((left_start, text)) = left {
            let mut last_offset = 0;
            for (start, end) in default_ranges {
                if start > last_offset {
                    self.mapped(
                        text.get(last_offset..start).unwrap_or(""),
                        left_start + last_offset,
                        ALL,
                    );
                }
                self.interpolation(
                    ALL,
                    text.get(start..end).unwrap_or(""),
                    left_start + start,
                    "",
                    "",
                    false,
                );
                last_offset = end;
            }
            if last_offset < text.len() {
                self.mapped(
                    text.get(last_offset..).unwrap_or(""),
                    left_start + last_offset,
                    ALL,
                );
            }
        }
        self.text("] of ");
        self.text("__VLS_vFor(__VLS_nonNull(");
        self.text(source_alias);
        self.text("))");
        self.text(format!(") {{{NEW_LINE}"));

        let in_v_for = std::mem::replace(&mut self.ctx.in_v_for, true);
        for child in &node.children {
            self.template_child(*child, false, true);
        }
        self.ctx.in_v_for = in_v_for;

        self.ctx.end_scope(&mut self.out);
        self.text(format!("}}{NEW_LINE}"));
    }

    // ---- `objectProperty.ts`, `propertyAccess.ts` ----

    /// `generateObjectProperty`: a name as an object's key.
    fn object_property(
        &mut self,
        code: &str,
        offset: usize,
        features: Features,
        should_camelize: bool,
        should_be_constant: bool,
    ) {
        if code.starts_with('[') && code.ends_with(']') && code.len() >= 2 {
            if should_be_constant {
                let inner = code.get(1..code.len() - 1).unwrap_or("");
                self.interpolation(
                    features,
                    inner,
                    offset + 1,
                    "[__VLS_tryAsConstant(",
                    ")]",
                    false,
                );
            } else {
                self.interpolation(features, code, offset, "", "", false);
            }
        } else if should_camelize {
            if is_identifier(&js::camelize(code)) {
                self.camelized(code, offset, features);
            } else {
                let boundary = self.boundary(offset, offset + code.len(), features);
                self.text("'");
                self.camelized(code, offset, boundary.features);
                self.text("'");
                self.out.push(boundary.end());
            }
        } else if is_identifier(code) {
            self.mapped(code, offset, features);
        } else {
            self.string_literal_key(code, offset, features);
        }
    }

    /// `generatePropertyAccess`: `.name`, `['a-name']` or `[expression]`.
    fn property_access(&mut self, code: &str, offset: usize, features: Features) {
        if code.starts_with('[') && code.ends_with(']') && code.len() >= 2 {
            self.interpolation(features, code, offset, "", "", false);
        } else if is_identifier(code) {
            self.text(".");
            self.mapped(code, offset, features);
        } else {
            self.text("[");
            self.string_literal_key(code, offset, features);
            self.text("]");
        }
    }

    /// `asType('any', lang)` and friends.
    fn as_type(&self, type_text: &str) -> String {
        as_type(type_text, self.options.is_ts)
    }
}

#[cfg(test)]
mod tests {
    use super::{directive_comment, glob_matches};

    #[test]
    fn directive_comments() {
        assert_eq!(
            directive_comment("<!-- @vue-ignore -->"),
            Some(("ignore", " "))
        );
        assert_eq!(directive_comment("<!--@vue-skip-->"), Some(("skip", "")));
        assert_eq!(
            directive_comment("<!-- @vue-expect-error why -->"),
            Some(("expect-error", " why "))
        );
        assert_eq!(
            directive_comment("<!-- @vue-generic {T} -->"),
            Some(("generic", " {T} "))
        );
        assert_eq!(directive_comment("<!-- vue-ignore -->"), None);
        assert_eq!(directive_comment("<!-- @vue-ignore"), None);
    }

    #[test]
    fn globs() {
        assert_eq!(glob_matches("aria-*", "aria-label"), Some(true));
        assert_eq!(glob_matches("aria-*", "aria-"), Some(true));
        assert_eq!(glob_matches("aria-*", "data-x"), Some(false));
        assert_eq!(glob_matches("data-*-id", "data-a-b-id"), Some(true));
        assert_eq!(glob_matches("exact", "exact"), Some(true));
        assert_eq!(glob_matches("exact", "exactly"), Some(false));
        assert_eq!(glob_matches("a?c", "abc"), None);
    }
}
