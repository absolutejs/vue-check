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
mod imports;
mod js;
mod script;
mod sfc;
mod style;
mod template;
mod utf16;

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    let options = script::ScriptOptions {
        core: core_argument(),
        ..script::ScriptOptions::default()
    };
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
    use codegen::Code;

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
                let verification = verification_name(features.verification);
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
        core: core_argument(),
    };
    let mut context = TemplateContext::default();
    context.core = core_argument();
    context.scopes = call.scopes.iter().map(|scope| set(scope)).collect();
    context.dollar_vars = set(&call.dollar_vars);
    let verification = match &call.verification {
        Value::Number(number) if number.as_u64() == Some(1) => Verification::On,
        Value::String(rule) if rule == "ts2353" => Verification::NotTs2353OrTs2561,
        Value::String(rule) if rule == "ts6133" => Verification::NotTs6133,
        Value::String(rule) if rule == "ts2339" => Verification::NotTs2339OrTs2551,
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
    /// `experimentalModelPropName`, as the gate gives it (`ModelPropNames`).
    model_prop_names: Option<ModelPropNames>,
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
        core: core_argument(),
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
        model_prop_names: recorded.model_prop_names.as_ref().map_or_else(
            || CompilerFlags::default().model_prop_names,
            model_prop_names,
        ),
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
        "codes": dump_template_codes(
            &generated.codes,
            &call.template,
            root.as_ref().map_or(&[], |root| root.decoded.as_slice()),
        ),
        "generatedTypes": generated.generated_types,
        "contextAccesses": generated.context_access_names,
        "dotValueAccesses": generated.dot_value_accesses,
        "unsupported": unsupported.or(generated.unsupported),
    })
}

/// The template's codes in the shape the oracle records them.
fn dump_template_codes(
    codes: &[codegen::Code],
    template: &str,
    decoded: &[template::ast::DecodedValue],
) -> Vec<Value> {
    use codegen::{Code, Source};

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
                let verification = verification_name(features.verification);
                let source = match source {
                    Source::Template => "template".to_owned(),
                    Source::Script => "script".to_owned(),
                    Source::ScriptSetup => "scriptSetup".to_owned(),
                    Source::Style(index) => format!("style_{index}"),
                    Source::Main => "main".to_owned(),
                };
                json!([
                    text,
                    source,
                    map.offset_in(0, *offset, decoded),
                    verification,
                    group
                ])
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
    /// The compiler options the project sets, of those the port reads.
    #[serde(default)]
    options: GenerateOptions,
    /// The version of `@vue/language-core` the project runs; the latest
    /// when not given.
    core: Option<String>,
}

/// `vueCompilerOptions`, as resolved for the project: each option the port
/// reads, absent when the project leaves it at its default.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerateOptions {
    strict_v_model: Option<bool>,
    check_unknown_props: Option<bool>,
    check_unknown_events: Option<bool>,
    check_unknown_directives: Option<bool>,
    check_unknown_components: Option<bool>,
    infer_component_dollar_el: Option<bool>,
    infer_component_dollar_refs: Option<bool>,
    infer_template_dollar_attrs: Option<bool>,
    infer_template_dollar_el: Option<bool>,
    infer_template_dollar_refs: Option<bool>,
    infer_template_dollar_slots: Option<bool>,
    fallthrough_attributes: Option<bool>,
    check_required_fallthrough_attributes: Option<bool>,
    jsx_slots: Option<bool>,
    fallthrough_component_names: Option<Vec<String>>,
    data_attributes: Option<Vec<String>>,
    html_attributes: Option<Vec<String>>,
    lib: Option<String>,
    /// Two strings, or none.
    options_wrapper: Option<Vec<String>>,
    macros: Option<Names>,
    composables: Option<Names>,
    resolve_style_class_names: Option<StyleClassNames>,
    strict_css_modules: Option<bool>,
    resolve_style_imports: Option<bool>,
    experimental_model_prop_name: Option<ModelPropNames>,
}

/// `experimentalModelPropName` in the original's order: each prop, with each
/// tag and when it takes the prop: `true`, the attribute lists that ask for
/// it, or `false` for a value the original reads as neither.
type ModelPropNames = Vec<(String, Vec<(String, ModelTagInput)>)>;

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelTagInput {
    Flag(bool),
    WithAttributes(Vec<Vec<(String, String)>>),
}

fn model_prop_names(
    given: &ModelPropNames,
) -> Vec<(String, Vec<(String, codegen::template::generate::ModelTag)>)> {
    use codegen::template::generate::ModelTag;

    given
        .iter()
        .map(|(name, tags)| {
            let tags = tags
                .iter()
                .map(|(tag, when)| {
                    let when = match when {
                        ModelTagInput::Flag(true) => ModelTag::Always,
                        ModelTagInput::Flag(false) => ModelTag::Never,
                        ModelTagInput::WithAttributes(lists) => {
                            ModelTag::WithAttributes(lists.clone())
                        }
                    };
                    (tag.clone(), when)
                })
                .collect();
            (name.clone(), tags)
        })
        .collect()
}

