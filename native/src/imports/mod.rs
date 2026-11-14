//! The files a source file imports, as TypeScript's `preProcessFile(text,
//! true, true)` lists them (`services/preProcess.ts`): its `importedFiles`,
//! in order. vue-check follows them to find the Vue files a project reaches.
//!
//! `preProcessFile` has no parser. It walks the scanner's tokens and
//! recognises imports by the tokens around a string, so it is wrong the same
//! way every time (a regular expression's text is scanned as code), and this
//! is a port of exactly that.

mod scanner;
mod tables;

use scanner::{Keyword, Scanner, Token, Value};

struct Reader<'a> {
    scanner: Scanner<'a>,
    last_token: Option<Token>,
    current_token: Option<Token>,
    brace_nesting: i64,
    external_module: bool,
    imported: Vec<String>,
    /// `ambientExternalModules`: each name, and how deep in braces it was.
    ambient: Vec<(String, i64)>,
    /// A name that can't be given as text (see [`Value::LoneSurrogate`]).
    unrepresentable: bool,
}

const fn is_keyword(token: Token) -> bool {
    matches!(token, Token::Keyword(_))
}

const fn is_string_or_template(token: Token) -> bool {
    matches!(
        token,
        Token::StringLiteral | Token::NoSubstitutionTemplateLiteral
    )
}

impl Reader<'_> {
    fn next_token(&mut self) -> Token {
        self.last_token = self.current_token;
        let token = self.scanner.scan();
        self.current_token = Some(token);
        match token {
            Token::OpenBrace => self.brace_nesting += 1,
            Token::CloseBrace => self.brace_nesting -= 1,
            _ => {}
        }
        token
    }

    /// `getFileReference().fileName`.
    fn file_name(&mut self) -> String {
        match self.scanner.value() {
            Value::Text(text) => text.clone().into_owned(),
            Value::None => String::new(),
            Value::LoneSurrogate => {
                self.unrepresentable = true;
                String::new()
            }
        }
    }

    fn record_ambient_external_module(&mut self) {
        let name = self.file_name();
        self.ambient.push((name, self.brace_nesting));
    }

    fn record_module_name(&mut self) {
        let name = self.file_name();
        self.imported.push(name);
        self.mark_as_external_module_if_top_level();
    }

    fn mark_as_external_module_if_top_level(&mut self) {
        if self.brace_nesting == 0 {
            self.external_module = true;
        }
    }

    fn try_consume_declare(&mut self) -> bool {
        if self.scanner.token() != Token::Keyword(Keyword::Declare) {
            return false;
        }
        if self.next_token() == Token::Keyword(Keyword::Module)
            && self.next_token() == Token::StringLiteral
        {
            self.record_ambient_external_module();
        }
        true
    }

    /// `from "name"`, when that is what follows `token`.
    fn record_from(&mut self, token: Token) {
        if token == Token::Keyword(Keyword::From) && self.next_token() == Token::StringLiteral {
            self.record_module_name();
        }
    }

    /// The rest of a `{ … }` list, and the `from` after it.
    fn consume_braces_and_from(&mut self) {
        let mut token = self.next_token();
        while token != Token::CloseBrace && token != Token::EndOfFile {
            token = self.next_token();
        }
        if token == Token::CloseBrace {
            let token = self.next_token();
            self.record_from(token);
        }
    }

    fn try_consume_import(&mut self) -> bool {
        if self.last_token == Some(Token::Dot) {
            return false;
        }
        if self.scanner.token() != Token::Keyword(Keyword::Import) {
            return false;
        }
        let mut token = self.next_token();
        if token == Token::OpenParen {
            token = self.next_token();
            if is_string_or_template(token) {
                self.record_module_name();
            }
            return true;
        }
        if token == Token::StringLiteral {
            self.record_module_name();
            return true;
        }
        if token == Token::Keyword(Keyword::Type) {
            let ahead = self.scanner.look_ahead();
            if ahead != Token::Keyword(Keyword::From)
                && (matches!(ahead, Token::Asterisk | Token::OpenBrace | Token::Identifier)
                    || is_keyword(ahead))
            {
                token = self.next_token();
            }
        }
        if token == Token::Identifier || is_keyword(token) {
            token = self.next_token();
            if token == Token::Keyword(Keyword::From) {
                token = self.next_token();
                if token == Token::StringLiteral {
                    self.record_module_name();
                    return true;
                }
            } else if token == Token::Equals {
                if self.try_consume_require_call(true, false) {
                    return true;
                }
            } else if token == Token::Comma {
                token = self.next_token();
            } else {
                return true;
            }
        }
        if token == Token::OpenBrace {
            self.consume_braces_and_from();
        } else if token == Token::Asterisk
            && self.next_token() == Token::Keyword(Keyword::As)
        {
            let token = self.next_token();
            if token == Token::Identifier || is_keyword(token) {
                let token = self.next_token();
                self.record_from(token);
            }
        }
        true
    }

    fn try_consume_export(&mut self) -> bool {
        if self.scanner.token() != Token::Keyword(Keyword::Export) {
            return false;
        }
        self.mark_as_external_module_if_top_level();
        let mut token = self.next_token();
        if token == Token::Keyword(Keyword::Type)
            && matches!(self.scanner.look_ahead(), Token::Asterisk | Token::OpenBrace)
        {
            token = self.next_token();
        }
        if token == Token::OpenBrace {
            self.consume_braces_and_from();
        } else if token == Token::Asterisk {
            let token = self.next_token();
            self.record_from(token);
        } else if token == Token::Keyword(Keyword::Import) {
            token = self.next_token();
            if token == Token::Keyword(Keyword::Type) {
                let ahead = self.scanner.look_ahead();
                if ahead == Token::Identifier || is_keyword(ahead) {
                    token = self.next_token();
                }
            }
            if (token == Token::Identifier || is_keyword(token))
                && self.next_token() == Token::Equals
                && self.try_consume_require_call(true, false)
            {
                return true;
            }
        }
        true
    }

    fn try_consume_require_call(&mut self, skip_current_token: bool, allow_templates: bool) -> bool {
        let token = if skip_current_token {
            self.next_token()
        } else {
            self.scanner.token()
        };
        if token != Token::Keyword(Keyword::Require) {
            return false;
        }
        if self.next_token() == Token::OpenParen {
            let token = self.next_token();
            if token == Token::StringLiteral
                || (allow_templates && token == Token::NoSubstitutionTemplateLiteral)
            {
                self.record_module_name();
            }
        }
        true
    }

    fn try_consume_define(&mut self) -> bool {
        let is_define = self.scanner.token() == Token::Identifier
            && matches!(self.scanner.value(), Value::Text(text) if text == "define");
        if !is_define {
            return false;
        }
        if self.next_token() != Token::OpenParen {
            return true;
        }
        let mut token = self.next_token();
        if is_string_or_template(token) {
            if self.next_token() != Token::Comma {
                return true;
            }
            token = self.next_token();
        }
        if token != Token::OpenBracket {
            return true;
        }
        token = self.next_token();
        while token != Token::CloseBracket && token != Token::EndOfFile {
            if is_string_or_template(token) {
                self.record_module_name();
            }
            token = self.next_token();
        }
        true
    }

    /// A template with substitutions: its tokens are walked to its end, so
    /// that the text between substitutions isn't read as code.
    fn consume_template(&mut self) {
        let mut stack = vec![Token::TemplateHead];
        while !stack.is_empty() {
            match self.scanner.scan() {
                Token::EndOfFile => break,
                Token::Keyword(Keyword::Import) => {
                    self.try_consume_import();
                }
                Token::TemplateHead => stack.push(Token::TemplateHead),
                Token::OpenBrace => stack.push(Token::OpenBrace),
                Token::CloseBrace => {
                    if stack.last() == Some(&Token::TemplateHead) {
                        if self.scanner.rescan_template_token() == Token::TemplateTail {
                            stack.pop();
                        }
                    } else {
                        stack.pop();
                    }
                }
                _ => {}
            }
        }
        self.next_token();
    }

    /// `processImports`.
    fn process_imports(&mut self) {
        self.next_token();
        while self.scanner.token() != Token::EndOfFile {
            if self.scanner.token() == Token::TemplateHead {
                self.consume_template();
            }
            if !(self.try_consume_declare()
                || self.try_consume_import()
                || self.try_consume_export()
                || self.try_consume_require_call(false, true)
                || self.try_consume_define())
            {
                self.next_token();
            }
        }
    }
}

