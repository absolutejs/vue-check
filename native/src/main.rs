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
mod style;
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
        "accesses": context.access_log,
        "unsupported": context.unsupported,
    })
}

/// One recorded `generateTemplate` call (see `native/oracle/dump.ts`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "the recorded options, one to one"
)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TemplateCall {
    compiler: RecordedCompiler,
    component_name: String,
    dot_value_bindings: Vec<String>,
    has_ast: bool,
    has_define_slots: bool,
    imported_components: Vec<String>,
    inherit_attrs: bool,
    is_vapor: bool,
    lang: String,
    props_assign_name: Option<String>,
    reassert_bindings: Vec<String>,
    script_lang: String,
    setup_bindings: Vec<String>,
    setup_consts: Vec<String>,
    setup_refs: Vec<String>,
    slots_assign_name: Option<String>,
    template: String,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "the recorded options, one to one"
)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordedCompiler {
    check_required_fallthrough_attributes: bool,
    check_unknown_components: bool,
    check_unknown_directives: bool,
    check_unknown_events: bool,
    check_unknown_props: bool,
    data_attributes: Vec<String>,
    fallthrough_attributes: bool,
    fallthrough_component_names: Vec<String>,
    html_attributes: Vec<String>,
    infer_component_dollar_el: bool,
    infer_component_dollar_refs: bool,
    infer_template_dollar_attrs: bool,
    infer_template_dollar_el: bool,
    infer_template_dollar_refs: bool,
    infer_template_dollar_slots: bool,
    lib: String,
    strict_v_model: bool,
}

#[derive(Deserialize)]
struct RecordedTemplates {
    file: String,
    #[serde(default)]
    templates: Vec<TemplateCall>,
}

/// Replays one recorded `generateTemplate` call on its template.
fn replay_template(call: &TemplateCall) -> Value {
    use codegen::template::generate::{CompilerFlags, TemplateOptions, generate_template};

    let set = |names: &[String]| {
        names
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>()
    };
    let recorded = &call.compiler;
    let compiler = CompilerFlags {
        lib: recorded.lib.clone(),
        check_unknown_props: recorded.check_unknown_props,
        check_unknown_events: recorded.check_unknown_events,
        check_unknown_directives: recorded.check_unknown_directives,
        check_unknown_components: recorded.check_unknown_components,
        strict_v_model: recorded.strict_v_model,
        infer_template_dollar_slots: recorded.infer_template_dollar_slots,
        infer_template_dollar_attrs: recorded.infer_template_dollar_attrs,
        infer_template_dollar_refs: recorded.infer_template_dollar_refs,
        infer_template_dollar_el: recorded.infer_template_dollar_el,
        infer_component_dollar_el: recorded.infer_component_dollar_el,
        infer_component_dollar_refs: recorded.infer_component_dollar_refs,
        fallthrough_attributes: recorded.fallthrough_attributes,
        check_required_fallthrough_attributes: recorded.check_required_fallthrough_attributes,
        fallthrough_component_names: recorded.fallthrough_component_names.clone(),
        data_attributes: recorded.data_attributes.clone(),
        html_attributes: recorded.html_attributes.clone(),
    };
    let (imported_components, setup_refs, setup_consts) = (
        set(&call.imported_components),
        set(&call.setup_refs),
        set(&call.setup_consts),
    );
    let (setup_bindings, dot_value_bindings, reassert_bindings) = (
        set(&call.setup_bindings),
        set(&call.dot_value_bindings),
        set(&call.reassert_bindings),
    );

    // The template's tree, as the `vue-template-html` plugin makes it.
    let parsed_text = template::compile::with_suffix(&call.template);
    let mut unsupported = None;
    let root = if call.has_ast {
        if matches!(call.lang.as_str(), "html" | "md") {
            let mut parsed = parse(&parsed_text, ParseOptions::template());
            unsupported = parsed.unsupported;
            if parsed.poisons_later_parses {
                unsupported = unsupported.or(Some("the template changes how later files parse"));
            }
            unsupported = unsupported.or(template::transform::transform(&mut parsed.root));
            Some(parsed.root)
        } else {
            unsupported = Some("a template in a language other than HTML");
            None
        }
    } else {
        None
    };
    let options = TemplateOptions {
        template: &call.template,
        parsed: &parsed_text,
        is_html: call.lang == "html",
        is_ts: matches!(call.script_lang.as_str(), "ts" | "tsx"),
        is_vapor: call.is_vapor,
        compiler: &compiler,
        imported_components: &imported_components,
        setup_refs: &setup_refs,
        setup_consts: &setup_consts,
        setup_bindings: &setup_bindings,
        dot_value_bindings: &dot_value_bindings,
        reassert_bindings: &reassert_bindings,
        has_define_slots: call.has_define_slots,
        props_assign_name: call.props_assign_name.as_deref(),
        slots_assign_name: call.slots_assign_name.as_deref(),
        inherit_attrs: call.inherit_attrs,
        component_name: &call.component_name,
    };
    let generated = generate_template(&options, root.as_ref());
    json!({
        "codes": dump_template_codes(&generated.codes, &call.template),
        "generatedTypes": generated.generated_types,
        "contextAccesses": generated.context_access_names,
        "dotValueAccesses": generated.dot_value_accesses,
        "unsupported": unsupported.or(generated.unsupported),
    })
}

