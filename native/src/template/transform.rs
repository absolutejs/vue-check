//! The transforms Vue's language tools run over a parsed template: a port of
//! `@vue/compiler-core`'s `transform.ts` traversal and
//! `transformVBindShorthand`, and `@vue/language-core`'s own `v-if`, `v-for`,
//! element and text transforms (`template/transforms`, 3.3.14).
//!
//! The originals also report errors, which the codegen never reads; only
//! what they do to the tree is here. Where the original throws instead (a
//! development build of the compiler does, on some combinations of `v-if`
//! and `v-else` on one element), the result is marked unsupported.

use super::ast::{
    Compound, CompoundChild, ConstantType, Directive, ElementType, For, If, IfBranch, Node, NodeId,
    Prop, Root, SimpleExpression,
};
use crate::js;

/// Where a node is: the root's children, or a node's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    Root,
    Node(NodeId),
}

/// `TransformContext`: where the traversal is.
struct Context<'a> {
    root: &'a mut Root,
    parent: Option<Parent>,
    child_index: usize,
    /// The node being transformed. `None` once it has been removed.
    current: Option<NodeId>,
    /// For each `traverse_children` call, how many nodes were removed at or
    /// before its position during the current step (the original's
    /// `onNodeRemoved`, which steps that loop back by one).
    removals: Vec<usize>,
    /// Whose `onNodeRemoved` is current.
    frame: usize,
    unsupported: Option<&'static str>,
}

/// Runs the transforms. Returns why the result can't be used, if it can't.
pub fn transform(root: &mut Root) -> Option<&'static str> {
    let mut context = Context {
        root,
        parent: None,
        child_index: 0,
        current: None,
        removals: Vec::new(),
        frame: 0,
        unsupported: None,
    };
    // The root itself: only the text transform applies to it.
    context.traverse_children(Parent::Root);
    context.merge_text(Parent::Root);
    context.unsupported
}

/// `validFirstIdentCharRE` on a name's first UTF-16 unit:
/// `/[A-Za-z_$\xA0-￿]/`. Half of a surrogate pair is in that range, so
/// every character from U+00A0 up passes.
fn has_valid_first_ident_char(name: &str) -> bool {
    name.chars()
        .next()
        // The original tests the text "undefined" for an empty name.
        .is_none_or(|char| {
            char.is_ascii_alphabetic() || char == '_' || char == '$' || char >= '\u{a0}'
        })
}

