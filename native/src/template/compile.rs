//! How the `vue-template-html` plugin hands a template to the compiler
//! (`plugins/vue-template-html.ts`, 3.3.14).

use std::borrow::Cow;

/// A template that ends inside an open tag (`<div cla`) gets the `>` the tag
/// is missing, so the element still exists while it is being typed
/// (vuejs/language-tools#4583). The original tests `/(?<=<[^>/]+)$/`: a `<`
/// with at least one character after it, and no `>` or `/` up to the end.
pub fn with_suffix(template: &str) -> Cow<'_, str> {
    let tail = template
        .rfind(['>', '/'])
        .and_then(|last| template.get(last + 1..))
        .unwrap_or(template);
    match tail.find('<') {
        Some(open) if open + 1 < tail.len() => Cow::Owned(format!("{template}>")),
        _ => Cow::Borrowed(template),
    }
}

#[cfg(test)]
mod tests {
    use super::with_suffix;

    #[test]
    fn closes_an_open_tag() {
        assert_eq!(with_suffix("<div cla"), "<div cla>");
        assert_eq!(with_suffix("<a></a><b x=\"1\""), "<a></a><b x=\"1\">");
        assert_eq!(with_suffix("<div>"), "<div>");
        assert_eq!(with_suffix("<div><"), "<div><");
        assert_eq!(with_suffix("<br /"), "<br /");
        assert_eq!(with_suffix("text"), "text");
        assert_eq!(with_suffix("a < b"), "a < b>");
    }
}