/// A list of names each macro or composable goes by, as the project has it.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Names {
    define_props: Option<Vec<String>>,
    define_slots: Option<Vec<String>>,
    define_emits: Option<Vec<String>>,
    define_expose: Option<Vec<String>>,
    define_model: Option<Vec<String>>,
    define_options: Option<Vec<String>>,
    with_defaults: Option<Vec<String>>,
    use_attrs: Option<Vec<String>>,
    use_css_module: Option<Vec<String>>,
    use_slots: Option<Vec<String>>,
    use_template_ref: Option<Vec<String>>,
}

/// `resolveStyleClassNames`: `true`, `false` or `"scoped"`.
#[derive(Deserialize)]
#[serde(untagged)]
enum StyleClassNames {
    All(bool),
    Scoped(Scoped),
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Scoped {
    Scoped,
}

impl GenerateOptions {
    fn file_options(&self, core: codegen::CoreVersion) -> codegen::file::FileOptions {
        use codegen::style::ResolveClassNames;

        let mut options = codegen::file::FileOptions::default();
        options.compiler.core = core;
        options.analysis.core = core;
        let set = |flag: &mut bool, value: Option<bool>| {
            if let Some(value) = value {
                *flag = value;
            }
        };
        let flags = &mut options.compiler;
        set(&mut flags.strict_v_model, self.strict_v_model);
        set(&mut flags.check_unknown_props, self.check_unknown_props);
        set(&mut flags.check_unknown_events, self.check_unknown_events);
        set(&mut flags.check_unknown_directives, self.check_unknown_directives);
        set(&mut flags.check_unknown_components, self.check_unknown_components);
        set(&mut flags.infer_component_dollar_el, self.infer_component_dollar_el);
        set(&mut flags.infer_component_dollar_refs, self.infer_component_dollar_refs);
        set(&mut flags.infer_template_dollar_attrs, self.infer_template_dollar_attrs);
        set(&mut flags.infer_template_dollar_el, self.infer_template_dollar_el);
        set(&mut flags.infer_template_dollar_refs, self.infer_template_dollar_refs);
        set(&mut flags.infer_template_dollar_slots, self.infer_template_dollar_slots);
        set(&mut flags.fallthrough_attributes, self.fallthrough_attributes);
        set(
            &mut flags.check_required_fallthrough_attributes,
            self.check_required_fallthrough_attributes,
        );
        let names = |list: &mut Vec<String>, value: &Option<Vec<String>>| {
            if let Some(value) = value {
                list.clone_from(value);
            }
        };
        names(&mut flags.fallthrough_component_names, &self.fallthrough_component_names);
        names(&mut flags.data_attributes, &self.data_attributes);
        names(&mut flags.html_attributes, &self.html_attributes);
        if let Some(lib) = &self.lib {
            flags.lib.clone_from(lib);
        }
        if let Some(given) = &self.experimental_model_prop_name {
            flags.model_prop_names = model_prop_names(given);
        }
        let analysis = &mut options.analysis;
        for given in [&self.macros, &self.composables].into_iter().flatten() {
            names(&mut analysis.define_props, &given.define_props);
            names(&mut analysis.define_slots, &given.define_slots);
            names(&mut analysis.define_emits, &given.define_emits);
            names(&mut analysis.define_expose, &given.define_expose);
            names(&mut analysis.define_model, &given.define_model);
            names(&mut analysis.define_options, &given.define_options);
            names(&mut analysis.with_defaults, &given.with_defaults);
            names(&mut analysis.use_attrs, &given.use_attrs);
            names(&mut analysis.use_css_module, &given.use_css_module);
            names(&mut analysis.use_slots, &given.use_slots);
            names(&mut analysis.use_template_ref, &given.use_template_ref);
        }
        set(&mut options.jsx_slots, self.jsx_slots);
        set(&mut options.strict_css_modules, self.strict_css_modules);
        set(&mut options.resolve_style_imports, self.resolve_style_imports);
        match &self.resolve_style_class_names {
            Some(StyleClassNames::All(true)) => options.resolve_class_names = ResolveClassNames::All,
            Some(StyleClassNames::All(false)) => {
                options.resolve_class_names = ResolveClassNames::None;
            }
            Some(StyleClassNames::Scoped(Scoped::Scoped)) => {
                options.resolve_class_names = ResolveClassNames::Scoped;
            }
            None => {}
        }
        if let Some(wrapper) = &self.options_wrapper {
            options.options_wrapper = match wrapper.as_slice() {
                [left, right] => Some((left.clone(), right.clone())),
                _ => None,
            };
        }
        options
    }
}

/// The generated code of one file, in the shape the oracle records it.
fn generate_one(input: &GenerateInput) -> Value {
    let name = input.file.rsplit('/').next().unwrap_or(&input.file);
    let core = match input.core.as_deref() {
        None => codegen::CoreVersion::default(),
        Some(version) => match codegen::CoreVersion::parse(version) {
            Some(core) => core,
            None => {
                return json!({
                    "file": input.file,
                    "unsupported": "a version of @vue/language-core the port doesn't reproduce",
                    "poisonsLaterParses": false,
                });
            }
        },
    };
    match codegen::file::generate_file(
        &input.text,
        name,
        input.types_path.as_deref().unwrap_or(""),
        input.target.unwrap_or(99.0),
        &input.options.file_options(core),
    ) {
        Ok(generated) => {
            let mappings: Vec<Value> = generated
                .mappings
                .iter()
                .map(|mapping| {
                    let verification = verification_name(mapping.verification);
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
                // What the generated code imports; absent when the port
                // can't say (see `imports`).
                "imports": imports::imported_files(&generated.text),
            })
        }
        Err(declined) => json!({
            "file": input.file,
            "unsupported": declined.reason,
            "poisonsLaterParses": declined.poisons_later_parses,
        }),
    }
}

/// One line of output for each line of input, in the same order, with the
/// lines shared out over the machine's cores: each is independent of the rest.
fn run_in_parallel(one: impl Fn(&str) -> io::Result<String> + Sync) -> io::Result<()> {
    let lines: Vec<String> = io::stdin()
        .lock()
        .lines()
        .filter(|line| line.as_ref().map_or(true, |line| !line.is_empty()))
        .collect::<io::Result<_>>()?;
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(lines.len())
        .max(1);
    let next = AtomicUsize::new(0);
    let work = || -> io::Result<Vec<(usize, String)>> {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            let Some(line) = lines.get(index) else {
                return Ok(done);
            };
            done.push((index, one(line)?));
        }
    };
    let mut outputs: Vec<(usize, String)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(work)).collect();
        let mut outputs = Vec::with_capacity(lines.len());
        for handle in handles {
            let done = handle
                .join()
                .map_err(|_| io::Error::other("a worker thread panicked"))??;
            outputs.extend(done);
        }
        Ok::<_, io::Error>(outputs)
    })?;
    outputs.sort_unstable_by_key(|(index, _)| *index);
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for (_, output) in &outputs {
        stdout.write_all(output.as_bytes())?;
        stdout.write_all(b"\n")?;
    }
    stdout.flush()
}

