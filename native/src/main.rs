//! vue-check's native codegen: a port of `@vue/language-core` 3.3.14's
//! codegen (see `docs/NATIVE.md`).
//!
//!   vue-check-codegen parse < files.jsonl > parsed.jsonl
//!   vue-check-codegen interpolate < oracle.jsonl > interpolations.jsonl
//!
//! `parse` reads one `{"file", "text"}` per line and writes, per line, what
//! the parser, the transforms and the script analysis make of the file, in
//! the shape `native/oracle/dump.ts` writes the original's. `interpolate`
//! replays the template expressions the oracle recorded.

mod codegen;
mod dump;
mod js;
mod script;
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
    // Each <script> block's analysis. A block in a language that isn't
    // JavaScript or TypeScript has none.
    let options = script::ScriptOptions::default();
    let mut analyze = |block: Option<&sfc::Block>, setup: bool| {
        let block = block?;
        let lang = script::ScriptLang::from_lang(block.lang.as_deref().unwrap_or("js"))?;
        let dumped = if setup {
            script::parse_script_setup_ranges(&block.content, lang, &options)
                .map(|ranges| dump::script::setup_ranges(&block.content, &ranges))
        } else {
            script::parse_script_ranges(&block.content, lang, &options)
                .map(|ranges| dump::script::ranges(&block.content, &ranges))
        };
        match dumped {
            Ok(dumped) => Some(dumped),
            Err(reason) => {
                unsupported = unsupported.or(Some(reason));
                None
            }
        }
    };
    let script_ranges = analyze(sfc.descriptor.script.as_ref(), false);
    let script_setup_ranges = analyze(sfc.descriptor.script_setup.as_ref(), true);
    json!({
        "file": input.file,
        "sfc": dump::sfc(&input.text, &sfc),
        "template": template,
        "script": script_ranges,
        "scriptSetup": script_setup_ranges,
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

/// One recorded `generateInterpolation` call (see `native/oracle/dump.ts`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InterpolationCall {
    code: String,
    dollar_vars: Vec<String>,
    dot_value_bindings: Vec<String>,
    in_narrowing: bool,
    lib: String,
    prefix: String,
    reassert_bindings: Vec<String>,
    scopes: Vec<Vec<String>>,
    script_lang: String,
    setup_bindings: Vec<String>,
    setup_consts: Vec<String>,
    setup_refs: Vec<String>,
    source: String,
    start: usize,
    suffix: String,
    verification: Value,
}

#[derive(Deserialize)]
struct RecordedFile {
    file: String,
    #[serde(default)]
    interpolations: Vec<InterpolationCall>,
}

/// Pieces of code in the shape the oracle records them: text, or
/// `[text, source, offset, verification, combine group]`.
fn dump_codes(codes: &[codegen::Code], call: &InterpolationCall) -> Vec<Value> {
    use codegen::{Code, Verification};

    let map = utf16::Utf16Map::new(&call.code);
    let mut tokens: Vec<u32> = Vec::new();
    codes
        .iter()
        .map(|code| match code {
            Code::Text(text) => json!(text),
            Code::Mapped {
                text,
                offset,
                features,
                ..
            } => {
                let group = features.combine.map(|token| {
                    tokens
                        .iter()
                        .position(|known| *known == token)
                        .unwrap_or_else(|| {
                            tokens.push(token);
                            tokens.len() - 1
                        })
                });
                let verification = match features.verification {
                    Verification::Off => json!(0),
                    Verification::On => json!(1),
                    Verification::NotTs2353OrTs2561 => json!("ts2353"),
                    Verification::NotTs6133 => json!("ts6133"),
                    Verification::Stateful => json!("stateful"),
                };
                json!([
                    text,
                    call.source,
                    call.start + map.offset(*offset),
                    verification,
                    group
                ])
            }
        })
        .collect()
}

/// Replays one recorded call. Its offsets are relative to the expression's
/// start, so the expression is generated from 0 and moved there.
fn replay_interpolation(call: &InterpolationCall) -> Value {
    use codegen::template::context::TemplateContext;
    use codegen::template::interpolation::{IdentifierOptions, generate_interpolation};
    use codegen::{Features, Source, Verification};

    let set = |names: &[String]| {
        names
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
    };
    let (setup_refs, setup_consts, setup_bindings) = (
        set(&call.setup_refs),
        set(&call.setup_consts),
        set(&call.setup_bindings),
    );
    let (dot_value_bindings, reassert_bindings) =
        (set(&call.dot_value_bindings), set(&call.reassert_bindings));
    let options = IdentifierOptions {
        setup_refs: &setup_refs,
        setup_consts: &setup_consts,
        setup_bindings: &setup_bindings,
        dot_value_bindings: &dot_value_bindings,
        reassert_bindings: &reassert_bindings,
        lib: &call.lib,
        is_ts: matches!(call.script_lang.as_str(), "ts" | "tsx"),
    };
    let mut context = TemplateContext::default();
    context.scopes = call.scopes.iter().map(|scope| set(scope)).collect();
    context.dollar_vars = set(&call.dollar_vars);
    let verification = match &call.verification {
        Value::Number(number) if number.as_u64() == Some(1) => Verification::On,
        Value::String(rule) if rule == "ts2353" => Verification::NotTs2353OrTs2561,
        Value::String(rule) if rule == "ts6133" => Verification::NotTs6133,
        Value::String(_) => Verification::Stateful,
        _ => Verification::Off,
    };
    let source = match call.source.as_str() {
        "template" => Source::Template,
        "script" => Source::Script,
        "scriptSetup" => Source::ScriptSetup,
        other => Source::Style(
            other
                .strip_prefix("style_")
                .and_then(|index| index.parse().ok())
                .unwrap_or(0),
        ),
    };
    let mut codes = Vec::new();
    generate_interpolation(
        &options,
        &mut context,
        source,
        Features::new(verification),
        &call.code,
        0,
        &call.prefix,
        &call.suffix,
        call.in_narrowing,
        &mut codes,
    );
    let dumped = dump_codes(&codes, call);
    json!({
        "codes": dumped,
        "accesses": context.accesses.iter().map(|access| access.name.as_str()).collect::<Vec<_>>(),
        "unsupported": context.unsupported,
    })
}

fn run_interpolate() -> io::Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let recorded: RecordedFile = serde_json::from_str(&line)?;
        let replayed = recorded
            .interpolations
            .iter()
            .map(replay_interpolation)
            .collect::<Vec<_>>();
        serde_json::to_writer(
            &mut stdout,
            &json!({ "file": recorded.file, "interpolations": replayed }),
        )?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("parse") => run_parse(),
        Some("interpolate") => run_interpolate(),
        _ => {
            eprintln!("usage: vue-check-codegen <parse|interpolate> < input.jsonl > output.jsonl");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vue-check-codegen: {error}");
            ExitCode::FAILURE
        }
    }
}