/// The template's codes in the shape the oracle records them.
fn dump_template_codes(codes: &[codegen::Code], template: &str) -> Vec<Value> {
    use codegen::{Code, Source, Verification};

    let map = utf16::Utf16Map::new(template);
    let mut tokens: Vec<u32> = Vec::new();
    codes
        .iter()
        .map(|code| match code {
            Code::Text(text) => json!(text),
            Code::Mapped {
                text,
                source,
                offset,
                features,
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
                let source = match source {
                    Source::Template => "template".to_owned(),
                    Source::Script => "script".to_owned(),
                    Source::ScriptSetup => "scriptSetup".to_owned(),
                    Source::Style(index) => format!("style_{index}"),
                    Source::Main => "main".to_owned(),
                };
                json!([text, source, map.offset(*offset), verification, group])
            }
        })
        .collect()
}

fn run_template() -> io::Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let recorded: RecordedTemplates = serde_json::from_str(&line)?;
        let replayed = recorded
            .templates
            .iter()
            .map(replay_template)
            .collect::<Vec<_>>();
        serde_json::to_writer(
            &mut stdout,
            &json!({ "file": recorded.file, "templates": replayed }),
        )?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

/// One file to generate: its text, and the folder of language-core's type
/// declarations as its reference lines name it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateInput {
    file: String,
    text: String,
    /// Absent when the project has no language-core to name.
    types_path: Option<String>,
    /// The version of Vue the code is for (`vueCompilerOptions.target`).
    target: Option<f64>,
}

/// The generated code of one file, in the shape the oracle records it.
fn generate_one(input: &GenerateInput) -> Value {
    use codegen::Verification;

    let name = input.file.rsplit('/').next().unwrap_or(&input.file);
    match codegen::file::generate_file(
        &input.text,
        name,
        input.types_path.as_deref().unwrap_or(""),
        input.target.unwrap_or(99.0),
    ) {
        Ok(generated) => {
            let mappings: Vec<Value> = generated
                .mappings
                .iter()
                .map(|mapping| {
                    let verification = match mapping.verification {
                        Verification::Off => json!(0),
                        Verification::On => json!(1),
                        Verification::NotTs2353OrTs2561 => json!("ts2353"),
                        Verification::NotTs6133 => json!("ts6133"),
                        Verification::Stateful => json!("stateful"),
                    };
                    json!({
                        "generated": mapping.generated_offsets,
                        "generatedLengths": null,
                        "lengths": mapping.lengths,
                        "source": mapping.source_offsets,
                        "verification": verification,
                    })
                })
                .collect();
            json!({
                "file": input.file,
                "generated": {
                    "id": generated.id,
                    "languageId": generated.language_id,
                    "mappings": mappings,
                    "text": generated.text,
                },
            })
        }
        Err(declined) => json!({
            "file": input.file,
            "unsupported": declined.reason,
            "poisonsLaterParses": declined.poisons_later_parses,
        }),
    }
}

fn run_generate() -> io::Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let input: GenerateInput = serde_json::from_str(&line)?;
        serde_json::to_writer(&mut stdout, &generate_one(&input))?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
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
        Some("template") => run_template(),
        Some("generate") => run_generate(),
        _ => {
            eprintln!(
                "usage: vue-check-codegen <parse|interpolate|template|generate> < input.jsonl > output.jsonl"
            );
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