fn run_generate() -> io::Result<()> {
    run_in_parallel(|line| {
        let input: GenerateInput = serde_json::from_str(line)?;
        Ok(serde_json::to_string(&generate_one(&input))?)
    })
}

/// A file to read the imports of: its text, or where it is.
#[derive(Deserialize)]
struct ImportsInput {
    file: String,
    text: Option<String>,
}

/// The imports of each file, or why the port leaves the file alone.
fn run_imports() -> io::Result<()> {
    run_in_parallel(|line| {
        let input: ImportsInput = serde_json::from_str(line)?;
        let read = match input.text {
            Some(text) => Ok(text),
            None => std::fs::read(&input.file)
                .map_err(|error| error.to_string())
                .and_then(|bytes| String::from_utf8(bytes).map_err(|_| "it isn't UTF-8".to_owned())),
        };
        let text = read.as_deref().map_err(Clone::clone);
        let output = match text.map(imports::imported_files) {
            Ok(Some(imports)) => json!({ "file": input.file, "imports": imports }),
            Ok(None) => json!({
                "file": input.file,
                "unsupported": "an import's name has half a surrogate pair",
            }),
            Err(reason) => json!({ "file": input.file, "unsupported": reason }),
        };
        Ok(serde_json::to_string(&output)?)
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

/// A piece of code's rule, as the oracle names it: 0 (nothing is reported),
/// 1 (everything is), or the rule.
fn verification_name(verification: codegen::Verification) -> Value {
    use codegen::Verification;

    match verification {
        Verification::Off => json!(0),
        Verification::On => json!(1),
        Verification::NotTs2353OrTs2561 => json!("ts2353"),
        Verification::NotTs6133 => json!("ts6133"),
        Verification::NotTs2339OrTs2551 => json!("ts2339"),
        Verification::Stateful => json!("stateful"),
    }
}

/// `--core=3.3.13`: the version of language-core the oracle recorded, for
/// the commands that replay what it recorded. The latest when not given.
fn core_argument() -> codegen::CoreVersion {
    std::env::args()
        .find_map(|argument| {
            argument
                .strip_prefix("--core=")
                .and_then(codegen::CoreVersion::parse)
        })
        .unwrap_or_default()
}

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("parse") => run_parse(),
        Some("interpolate") => run_interpolate(),
        Some("template") => run_template(),
        Some("generate") => run_generate(),
        Some("imports") => run_imports(),
        Some("--version") => {
            println!("vue-check-codegen {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            eprintln!(
                "usage: vue-check-codegen <parse|interpolate|template|generate|imports> < input.jsonl > output.jsonl"
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
