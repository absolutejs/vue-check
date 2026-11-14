//! vue-check's native codegen: a port of `@vue/language-core` 3.3.14's
//! codegen (see `docs/NATIVE.md`).
//!
//!   vue-check-codegen parse < files.jsonl > parsed.jsonl
//!
//! `parse` reads one `{"file", "text"}` per line and writes, per line, what
//! Vue's template parser makes of the file and of its template, in the shape
//! `native/oracle/dump.ts` writes the original's.

mod dump;
mod js;
mod sfc;
mod template;
mod utf16;

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use serde::Deserialize;
use serde_json::{Value, json};

use template::parser::{ParseOptions, parse};

#[derive(Deserialize)]
struct Input {
    file: String,
    text: String,
}

/// What the parser makes of one file. `unsupported` is set when the port
/// can't stand behind the result, which the caller must not use then.
fn parse_file(input: &Input) -> Value {
    let sfc = sfc::parse_sfc(&input.text);
    let mut unsupported = sfc.unsupported;
    let mut poisons_later_parses = sfc.parsed.poisons_later_parses;
    // The template, as `vue-template-html` compiles an HTML one.
    let template = sfc
        .descriptor
        .template
        .as_ref()
        .filter(|block| matches!(block.lang.as_deref(), None | Some("html" | "md")))
        .map(|block| {
            let source = template::compile::with_suffix(&block.content);
            let mut parsed = parse(&source, ParseOptions::template());
            unsupported = unsupported.or(parsed.unsupported);
            poisons_later_parses |= parsed.poisons_later_parses;
            unsupported = unsupported.or(template::transform::transform(&mut parsed.root));
            dump::template(&source, &parsed)
        });
    json!({
        "file": input.file,
        "sfc": dump::sfc(&input.text, &sfc),
        "template": template,
        "unsupported": unsupported,
        "poisonsLaterParses": poisons_later_parses,
    })
}

fn run_parse() -> io::Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let input: Input = serde_json::from_str(&line)?;
        serde_json::to_writer(&mut stdout, &parse_file(&input))?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() != Some("parse") {
        eprintln!("usage: vue-check-codegen parse < files.jsonl > parsed.jsonl");
        return ExitCode::from(2);
    }
    let result = run_parse();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vue-check-codegen: {error}");
            ExitCode::FAILURE
        }
    }
}
