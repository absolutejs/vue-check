//! One `.vue` file to the TypeScript that is checked for it: what the
//! `vue-tsx` plugin's `useCodegen` derives from the file's blocks
//! (`plugins/vue-tsx.ts`, `virtualCode/ir.ts`), and the mappings Volar makes
//! of the generated code (`virtualCode/embeddedCodes.ts`).

use std::collections::HashSet;

use super::script::{FileAttr, Generic, ScriptOptions, generate_script};
use super::style::{ResolveClassNames, Style, StyleOptions, generate_style};
use super::CoreVersion;
use super::template::generate::{CompilerFlags, TemplateOptions, generate_template};
use super::{Code, Source, Verification, to_string};
use crate::js;
use crate::script::{
    self, ScriptLang, ScriptRanges, ScriptSetupRanges, binding_flag, parse_script_ranges,
    parse_script_setup_ranges,
};
use crate::sfc::{AttrOrTrue, Block, parse_sfc};
use crate::template::parser::{ParseOptions, parse};
use crate::template::{compile, transform};
use crate::utf16::Utf16Map;

/// A mapping from the generated code to the file, as Volar's `Mapping`:
/// several ranges when pieces of code were combined. In UTF-16 units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub source_offsets: Vec<usize>,
    pub generated_offsets: Vec<usize>,
    pub lengths: Vec<usize>,
    pub verification: Verification,
}

/// The generated code of one file.
#[derive(Debug)]
pub struct GeneratedFile {
    /// `script_ts`, `script_js`, ...
    pub id: String,
    /// The language Volar gives the code: `typescript`, `javascript`, ...
    pub language_id: &'static str,
    pub text: String,
    pub mappings: Vec<Mapping>,
}

/// Why a file is left to the original codegen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Declined {
    pub reason: &'static str,
    /// The original keeps parser state between files, which this file
    /// changes: the files after it must be left to the original too.
    pub poisons_later_parses: bool,
}

impl Declined {
    fn file(reason: &'static str) -> Self {
        Self {
            reason,
            poisons_later_parses: false,
        }
    }
}

/// `validLangs`, as the original has them: a block in another language is
/// not analyzed.
fn valid_lang(lang: &str) -> Option<ScriptLang> {
    ScriptLang::from_lang(lang)
}

/// `computeLang`: the language of the generated code.
fn compute_lang(script: Option<&str>, script_setup: Option<&str>) -> &'static str {
    let lang = match (script, script_setup) {
        (Some(script), Some(setup)) => Some(if setup == "js" { script } else { setup }),
        (script, setup) => setup.or(script),
    };
    match lang {
        Some("js") => "js",
        Some("jsx") => "jsx",
        Some("tsx") => "tsx",
        _ => "ts",
    }
}

/// `resolveCommonLanguageId`.
fn language_id(lang: &str) -> &'static str {
    match lang {
        "js" => "javascript",
        "jsx" => "javascriptreact",
        "tsx" => "typescriptreact",
        _ => "typescript",
    }
}

/// `useSfcBlock`'s `start`: where the last `<tag` before the content is. An
/// attribute's place is kept from there; it is only known here when that is
/// where the block's tag starts.
fn block_start_is_tag(source: &str, block: &Block) -> bool {
    let tag = if block.tag == "script" || block.tag == "template" || block.tag == "style" {
        block.tag.as_str()
    } else {
        return true;
    };
    source
        .get(..block.start.offset)
        .and_then(|before| before.rfind(&format!("<{tag}")))
        == Some(block.tag_start)
}

/// A `<!-- @option value -->` comment sets a compiler option for its file
/// (`parseVueCompilerOptions`).
fn has_option_comment(comments: &[String]) -> bool {
    comments.iter().any(|comment| {
        comment
            .lines()
            .any(|line| js::trim_start(line).starts_with('@'))
    })
}