/// `preProcessFile(text, true, true).importedFiles`, as their names. `None`
/// when a name can't be given as text.
pub fn imported_files(text: &str) -> Option<Vec<String>> {
    let mut reader = Reader {
        scanner: Scanner::new(text),
        last_token: None,
        current_token: None,
        brace_nesting: 0,
        external_module: false,
        imported: Vec::new(),
        ambient: Vec::new(),
        unrepresentable: false,
    };
    reader.process_imports();
    let Reader {
        mut imported,
        ambient,
        external_module,
        unrepresentable,
        ..
    } = reader;
    // In a module, `declare module "name"` augments that module: an import.
    // In a script, one at the top level declares it instead.
    for (name, depth) in ambient {
        if external_module || depth != 0 {
            imported.push(name);
        }
    }
    (!unrepresentable).then_some(imported)
}

#[cfg(test)]
mod tests {
    use super::imported_files;

    fn names(text: &str) -> Vec<String> {
        imported_files(text).unwrap_or_default()
    }

    #[test]
    fn imports_and_exports() {
        assert_eq!(
            names("import a from './a'; import { b } from \"./b\"; import * as c from './c'; import './d'"),
            ["./a", "./b", "./c", "./d"]
        );
        assert_eq!(
            names("export { a } from './a'; export * from './b'; import type { T } from './t'"),
            ["./a", "./b", "./t"]
        );
        assert_eq!(names("const a = import('./a'); const b = require('./b')"), ["./a", "./b"]);
        assert_eq!(names("import a = require('./a')"), ["./a"]);
    }

    #[test]
    fn strings_are_cooked_and_templates_are_walked() {
        assert_eq!(names("import a from './\\x61\\u0062\\u{63}'"), ["./abc"]);
        assert_eq!(names("const a = `${import('./a')} import('./no')`; import('./b')"), ["./a", "./b"]);
        assert_eq!(names("// import './no'\n/* import './no' */ import './yes'"), ["./yes"]);
    }

    #[test]
    fn a_property_named_import_is_not_one() {
        assert_eq!(names("a.import('./no'); import('./yes')"), ["./yes"]);
    }
}
