//! References to the classes of `<style scoped>`: a port of
//! `codegen/template/styleScopedClasses.ts`. They are comments for an
//! editor, which the generated code has all the same.

use super::analysis::class_names;
use super::{COMPLETION, Generator, NAVIGATION_AND_COMPLETION};
use crate::codegen::{END_OF_LINE, Source, generate_escaped};
use crate::template::ast::{Element, Prop};

impl Generator<'_> {
    /// `generateStyleScopedClassReferences`.
    pub(super) fn style_scoped_class_references(&mut self, node: &Element) {
        for prop in &node.props {
            match prop {
                // `class="a b"`
                Prop::Attribute(attribute) if attribute.name == "class" => {
                    let Some(value) = &attribute.value else {
                        continue;
                    };
                    let (text, start) = self.attribute_value(value);
                    self.class_names_of(text, start);
                }
                // `:class="..."`: the names in strings, arrays and objects.
                Prop::Directive(directive) => {
                    let (Some(arg), Some(exp)) = (&directive.arg, &directive.exp) else {
                        continue;
                    };
                    if arg.content != "class" {
                        continue;
                    }
                    let content = format!("({})", exp.content);
                    let start_offset = exp.loc.start.offset.saturating_sub(1);
                    let Some(found) = class_names(&content) else {
                        self.ctx.give_up("a :class expression has a syntax error");
                        continue;
                    };
                    if found.has_escape {
                        self.ctx
                            .give_up("a :class expression has a string with an escape in it");
                    }
                    for (name, start) in &found.keys {
                        self.style_scoped_class_reference(name, start + start_offset);
                    }
                    for (text, start) in &found.literals {
                        self.class_names_of(text, start + start_offset);
                    }
                }
                Prop::Attribute(_) => {}
            }
        }
    }

    /// `forEachClassName`: the names between single spaces.
    fn class_names_of(&mut self, content: &str, start: usize) {
        let mut offset = 0;
        for class_name in content.split(' ') {
            self.style_scoped_class_reference(class_name, start + offset);
            offset += class_name.len() + 1;
        }
    }

    /// `generateStyleScopedClassReference`.
    fn style_scoped_class_reference(&mut self, class_name: &str, offset: usize) {
        if class_name.is_empty() {
            self.text("/** @type {__VLS_StyleScopedClasses['");
            self.mapped("", offset, COMPLETION);
            self.text(format!("']}} */{END_OF_LINE}"));
            return;
        }
        self.text("/** @type {__VLS_StyleScopedClasses[");
        let boundary = self.boundary(offset, offset + class_name.len(), NAVIGATION_AND_COMPLETION);
        self.text("'");
        let token = self.ctx.token();
        generate_escaped(
            &mut self.out,
            token,
            class_name,
            Source::Template,
            offset,
            boundary.features,
        );
        self.text("'");
        self.out.push(boundary.end());
        self.text(format!("]}} */{END_OF_LINE}"));
    }
}