fn ordered_set(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

/// The highest combine token in the codes.
fn last_token(codes: &[Code]) -> u32 {
    codes
        .iter()
        .filter_map(|code| match code {
            Code::Mapped { features, .. } => features.combine,
            Code::Text(_) => None,
        })
        .max()
        .unwrap_or(0)
}

/// The blocks, as the codegen reads them.
struct Blocks<'a> {
    source: &'a str,
    template: Option<&'a Block>,
    script: Option<&'a Block>,
    /// `None` for the empty `<script setup>` a file with no script is given,
    /// which starts where the file does.
    script_setup: Option<&'a Block>,
    styles: &'a [Block],
}

impl Blocks<'_> {
    /// `block.startTagEnd`, for the block a piece of code names.
    fn content_start(&self, source: Source) -> usize {
        let block = match source {
            Source::Template => self.template,
            Source::Script => self.script,
            Source::ScriptSetup => self.script_setup,
            Source::Style(index) => self.styles.get(index),
            Source::Main => None,
        };
        block.map_or(0, |block| block.start.offset)
    }
}

/// `getMappingsForCode`.
fn build_mappings(
    codes: &[Code],
    blocks: &Blocks,
    decoded: &[crate::template::ast::DecodedValue],
) -> Vec<Mapping> {
    let map = Utf16Map::new(blocks.source);
    let mut mappings: Vec<Mapping> = Vec::new();
    // The mapping each combine token started.
    let mut combined: Vec<(u32, usize)> = Vec::new();
    let mut length = 0;
    for code in codes {
        match code {
            Code::Text(text) => length += text.encode_utf16().count(),
            Code::Mapped {
                text,
                source,
                offset,
                features,
            } => {
                let units = text.encode_utf16().count();
                let base = blocks.content_start(*source);
                let source_offset = if *source == Source::Template {
                    map.offset_in(base, *offset, decoded)
                } else {
                    map.offset(base + offset)
                };
                let existing = features
                    .combine
                    .and_then(|token| combined.iter().find(|(known, _)| *known == token))
                    .and_then(|(_, index)| mappings.get_mut(*index));
                if let Some(mapping) = existing {
                    mapping.source_offsets.push(source_offset);
                    mapping.generated_offsets.push(length);
                    mapping.lengths.push(units);
                } else {
                    if let Some(token) = features.combine {
                        combined.push((token, mappings.len()));
                    }
                    mappings.push(Mapping {
                        source_offsets: vec![source_offset],
                        generated_offsets: vec![length],
                        lengths: vec![units],
                        verification: features.verification,
                    });
                }
                length += units;
            }
        }
    }
    mappings
}

/// What `useCodegen` derives from the script blocks' analysis.
#[derive(Default)]
struct Derived {
    /// `getBindingFlags`, in the original's order.
    binding_flags: Vec<(String, u8)>,
    setup_bindings: HashSet<String>,
    script_setup_bindings: HashSet<String>,
    imported_components: HashSet<String>,
    setup_consts: HashSet<String>,
    setup_refs: HashSet<String>,
}

fn derive(
    script_ranges: Option<&ScriptRanges>,
    setup_ranges: Option<&ScriptSetupRanges>,
    has_setup: bool,
    core: CoreVersion,
) -> Derived {
    let mut derived = Derived::default();
    if let Some(ranges) = setup_ranges {
        derived.binding_flags.clone_from(&ranges.bindings.0);
        derived.script_setup_bindings = ranges
            .bindings
            .0
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
    }
    if has_setup && let Some(ranges) = script_ranges {
        for (name, flags) in &ranges.bindings.0 {
            if !derived.binding_flags.iter().any(|(known, _)| known == name) {
                derived.binding_flags.push((name.clone(), *flags));
            }
        }
    }
    // Before 3.3.12 an imported component isn't a binding, and the constants
    // are the components (and the destructured props, below).
    let old = core < CoreVersion::V3_3_12;
    for (name, flags) in &derived.binding_flags {
        let is_component = flags & binding_flag::COMPONENT != 0;
        if !(old && is_component) {
            derived.setup_bindings.insert(name.clone());
        }
        if is_component {
            derived.imported_components.insert(name.clone());
        }
        if if old {
            is_component
        } else {
            flags & binding_flag::CONST != 0
        } {
            derived.setup_consts.insert(name.clone());
        }
    }
    if old && setup_ranges.is_none() {
        // Without `<script setup>` the original reads none of this.
        derived.setup_bindings.clear();
        derived.imported_components.clear();
        derived.setup_consts.clear();
    }
    if let Some(props) = setup_ranges.and_then(|ranges| ranges.define_props.as_ref())
        && let Some(destructured) = &props.destructured
    {
        derived
            .setup_consts
            .extend(destructured.iter().map(|(name, _)| name.clone()));
        // The original only reads the rest of a destructuring.
        derived.setup_consts.extend(props.destructured_rest.clone());
    }
    if let Some(ranges) = setup_ranges {
        derived.setup_refs = ranges
            .use_template_ref
            .iter()
            .filter_map(|template_ref| template_ref.name.clone())
            .collect();
    }
    derived
}