impl Context<'_> {
    fn children(&mut self, parent: Parent) -> Option<&mut Vec<NodeId>> {
        match parent {
            Parent::Root => Some(&mut self.root.children),
            Parent::Node(id) => match self.root.nodes.get_mut(id.0)? {
                Node::Element(element) => Some(&mut element.children),
                Node::IfBranch(branch) => Some(&mut branch.children),
                Node::For(node) => Some(&mut node.children),
                _ => None,
            },
        }
    }

    fn add(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.root.nodes.len());
        self.root.nodes.push(node);
        id
    }

    fn is_element(&self, id: NodeId) -> bool {
        matches!(self.root.nodes.get(id.0), Some(Node::Element(_)))
    }

    /// `traverseChildren`.
    fn traverse_children(&mut self, parent: Parent) {
        let frame = self.removals.len();
        self.removals.push(0);
        let mut i = 0;
        while let Some(child) = self
            .children(parent)
            .and_then(|children| children.get(i).copied())
        {
            self.parent = Some(parent);
            self.child_index = i;
            self.frame = frame;
            if let Some(removed) = self.removals.get_mut(frame) {
                *removed = 0;
            }
            self.traverse_node(child);
            let removed = self.removals.get(frame).copied().unwrap_or(0);
            i = (i + 1).saturating_sub(removed);
        }
    }

    /// `traverseNode`, with the five transforms in the order
    /// `template/compile.ts` gives them.
    fn traverse_node(&mut self, id: NodeId) {
        self.current = Some(id);
        self.transform_v_bind_shorthand(id);
        // Each transform is given the node the one before it left.
        let Some(node) = self.current else { return };
        self.transform_if(node);
        let Some(node) = self.current else { return };
        self.transform_for(node);
        let Some(node) = self.current else { return };

        match self.root.nodes.get(node.0) {
            Some(Node::If(if_node)) => {
                let mut i = 0;
                let mut count = if_node.branches.len();
                while i < count {
                    let branch = match self.root.nodes.get(node.0) {
                        Some(Node::If(if_node)) => if_node.branches.get(i).copied(),
                        _ => None,
                    };
                    if let Some(branch) = branch {
                        self.traverse_node(branch);
                    }
                    i += 1;
                    count = match self.root.nodes.get(node.0) {
                        Some(Node::If(if_node)) => if_node.branches.len(),
                        _ => 0,
                    };
                }
            }
            Some(Node::IfBranch(_) | Node::For(_) | Node::Element(_)) => {
                self.traverse_children(Parent::Node(node));
            }
            _ => {}
        }

        // The exit functions, last transform first.
        self.current = Some(node);
        if matches!(
            self.root.nodes.get(node.0),
            Some(Node::Element(_) | Node::For(_) | Node::IfBranch(_))
        ) {
            self.merge_text(Parent::Node(node));
        }
        self.exit_element(node);
    }

    /// `transformVBindShorthand`: `:name` is `:name="name"`.
    fn transform_v_bind_shorthand(&mut self, id: NodeId) {
        let Some(Node::Element(element)) = self.root.nodes.get_mut(id.0) else {
            return;
        };
        for prop in &mut element.props {
            let Prop::Directive(directive) = prop else {
                continue;
            };
            if directive.name != "bind" || directive.exp.is_some() {
                continue;
            }
            let Some(arg) = &directive.arg else { continue };
            if !arg.is_static {
                // Only a plain name can be its own value.
                directive.exp = Some(SimpleExpression {
                    content: String::new(),
                    is_static: true,
                    const_type: ConstantType::CanStringify,
                    loc: arg.loc,
                });
                continue;
            }
            let name = js::camelize(&arg.content);
            if has_valid_first_ident_char(&name) || name.starts_with('-') {
                directive.exp = Some(SimpleExpression {
                    content: name,
                    is_static: false,
                    const_type: ConstantType::NotConstant,
                    loc: arg.loc,
                });
            }
        }
    }

    /// Takes each directive `matches` accepts off the element, in order, and
    /// applies `apply` to it (`createStructuralDirectiveTransform`).
    fn each_structural_directive(
        &mut self,
        id: NodeId,
        matches: fn(&str) -> bool,
        apply: fn(&mut Self, NodeId, Directive),
    ) {
        if !self.is_element(id) {
            return;
        }
        let mut i = 0;
        loop {
            let Some(Node::Element(element)) = self.root.nodes.get_mut(id.0) else {
                return;
            };
            let Some(prop) = element.props.get(i) else {
                return;
            };
            if matches!(prop, Prop::Directive(directive) if matches(&directive.name)) {
                if let Prop::Directive(directive) = element.props.remove(i) {
                    apply(self, id, *directive);
                }
            } else {
                i += 1;
            }
        }
    }

    fn transform_if(&mut self, id: NodeId) {
        self.each_structural_directive(
            id,
            |name| matches!(name, "if" | "else-if" | "else"),
            Self::apply_if,
        );
    }

    fn create_if_branch(&mut self, node: NodeId, directive: Directive) -> Option<NodeId> {
        let loc = *self.root.nodes.get(node.0)?.loc();
        Some(self.add(Node::IfBranch(IfBranch {
            condition: if directive.name == "else" {
                None
            } else {
                directive.exp
            },
            children: vec![node],
            loc,
        })))
    }

    fn apply_if(&mut self, node: NodeId, directive: Directive) {
        if directive.name == "if" {
            let Some(branch) = self.create_if_branch(node, directive) else {
                return;
            };
            let Some(loc) = self.root.nodes.get(node.0).map(|node| *node.loc()) else {
                return;
            };
            let if_node = self.add(Node::If(If {
                branches: vec![branch],
                loc,
            }));
            self.replace_node(if_node);
            return;
        }
        let Some(parent) = self.parent else {
            self.unsupported
                .get_or_insert("a v-else on a node that has no parent");
            return;
        };
        // Backwards from the node: comments and blank text are taken out, and
        // the first thing else has to be the `v-if` chain this joins.
        let mut comments = Vec::new();
        let position = self
            .children(parent)
            .and_then(|children| children.iter().position(|&child| child == node));
        let mut index = position.and_then(|position| position.checked_sub(1));
        loop {
            let sibling = index.and_then(|index| self.children(parent)?.get(index).copied());
            match sibling.and_then(|sibling| self.root.nodes.get(sibling.0)) {
                Some(Node::Comment(_)) => {
                    if let Some(sibling) = sibling {
                        self.remove_sibling(parent, sibling);
                        comments.insert(0, sibling);
                    }
                }
                Some(Node::Text(text)) if js::trim(&text.content).is_empty() => {
                    if let Some(sibling) = sibling {
                        self.remove_sibling(parent, sibling);
                    }
                }
                Some(Node::If(_)) => {
                    let Some(sibling) = sibling else { return };
                    self.remove_current(parent);
                    let Some(branch) = self.create_if_branch(node, directive) else {
                        return;
                    };
                    if let Some(Node::IfBranch(created)) = self.root.nodes.get_mut(branch.0) {
                        comments.push(node);
                        created.children = comments;
                    }
                    if let Some(Node::If(if_node)) = self.root.nodes.get_mut(sibling.0) {
                        if_node.branches.push(branch);
                    }
                    // The branch is traversed from here, since the traversal
                    // has already passed the chain it was added to.
                    self.traverse_node(branch);
                    self.current = None;
                    return;
                }
                // No chain to join: an error, and the node stays where it is.
                _ => return,
            }
            index = index.and_then(|index| index.checked_sub(1));
        }
    }

    fn transform_for(&mut self, id: NodeId) {
        self.each_structural_directive(
            id,
            |name| name == "for",
            |context, node, directive| {
                // With no expression, or one that isn't `x in y`: an error only.
                let Some(parse_result) = directive.exp.and(directive.for_parse_result) else {
                    return;
                };
                let for_node = context.add(Node::For(Box::new(For {
                    parse_result,
                    children: vec![node],
                    loc: directive.loc,
                })));
                context.replace_node(for_node);
            },
        );
    }

    /// `context.replaceNode`.
    fn replace_node(&mut self, node: NodeId) {
        // A development build throws when the node was already removed
        // (`v-else-if` before `v-if` on one element); a production build
        // writes over whatever is at that position.
        let (Some(parent), Some(_)) = (self.parent, self.current) else {
            self.unsupported
                .get_or_insert("a v-if or v-for replaces a node that was already removed");
            return;
        };
        let index = self.child_index;
        match self
            .children(parent)
            .and_then(|children| children.get_mut(index))
        {
            Some(slot) => {
                *slot = node;
                self.current = Some(node);
            }
            None => {
                self.unsupported
                    .get_or_insert("a v-if or v-for replaces a node outside its parent");
            }
        }
    }

    /// `context.removeNode(sibling)`.
    fn remove_sibling(&mut self, parent: Parent, sibling: NodeId) {
        let Some(index) = self
            .children(parent)
            .and_then(|children| children.iter().position(|&child| child == sibling))
        else {
            return;
        };
        if self.child_index > index {
            self.child_index -= 1;
            self.node_removed();
        }
        if let Some(children) = self.children(parent) {
            children.remove(index);
        }
    }

    /// `context.removeNode()`: the current node.
    fn remove_current(&mut self, parent: Parent) {
        if self.current.is_none() {
            // A development build throws here.
            self.unsupported
                .get_or_insert("a v-else removes a node that was already removed");
            return;
        }
        self.current = None;
        self.node_removed();
        let index = self.child_index;
        if let Some(children) = self.children(parent)
            && index < children.len()
        {
            children.remove(index);
        }
    }

    fn node_removed(&mut self) {
        if let Some(removed) = self.removals.get_mut(self.frame) {
            *removed += 1;
        }
    }

    /// `transformElement`'s exit function: the argument rewrites `v-bind`
    /// makes, and the list of components.
    fn exit_element(&mut self, id: NodeId) {
        let Some(Node::Element(element)) = self.root.nodes.get_mut(id.0) else {
            return;
        };
        if element.tag_type == ElementType::Template {
            return;
        }
        for prop in &mut element.props {
            if let Prop::Directive(directive) = prop
                && directive.name == "bind"
            {
                transform_bind(directive);
            }
        }
        if element.tag_type == ElementType::Component
            && !self.root.components.contains(&element.tag)
        {
            let tag = element.tag.clone();
            self.root.components.push(tag);
        }
    }

    /// `transformText`'s exit function: adjacent text and interpolations
    /// become one compound expression.
    fn merge_text(&mut self, parent: Parent) {
        let Some(children) = self.children(parent).map(|children| children.clone()) else {
            return;
        };
        let is_text = |id: &NodeId| {
            matches!(
                self.root.nodes.get(id.0),
                Some(Node::Text(_) | Node::Interpolation(_))
            )
        };
        let mut merged = Vec::with_capacity(children.len());
        let mut compounds = Vec::new();
        let mut i = 0;
        while let Some(&child) = children.get(i) {
            i += 1;
            if !is_text(&child) || !children.get(i).is_some_and(is_text) {
                merged.push(Ok(child));
                continue;
            }
            // A run of two or more.
            let mut parts = vec![CompoundChild::Node(child)];
            while let Some(&next) = children.get(i).filter(|next| is_text(next)) {
                parts.push(CompoundChild::Text(" + "));
                parts.push(CompoundChild::Node(next));
                i += 1;
            }
            merged.push(Err(compounds.len()));
            compounds.push((child, parts));
        }
        if compounds.is_empty() {
            return;
        }
        let mut created = Vec::with_capacity(compounds.len());
        for (first, parts) in compounds {
            let Some(loc) = self.root.nodes.get(first.0).map(|node| *node.loc()) else {
                return;
            };
            created.push(self.add(Node::Compound(Compound {
                children: parts,
                loc,
            })));
        }
        let merged = merged
            .into_iter()
            .filter_map(|child| match child {
                Ok(id) => Some(id),
                Err(index) => created.get(index).copied(),
            })
            .collect();
        if let Some(children) = self.children(parent) {
            *children = merged;
        }
    }
}

/// `transformBind`: what `v-bind` with an argument does to the argument.
fn transform_bind(directive: &mut Directive) {
    // With no argument, `transformElement` never calls it; with an empty
    // expression, it reports an error and leaves the argument alone.
    let Some(arg) = directive.arg.as_mut() else {
        return;
    };
    if directive
        .exp
        .as_ref()
        .is_some_and(|exp| js::trim(&exp.content).is_empty())
    {
        return;
    }
    if !arg.is_static {
        arg.content = if arg.content.is_empty() {
            "\"\"".to_owned()
        } else {
            format!("{} || \"\"", arg.content)
        };
    }
    let has_modifier = |name: &str| {
        directive
            .modifiers
            .iter()
            .any(|modifier| modifier.content == name)
    };
    if has_modifier("camel") {
        arg.content = if arg.is_static {
            js::camelize(&arg.content)
        } else {
            format!("_camelize({})", arg.content)
        };
    }
    for (modifier, prefix) in [("prop", '.'), ("attr", '^')] {
        if has_modifier(modifier) {
            arg.content = if arg.is_static {
                format!("{prefix}{}", arg.content)
            } else {
                format!("`{prefix}${{{}}}`", arg.content)
            };
        }
    }
}