/// The project's compiler options (`vueCompilerOptions`), as far as the port
/// reads them.
#[derive(Clone, Debug)]
pub struct FileOptions {
    pub compiler: CompilerFlags,
    /// The macros and composables, by the names the project gives them.
    pub analysis: script::ScriptOptions,
    pub jsx_slots: bool,
    pub resolve_class_names: ResolveClassNames,
    pub resolve_style_imports: bool,
    pub strict_css_modules: bool,
    /// `optionsWrapper`: what an options object is wrapped in, if anything.
    pub options_wrapper: Option<(String, String)>,
}

impl Default for FileOptions {
    /// The options of a project that sets none.
    fn default() -> Self {
        Self {
            compiler: CompilerFlags::default(),
            analysis: script::ScriptOptions::default(),
            jsx_slots: false,
            resolve_class_names: ResolveClassNames::Scoped,
            resolve_style_imports: false,
            strict_css_modules: false,
            options_wrapper: Some((
                "(await import('vue')).defineComponent(".to_owned(),
                ")".to_owned(),
            )),
        }
    }
}

/// Generates a file's code, with the project's compiler options.
/// `file_name` is the file's base name, and `types_path` the folder of
/// language-core's type declarations as the file's `/// <reference>` lines
/// name it; `target` is the version of Vue the code is for.
#[expect(
    clippy::too_many_lines,
    reason = "one derivation in the original, in the same order"
)]
pub fn generate_file(
    source: &str,
    file_name: &str,
    types_path: &str,
    target: f64,
    options: &FileOptions,
) -> Result<GeneratedFile, Declined> {
    let FileOptions {
        compiler,
        analysis,
        jsx_slots,
        resolve_class_names,
        resolve_style_imports,
        strict_css_modules,
        options_wrapper,
    } = options;
    let sfc = parse_sfc(source, options.compiler.cdata_outside_element_throws);
    if sfc.parsed.poisons_later_parses {
        return Err(Declined {
            reason: sfc
                .unsupported
                .unwrap_or("the file changes how later files parse"),
            poisons_later_parses: true,
        });
    }
    if let Some(reason) = sfc.unsupported {
        return Err(Declined::file(reason));
    }
    let descriptor = &sfc.descriptor;
    if has_option_comment(&descriptor.comments) {
        return Err(Declined::file("a comment sets a compiler option"));
    }
    let fake_setup = descriptor.script.is_none() && descriptor.script_setup.is_none();
    let blocks = Blocks {
        source,
        template: descriptor.template.as_ref(),
        script: descriptor.script.as_ref(),
        script_setup: descriptor.script_setup.as_ref(),
        styles: &descriptor.styles,
    };
    let all_blocks = blocks
        .template
        .into_iter()
        .chain(blocks.script)
        .chain(blocks.script_setup)
        .chain(blocks.styles);
    for block in all_blocks {
        if !block_start_is_tag(source, block) {
            return Err(Declined::file(
                "a block's tag isn't the last one before its content",
            ));
        }
    }

    let script_lang = blocks
        .script
        .map(|block| block.lang.as_deref().unwrap_or("js"));
    let setup_lang = if fake_setup {
        Some("ts")
    } else {
        blocks
            .script_setup
            .map(|block| block.lang.as_deref().unwrap_or("js"))
    };
    let lang = compute_lang(script_lang, setup_lang);
    let is_ts = matches!(lang, "ts" | "tsx");

    // The script blocks' analysis.
    let script_ranges = match blocks.script.zip(script_lang.and_then(valid_lang)) {
        Some((block, lang)) => {
            Some(parse_script_ranges(&block.content, lang, analysis).map_err(Declined::file)?)
        }
        None => None,
    };
    let setup_content = if fake_setup {
        Some("")
    } else {
        blocks.script_setup.map(|block| block.content.as_str())
    };
    let setup_ranges = match setup_content.zip(setup_lang.and_then(valid_lang)) {
        Some((content, lang)) => {
            Some(parse_script_setup_ranges(content, lang, analysis).map_err(Declined::file)?)
        }
        None => None,
    };
    let derived = derive(
        script_ranges.as_ref(),
        setup_ranges.as_ref(),
        setup_content.is_some(),
        compiler.core,
    );

    let has_vapor = |block: Option<&Block>| {
        block.is_some_and(|block| block.attrs.iter().any(|(name, _)| name == "vapor"))
    };
    let is_vapor = has_vapor(blocks.script_setup) || has_vapor(blocks.template);
    let component_options = script_ranges
        .as_ref()
        .and_then(|ranges| ranges.export_default.as_ref())
        .and_then(|export| export.options.as_ref());
    let define_options = setup_ranges
        .as_ref()
        .and_then(|ranges| ranges.define_options.as_ref());
    let inherit_attrs = define_options
        .and_then(|options| options.inherit_attrs.as_deref())
        .or(component_options.and_then(|options| options.inherit_attrs.as_deref()))
        != Some("false");
    let component_name = {
        let from_option = blocks
            .script
            .zip(component_options.and_then(|options| options.name))
            .map(|(block, name)| {
                block
                    .content
                    .get(name.start + 1..name.end.saturating_sub(1))
                    .unwrap_or("")
                    .to_owned()
            });
        let name = from_option
            .or_else(|| {
                define_options
                    .and_then(|options| options.name.clone())
                    .filter(|name| !name.is_empty())
            })
            .unwrap_or_else(|| {
                file_name
                    .rfind('.')
                    .and_then(|dot| file_name.get(..dot))
                    .unwrap_or("")
                    .to_owned()
            });
        js::capitalize(&js::camelize(&name))
    };

    // The template's tree, as the `vue-template-html` plugin makes it.
    let parsed_template = match blocks.template {
        Some(block) => {
            if !matches!(block.lang.as_deref(), None | Some("html" | "md")) {
                return Err(Declined::file("a template in a language other than HTML"));
            }
            let text = compile::with_suffix(&block.content);
            let mut parsed = parse(
                &text,
                ParseOptions::template(compiler.cdata_outside_element_throws),
            );
            if parsed.poisons_later_parses {
                return Err(Declined {
                    reason: parsed
                        .unsupported
                        .unwrap_or("the template changes how later files parse"),
                    poisons_later_parses: true,
                });
            }
            if let Some(reason) = parsed
                .unsupported
                .or_else(|| transform::transform(&mut parsed.root))
            {
                return Err(Declined::file(reason));
            }
            Some((block, text, parsed.root))
        }
        None => None,
    };

    let mut styles = Vec::new();
    for block in blocks.styles {
        let scan = crate::style::scan(&block.content);
        if let Some(reason) = scan.unsupported {
            return Err(Declined::file(reason));
        }
        styles.push(Style {
            scoped: block.scoped,
            module: block
                .module
                .as_ref()
                .map(|module| (module.text.clone(), module.position)),
            bindings: scan.bindings,
            class_names: scan.class_names,
            module_written: match &block.module_written {
                Some(AttrOrTrue::Attr(attr)) => Some((attr.text.clone(), attr.position)),
                _ => None,
            },
            imports: scan.imports,
            src: match &block.src {
                Some(AttrOrTrue::Attr(src)) => Some((src.text.clone(), src.position)),
                _ => None,
            },
        });
    }

    let template_pass = |dot_value_bindings: &HashSet<String>| {
        let (block, text, root) = parsed_template.as_ref()?;
        let reassert_bindings: HashSet<String> = derived
            .binding_flags
            .iter()
            .filter(|(name, flags)| {
                flags & binding_flag::VARIABLE != 0 && dot_value_bindings.contains(name)
            })
            .map(|(name, _)| name.clone())
            .collect();
        let define_props = setup_ranges
            .as_ref()
            .and_then(|ranges| ranges.define_props.as_ref());
        let define_slots = setup_ranges
            .as_ref()
            .and_then(|ranges| ranges.define_slots.as_ref());
        Some(generate_template(
            &TemplateOptions {
                template: &block.content,
                parsed: text,
                is_html: !matches!(block.lang.as_deref(), Some("md")),
                // Before 3.3.12 the template and the styles are written as
                // TypeScript whatever the script's language.
                is_ts: is_ts || compiler.core < CoreVersion::V3_3_12,
                is_vapor,
                compiler,
                imported_components: &derived.imported_components,
                setup_refs: &derived.setup_refs,
                setup_consts: &derived.setup_consts,
                setup_bindings: &derived.setup_bindings,
                dot_value_bindings,
                reassert_bindings: &reassert_bindings,
                has_define_slots: define_slots.is_some(),
                props_assign_name: define_props.and_then(|props| props.call.name.as_deref()),
                slots_assign_name: define_slots.and_then(|slots| slots.name.as_deref()),
                inherit_attrs,
                component_name: &component_name,
            },
            Some(root),
        ))
    };
    let style_pass = |dot_value_bindings: &HashSet<String>, first_token: u32| {
        if styles.is_empty() {
            return None;
        }
        Some(generate_style(
            &StyleOptions {
                core: compiler.core,
                styles: &styles,
                is_ts: is_ts || compiler.core < CoreVersion::V3_3_12,
                lib: &compiler.lib,
                resolve_class_names: *resolve_class_names,
                resolve_style_imports: *resolve_style_imports,
                strict_css_modules: *strict_css_modules,
                setup_refs: &derived.setup_refs,
                setup_consts: &derived.setup_consts,
                setup_bindings: &derived.setup_bindings,
                dot_value_bindings,
            },
            first_token,
        ))
    };

    // First the bindings read where a type is narrowed, then the code, in
    // which each of them is read with `.value`.
    let has_bindings = !derived.setup_bindings.is_empty();
    let old = compiler.core < CoreVersion::V3_3_12;
    let dot_value_bindings = if has_bindings && !old {
        let none = HashSet::new();
        let template = template_pass(&none);
        let style = style_pass(&none, 0);
        if let Some(reason) = template
            .as_ref()
            .and_then(|generated| generated.unsupported)
        {
            return Err(Declined::file(reason));
        }
        if let Some(reason) = style.as_ref().and_then(|generated| generated.unsupported) {
            return Err(Declined::file(reason));
        }
        ordered_set(
            template
                .into_iter()
                .flat_map(|generated| generated.dot_value_accesses)
                .chain(
                    style
                        .into_iter()
                        .flat_map(|generated| generated.dot_value_accesses),
                ),
        )
    } else {
        Vec::new()
    };
    let dot_value_set: HashSet<String> = dot_value_bindings.iter().cloned().collect();
    let template = template_pass(&dot_value_set);
    if let Some(reason) = template
        .as_ref()
        .and_then(|generated| generated.unsupported)
    {
        return Err(Declined::file(reason));
    }
    let template_tokens = template
        .as_ref()
        .map_or(0, |generated| last_token(&generated.codes));
    let style = style_pass(&dot_value_set, template_tokens);
    if let Some(reason) = style.as_ref().and_then(|generated| generated.unsupported) {
        return Err(Declined::file(reason));
    }

    // Before 3.3.12: the bindings the template and the styles read, and the
    // ones its components are named after.
    let exposed = if has_bindings && old {
        ordered_set(
            template
                .iter()
                .flat_map(|generated| generated.context_access_names.clone())
                .chain(
                    style
                        .iter()
                        .flat_map(|generated| generated.context_access_names.clone()),
                )
                .chain(
                    parsed_template
                        .iter()
                        .flat_map(|(_, _, root)| &root.components)
                        .flat_map(|name| {
                            let camelized = js::camelize(name);
                            let capitalized = js::capitalize(&camelized);
                            [camelized, capitalized]
                        }),
                )
                .filter(|name| derived.setup_bindings.contains(name)),
        )
    } else {
        Vec::new()
    };
    let local_components = if has_bindings {
        ordered_set(
            parsed_template
                .iter()
                .flat_map(|(_, _, root)| &root.components)
                .flat_map(|name| {
                    let camelized = js::camelize(name);
                    let capitalized = js::capitalize(&camelized);
                    [camelized, capitalized]
                })
                .filter(|name| derived.setup_bindings.contains(name)),
        )
    } else {
        Vec::new()
    };
    // `/^v[A-Z]/`
    let local_directives: Vec<String> = derived
        .binding_flags
        .iter()
        .map(|(name, _)| name)
        .filter(|name| {
            name.starts_with('v')
                && name
                    .chars()
                    .nth(1)
                    .is_some_and(|second| second.is_ascii_uppercase())
        })
        .cloned()
        .collect();

    let mut types: Vec<String> = Vec::new();
    let mut codes: Vec<Code> = Vec::new();
    let mut tokens = template_tokens;
    if let Some(generated) = style {
        tokens = tokens.max(last_token(&generated.codes));
        codes.extend(generated.codes);
        types.extend(generated.generated_types);
    }
    if let Some(generated) = template {
        codes.extend(generated.codes);
        // The template's types come first in the original's set.
        types.splice(0..0, generated.generated_types);
    }

    let attr = |attr: &crate::sfc::BlockAttr| FileAttr {
        text: attr.text.clone(),
        offset: attr.position,
    };
    let script_src = blocks.script.and_then(|block| match &block.src {
        Some(AttrOrTrue::Attr(src)) => Some(attr(src)),
        _ => None,
    });
    let generic = blocks.script_setup.and_then(|block| match &block.generic {
        Some(AttrOrTrue::Attr(generic)) => Some(Generic::Parameters(attr(generic))),
        Some(AttrOrTrue::True) => Some(Generic::Empty),
        None => None,
    });
    let script_codes = generate_script(
        &ScriptOptions {
            core: compiler.core,
            lib: &compiler.lib,
            target,
            check_unknown_props: compiler.check_unknown_props,
            infer_component_dollar_refs: compiler.infer_component_dollar_refs,
            infer_component_dollar_el: compiler.infer_component_dollar_el,
            infer_template_dollar_attrs: compiler.infer_template_dollar_attrs,
            infer_template_dollar_slots: compiler.infer_template_dollar_slots,
            jsx_slots: *jsx_slots,
            options_wrapper: options_wrapper
                .as_ref()
                .map(|(left, right)| (left.as_str(), right.as_str())),
            types_path,
            // Before 3.3.12 every helper is written as TypeScript.
            is_ts: is_ts || old,
            script: blocks.script.map(|block| block.content.as_str()),
            script_src: script_src.as_ref(),
            script_ranges: script_ranges.as_ref(),
            script_setup: setup_content,
            script_setup_generic: generic.as_ref(),
            script_setup_ranges: setup_ranges.as_ref(),
            template_and_style_types: &types,
            setup_bindings: &derived.script_setup_bindings,
            exposed: &exposed,
            local_components: &local_components,
            local_directives: &local_directives,
            dot_value_bindings: &dot_value_bindings,
        },
        codes,
        tokens,
    );
    Ok(GeneratedFile {
        id: format!("script_{lang}"),
        language_id: language_id(lang),
        text: to_string(&script_codes),
        mappings: build_mappings(
            &script_codes,
            &blocks,
            parsed_template
                .as_ref()
                .map_or(&[], |(_, _, root)| root.decoded.as_slice()),
        ),
    })
}
