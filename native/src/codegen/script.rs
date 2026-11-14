//! The generated file around the template: a port of `codegen/script`
//! (`index.ts`, `scriptSetup.ts`, `template.ts`, `component.ts`) and of
//! `codegen/localTypes.ts`.
//!
//! The original passes generators to one another, to be run where they are
//! yielded. Here each is a method, called at that place.

use crate::codegen::CoreVersion;
use std::collections::HashSet;

use super::local_types_data as local_types;
use super::{
    Boundary, Code, END_OF_LINE, Features, NEW_LINE, Source, Verification, as_type,
    generate_camelized, is_identifier, ref_brand_argument,
};
use crate::js;
use crate::script::{CallRange, DefineModel, MacroCall, Range, ScriptRanges, ScriptSetupRanges};

/// An attribute of a block's tag whose value the codegen maps: its text, and
/// where it is in the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAttr {
    pub text: String,
    pub offset: usize,
}

/// `<script setup generic>`: with no value, or with type parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Generic {
    Empty,
    Parameters(FileAttr),
}

/// `ScriptCodegenOptions`, and the compiler options it reads.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the original's options, by their names"
)]
#[derive(Debug)]
pub struct ScriptOptions<'a> {
    pub core: CoreVersion,
    pub lib: &'a str,
    /// `vueCompilerOptions.target`: the Vue version.
    pub target: f64,
    pub check_unknown_props: bool,
    pub infer_component_dollar_refs: bool,
    pub infer_component_dollar_el: bool,
    pub infer_template_dollar_attrs: bool,
    pub infer_template_dollar_slots: bool,
    pub jsx_slots: bool,
    /// `optionsWrapper`: what an options object is wrapped in.
    pub options_wrapper: Option<(&'a str, &'a str)>,
    /// The path of language-core's type declarations from the file, as the
    /// `/// <reference>` lines have it.
    pub types_path: &'a str,
    pub is_ts: bool,
    /// The `<script>` block's content.
    pub script: Option<&'a str>,
    /// `<script src="...">`.
    pub script_src: Option<&'a FileAttr>,
    pub script_ranges: Option<&'a ScriptRanges>,
    pub script_setup: Option<&'a str>,
    pub script_setup_generic: Option<&'a Generic>,
    pub script_setup_ranges: Option<&'a ScriptSetupRanges>,
    pub template_and_style_types: &'a [String],
    /// The names `<script setup>` declares.
    pub setup_bindings: &'a HashSet<String>,
    pub local_components: &'a [String],
    pub local_directives: &'a [String],
    /// Before 3.3.12: the `<script setup>` bindings the template and the
    /// styles read (`getSetupExposed`), in the original's order.
    pub exposed: &'a [String],
    pub dot_value_bindings: &'a [String],
}

const ALL: Features = Features::new(Verification::On);
const NAVIGATION: Features = Features::new(Verification::Off);
const WITHOUT_SEMANTIC: Features = Features::new(Verification::On);
const NOT_TS6133: Features = Features::new(Verification::NotTs6133);

/// The helper types, in the order the original declares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalType {
    PrettifyLocal,
    WithDefaults,
    WithSlots,
    PropsChildren,
    TypePropsToOption,
    OmitIndexSignature,
}

/// A change to `<script setup>`'s code: a range replaced by generated code.
struct Transform {
    start: usize,
    end: usize,
    codes: Vec<Code>,
}

/// What follows the component in `generateSetupFunction` (`output`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Output {
    /// `return `
    Return,
    /// `const __VLS_export = `
    ExportDeclare,
}

struct Generator<'a> {
    options: &'a ScriptOptions<'a>,
    /// The template's and the styles' code, written into the template function.
    template_and_style_codes: Vec<Code>,
    out: Vec<Code>,
    generated_types: Vec<&'static str>,
    used_local_types: Vec<LocalType>,
    next_token: u32,
}

/// `generateScript`. `first_token` keeps its combine tokens apart from the
/// template's and the styles'.
pub fn generate_script(
    options: &ScriptOptions,
    template_and_style_codes: Vec<Code>,
    first_token: u32,
) -> Vec<Code> {
    let mut generator = Generator {
        options,
        template_and_style_codes,
        out: Vec::new(),
        generated_types: Vec::new(),
        used_local_types: Vec::new(),
        next_token: first_token,
    };
    generator.worker();
    generator.out
}

impl<'a> Generator<'a> {
    fn text(&mut self, text: impl Into<String>) {
        self.out.push(Code::Text(text.into()));
    }

    fn boundary(
        &mut self,
        source: Source,
        start: usize,
        end: usize,
        features: Features,
    ) -> Boundary {
        self.next_token += 1;
        Boundary::start(&mut self.out, self.next_token, source, start, end, features)
    }

    fn as_type(&self, type_text: &str) -> String {
        as_type(type_text, self.options.is_ts)
    }

    fn has_type(&self, name: &str) -> bool {
        self.options
            .template_and_style_types
            .iter()
            .any(|known| known == name)
    }

    /// `ctx.localTypes.<name>`: the helper's name, which is then declared.
    fn local_type(&mut self, local_type: LocalType) -> &'static str {
        if !self.used_local_types.contains(&local_type) {
            self.used_local_types.push(local_type);
        }
        match local_type {
            LocalType::PrettifyLocal => "__VLS_PrettifyLocal",
            LocalType::WithDefaults => "__VLS_WithDefaults",
            LocalType::WithSlots => "__VLS_WithSlots",
            LocalType::PropsChildren => "__VLS_PropsChildren",
            LocalType::TypePropsToOption => "__VLS_TypePropsToOption",
            LocalType::OmitIndexSignature => "__VLS_OmitIndexSignature",
        }
    }

    /// 3.3.11 and before: the codegen 3.3.12 rewrote.
    fn before_3_3_12(&self) -> bool {
        self.options.core < CoreVersion::V3_3_12
    }

    /// `ctx.localTypes.generate()`: the helpers used, in the order they were.
    fn local_types(&mut self) {
        let mut index = 0;
        while let Some(local_type) = self.used_local_types.get(index).copied() {
            index += 1;
            let is_ts = self.options.is_ts;
            let text = match (local_type, is_ts) {
                (LocalType::PrettifyLocal, true) => local_types::PRETTIFY_LOCAL_TS,
                (LocalType::PrettifyLocal, false) => local_types::PRETTIFY_LOCAL_JS,
                (LocalType::WithDefaults, true) => local_types::WITH_DEFAULTS_TS,
                (LocalType::WithDefaults, false) => local_types::WITH_DEFAULTS_JS,
                (LocalType::WithSlots, true) => local_types::WITH_SLOTS_TS,
                (LocalType::WithSlots, false) => local_types::WITH_SLOTS_JS,
                (LocalType::PropsChildren, true) => local_types::PROPS_CHILDREN_TS,
                (LocalType::PropsChildren, false) => local_types::PROPS_CHILDREN_JS,
                (LocalType::TypePropsToOption, true) => local_types::TYPE_PROPS_TO_OPTION_TS,
                (LocalType::TypePropsToOption, false) => local_types::TYPE_PROPS_TO_OPTION_JS,
                (LocalType::OmitIndexSignature, true) => local_types::OMIT_INDEX_SIGNATURE_TS,
                (LocalType::OmitIndexSignature, false) => local_types::OMIT_INDEX_SIGNATURE_JS,
            };
            // `WithDefaults` is written with `PrettifyLocal`, which it then uses.
            if local_type == LocalType::WithDefaults {
                self.local_type(LocalType::PrettifyLocal);
            }
            let mut text = text.replace('\u{1}', self.options.lib);
            if self.before_3_3_12() {
                // The mapped types 3.3.12 narrowed to string keys.
                text = text
                    .replace("[K in keyof P & string]", "[K in keyof Pick<P, keyof P>]")
                    .replace("[K in keyof T & string]-?", "[K in keyof T]-?");
            }
            self.text(text);
        }
    }

    /// `generateSfcBlockSection`: a piece of a `<script>` block as it is.
    fn section(&mut self, source: Source, start: usize, end: usize, features: Features) {
        let codes = self.section_codes(source, start, end, features);
        self.out.extend(codes);
    }

    fn section_codes(
        &self,
        source: Source,
        start: usize,
        end: usize,
        features: Features,
    ) -> Vec<Code> {
        let content = match source {
            Source::Script => self.options.script,
            _ => self.options.script_setup,
        };
        vec![Code::Mapped {
            // A range that ends before it starts is empty, as `slice` has it.
            text: content
                .and_then(|content| content.get(start..end))
                .unwrap_or("")
                .to_owned(),
            source,
            offset: start,
            features,
        }]
    }

    fn range_text(&self, range: Range) -> &'a str {
        self.options
            .script_setup
            .and_then(|content| content.get(range.start..range.end))
            .unwrap_or("")
    }

    /// `generateTypedVar`.
    fn typed_var(&mut self, kind: &str, name: &str, type_text: &str) {
        if self.options.is_ts {
            self.text(format!("{kind} {name}!: "));
            self.text(type_text);
            self.text(END_OF_LINE);
        } else {
            self.text(format!("var {name} = /** @type {{"));
            self.text(type_text);
            self.text(format!("}} */ ({{}}){END_OF_LINE}"));
        }
    }

    /// `generateTypeAlias`.
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

    /// `generateSpreadMerge`: one object, or several spread into one.
    fn spread_merge(&mut self, codes: Vec<Code>) {
        if codes.len() <= 1 {
            self.out.extend(codes);
            return;
        }
        self.text(format!("{{{NEW_LINE}"));
        for code in codes {
            self.text("...");
            self.out.push(code);
            self.text(format!(",{NEW_LINE}"));
        }
        self.text("}");
    }

    /// `generateExportDeclareEqual`: `const name = `, mapped to the block so
    /// that an unused-variable error on it isn't shown.
    fn export_declare_equal(&mut self, source: Source, name: &str) {
        let length = match source {
            Source::Script => self.options.script,
            _ => self.options.script_setup,
        }
        .map_or(0, str::len);
        self.text("const ");
        let boundary = self.boundary(source, 0, length, NOT_TS6133);
        self.text(name);
        self.out.push(boundary.end());
        self.text(" = ");
    }

    // ---- `index.ts` ----

    fn worker(&mut self) {
        let options = self.options;
        let export_expression = self.as_type("typeof __VLS_export");
        self.global_types_reference();

        if let Some(src) = options.script_src {
            self.script_src(src);
        } else if let (Some(script), Some(ranges), Some(_), Some(setup_ranges)) = (
            options.script,
            options.script_ranges,
            options.script_setup,
            options.script_setup_ranges,
        ) {
            // <script> and <script setup>
            self.script_setup_imports(setup_ranges);
            // Where the macros are declared moved twice: at the end of the
            // file in 3.3.12, here in 3.3.13, after `<script>` since.
            if options.core == CoreVersion::V3_3_13 {
                self.macros();
            }
            let mut self_type = None;
            if ranges.export_default.is_some() {
                self_type = Some("__VLS_self");
                self.script_with_export_default(ranges, "__VLS_self", &export_expression, None);
            } else {
                self.section(Source::Script, 0, script.len(), ALL);
                self.text(format!("export default {export_expression}{END_OF_LINE}"));
            }
            if options.core.is_3_3_14() {
                self.macros();
            }
            self.export_declare_equal(Source::ScriptSetup, "__VLS_export");
            if let Some(generic) = options.script_setup_generic {
                self.generic(setup_ranges, generic, self_type);
            } else {
                self.text(format!("await (async () => {{{NEW_LINE}"));
                self.setup_function(setup_ranges, self_type, Some(Output::Return));
                self.text(format!("}})(){END_OF_LINE}"));
            }
        } else if let (Some(_), Some(setup_ranges)) =
            (options.script_setup, options.script_setup_ranges)
        {
            // Only <script setup>: its code is at the root of the file.
            self.script_setup_imports(setup_ranges);
            if options.core >= CoreVersion::V3_3_13 {
                self.macros();
            }
            if let Some(generic) = options.script_setup_generic {
                self.export_declare_equal(Source::ScriptSetup, "__VLS_export");
                self.generic(setup_ranges, generic, None);
            } else {
                self.setup_function(setup_ranges, None, Some(Output::ExportDeclare));
            }
            self.text(format!("export default {export_expression}{END_OF_LINE}"));
        } else if let (Some(script), Some(ranges)) = (options.script, options.script_ranges) {
            // Only <script>
            if ranges.export_default.is_some() {
                self.script_with_export_default(
                    ranges,
                    "__VLS_export",
                    &export_expression,
                    Some("__VLS_export"),
                );
            } else {
                self.section(Source::Script, 0, script.len(), ALL);
                self.export_declare_equal(Source::Script, "__VLS_export");
                self.text(format!(
                    "(await import('{}')).defineComponent({{}}){END_OF_LINE}",
                    options.lib
                ));
                self.template(Some("__VLS_export"));
                self.text(format!("export default {export_expression}{END_OF_LINE}"));
            }
        }
        self.local_types();
        if options.core == CoreVersion::V3_3_12
            && options.script_setup.is_some()
            && options.script_setup_ranges.is_some()
            && options.script_src.is_none()
        {
            self.macros();
        }
    }

    /// `<script src="...">`: the component is the module's default export.
    fn script_src(&mut self, src: &FileAttr) {
        // The module TypeScript resolves: `.js` for `.ts`.
        let mut module = src.text.clone();
        if let Some(stem) = module
            .strip_suffix(".ts")
            .filter(|_| !module.ends_with(".d.ts"))
        {
            module = format!("{stem}.js");
        } else if let Some(stem) = module.strip_suffix(".tsx") {
            module = format!("{stem}.jsx");
        }
        self.text("import __VLS_src from ");
        let boundary = self.boundary(Source::Main, src.offset, src.offset + src.text.len(), ALL);
        self.text("'");
        self.out.push(Code::Mapped {
            text: module.get(..src.text.len()).unwrap_or(&module).to_owned(),
            source: Source::Main,
            offset: src.offset,
            features: boundary.features,
        });
        self.text(module.get(src.text.len()..).unwrap_or(""));
        self.text("'");
        self.out.push(boundary.end());
        self.text(END_OF_LINE);
        self.text(format!("export default __VLS_src{END_OF_LINE}"));
        self.template(Some("__VLS_src"));
    }

    /// `generateScriptWithExportDefault`: the script, with its default export
    /// moved into a variable.
    fn script_with_export_default(
        &mut self,
        ranges: &ScriptRanges,
        var_name: &str,
        export_expression: &str,
        template_self_type: Option<&str>,
    ) {
        let Some(export_default) = &ranges.export_default else {
            return;
        };
        let script_length = self.options.script.map_or(0, str::len);
        let (expression, is_object_literal) = match &export_default.options {
            Some(options) => (options.expression, options.is_object_literal),
            None => (export_default.expression, export_default.is_object_literal),
        };
        let wrapper = self.options.options_wrapper.filter(|_| is_object_literal);

        self.section(Source::Script, 0, expression.start, ALL);
        self.text(export_expression);
        self.section(
            Source::Script,
            expression.end,
            export_default.range.end,
            ALL,
        );
        self.text(END_OF_LINE);
        if template_self_type.is_some() {
            self.template(template_self_type);
        }
        self.export_declare_equal(Source::Script, var_name);
        // The original wraps only when both halves of the wrapper have text.
        match wrapper.filter(|(left, right)| !left.is_empty() && !right.is_empty()) {
            Some((left, right)) => {
                self.text(left);
                self.section(Source::Script, expression.start, expression.end, ALL);
                self.text(right);
            }
            None => self.section(Source::Script, expression.start, expression.end, ALL),
        }
        self.text(END_OF_LINE);
        self.section(Source::Script, export_default.range.end, script_length, ALL);
    }

    /// `generateGlobalTypesReference`.
    fn global_types_reference(&mut self) {
        let options = self.options;
        let reference = |file: &str| {
            let path = serde_json::Value::String(format!("{}/{file}", options.types_path));
            format!("/// <reference types={path} />{NEW_LINE}")
        };
        self.text(reference("template-helpers.d.ts"));
        if !options.check_unknown_props {
            self.text(reference("props-fallback.d.ts"));
        }
        if options.lib == "vue" && options.target < 3.5 {
            self.text(reference("vue-3.4-shims.d.ts"));
        }
    }

    // ---- `scriptSetup.ts` ----

    /// Where `<script setup>`'s imports end, or its leading `@ts-check`.
    fn import_section_end(ranges: &ScriptSetupRanges) -> usize {
        ranges
            .import_section_end_offset
            .max(ranges.leading_comment_end_offset)
    }

    /// `generateScriptSetupImports`: the imports stay at the top of the file.
    fn script_setup_imports(&mut self, ranges: &ScriptSetupRanges) {
        self.section(
            Source::ScriptSetup,
            0,
            Self::import_section_end(ranges),
            ALL,
        );
    }

    /// `generateGeneric`: a generic component is a function of its props.
    #[expect(
        clippy::too_many_lines,
        reason = "one function in the original, in the same order"
    )]
    fn generic(
        &mut self,
        ranges: &ScriptSetupRanges,
        generic: &Generic,
        template_self_type: Option<&str>,
    ) {
        let options = self.options;
        let lib = options.lib;
        self.text("(");
        if let Generic::Parameters(parameters) = generic {
            let boundary = self.boundary(
                Source::Main,
                parameters.offset,
                parameters.offset + parameters.text.len(),
                Features::new(Verification::On),
            );
            self.text("<");
            self.out.push(Code::Mapped {
                text: parameters.text.clone(),
                source: Source::Main,
                offset: parameters.offset,
                features: ALL,
            });
            // 3.3.10 looks past the space after the last parameter.
            let written = if options.core < CoreVersion::V3_3_10 {
                parameters.text.as_str()
            } else {
                js::trim_end(&parameters.text)
            };
            if !written.ends_with(',') {
                self.text(",");
            }
            self.text(">");
            self.out.push(boundary.end());
        }
        let prettify = self.local_type(LocalType::PrettifyLocal);
        self.text(format!(
            "({NEW_LINE}\t__VLS_props: NonNullable<Awaited<typeof __VLS_setup>>['props'],{NEW_LINE}\t__VLS_ctx?: {prettify}<Pick<NonNullable<Awaited<typeof __VLS_setup>>, 'attrs' | 'emit' | 'slots'>>,{NEW_LINE}\t__VLS_exposed?: NonNullable<Awaited<typeof __VLS_setup>>['expose'],{NEW_LINE}\t__VLS_setup = (async () => {{{NEW_LINE}"
        ));

        self.setup_function(ranges, template_self_type, None);

        let mut prop_types: Vec<String> = Vec::new();
        let mut emit_types: Vec<String> = Vec::new();
        if self.generated_types.contains(&"__VLS_PublicProps") {
            prop_types.push("__VLS_PublicProps".to_owned());
        }
        if let Some(arg) = ranges
            .define_props
            .as_ref()
            .and_then(|props| props.call.call.arg)
        {
            self.text("const __VLS_propsOption = ");
            self.section(Source::ScriptSetup, arg.start, arg.end, NAVIGATION);
            self.text(END_OF_LINE);
            prop_types.push(format!(
                "import('{lib}').{}<typeof __VLS_propsOption>",
                if options.target >= 3.3 {
                    "ExtractPublicPropTypes"
                } else {
                    "ExtractPropTypes"
                }
            ));
        }
        let has_emit_props = ranges.define_emits.is_some() || !ranges.define_model.is_empty();
        if has_emit_props {
            prop_types.push("__VLS_EmitProps".to_owned());
        }
        if self.has_type("__VLS_InheritedAttrs") {
            prop_types.push(if has_emit_props && options.core.is_3_3_14() {
                "Omit<__VLS_InheritedAttrs, keyof __VLS_EmitProps>".to_owned()
            } else {
                "__VLS_InheritedAttrs".to_owned()
            });
        }
        if let Some(emits) = &ranges.define_emits {
            emit_types.push(format!(
                "typeof {}",
                emits.call.name.as_deref().unwrap_or("__VLS_emit")
            ));
        }
        if !ranges.define_model.is_empty() {
            emit_types.push("typeof __VLS_modelEmit".to_owned());
        }

        self.text(format!("return {{}} as {{{NEW_LINE}"));
        self.text("\tprops: ");
        self.text(if options.target >= 3.4 {
            format!("import('{lib}').PublicProps")
        } else if options.target >= 3.0 {
            format!(
                "import('{lib}').VNodeProps & import('{lib}').AllowedComponentProps & import('{lib}').ComponentCustomProps"
            )
        } else {
            "globalThis.JSX.IntrinsicAttributes".to_owned()
        });
        if !prop_types.is_empty() {
            let prettify = self.local_type(LocalType::PrettifyLocal);
            self.text(format!(" & {prettify}<{}>", prop_types.join(" & ")));
        }
        self.text(format!(
            " & (typeof globalThis extends {{ __VLS_PROPS_FALLBACK: infer P }} ? P : {{}}){END_OF_LINE}"
        ));
        self.text("\texpose: (exposed: ");
        self.text(if ranges.define_expose.is_some() {
            format!("import('{lib}').ShallowUnwrapRef<typeof __VLS_exposed>")
        } else {
            "{}".to_owned()
        });
        if options.infer_component_dollar_refs && self.has_type("__VLS_TemplateRefs") {
            self.text(" & { $refs: __VLS_TemplateRefs; }");
        }
        if options.infer_component_dollar_el && self.has_type("__VLS_RootEl") {
            self.text(" & { $el: __VLS_RootEl; }");
        }
        self.text(format!(") => void{END_OF_LINE}"));
        self.text(format!("\tattrs: any{END_OF_LINE}"));
        self.text(format!(
            "\tslots: {}{END_OF_LINE}",
            if self.has_slots_type() {
                "__VLS_Slots"
            } else {
                "{}"
            }
        ));
        self.text(format!(
            "\temit: {}{END_OF_LINE}",
            if emit_types.is_empty() {
                "{}".to_owned()
            } else {
                emit_types.join(" & ")
            }
        ));
        self.text(format!("}}{END_OF_LINE}"));
        self.text(format!("}})(),{NEW_LINE}"));
        self.text(format!(
            ") => ({{}} as import('{lib}').VNode & {{ __ctx?: NonNullable<Awaited<typeof __VLS_setup>> }})){END_OF_LINE}"
        ));
    }

    /// `hasSlotsType`.
    fn has_slots_type(&self) -> bool {
        self.options
            .script_setup_ranges
            .is_some_and(|ranges| ranges.define_slots.is_some())
            || self.has_type("__VLS_Slots")
    }

    fn insert(position: usize, codes: Vec<Code>) -> Transform {
        Transform {
            start: position,
            end: position,
            codes,
        }
    }

    fn text_code(text: impl Into<String>) -> Code {
        Code::Text(text.into())
    }

    /// `generateDefineWithTypeTransforms`: a macro's type argument becomes a
    /// type alias, and its result a variable.
    fn define_with_type_transforms(
        &self,
        transforms: &mut Vec<Transform>,
        call: &MacroCall,
        call_exp: Range,
        default_name: &str,
        type_name: &str,
    ) {
        let statement = call.statement;
        let type_arg = call.call.type_arg;
        let section =
            |start: usize, end: usize| self.section_codes(Source::ScriptSetup, start, end, ALL);
        if let Some(type_arg) = type_arg {
            let mut codes = vec![Self::text_code(format!("type {type_name} = "))];
            codes.extend(section(type_arg.start, type_arg.end));
            codes.push(Self::text_code(END_OF_LINE));
            transforms.push(Self::insert(statement.start, codes));
            transforms.push(Transform {
                start: type_arg.start,
                end: type_arg.end,
                codes: vec![Self::text_code(type_name)],
            });
        }
        let declare = Self::text_code(format!("const {default_name} = "));
        match &call.name {
            None => {
                if statement.start == call_exp.start && statement.end == call_exp.end {
                    transforms.push(Self::insert(call_exp.start, vec![declare]));
                } else if let Some(type_arg) = type_arg {
                    let mut before = vec![declare];
                    before.extend(section(call_exp.start, type_arg.start));
                    transforms.push(Transform {
                        start: statement.start,
                        end: type_arg.start,
                        codes: before,
                    });
                    let mut after = section(type_arg.end, call_exp.end);
                    after.push(Self::text_code(END_OF_LINE));
                    after.extend(section(statement.start, call_exp.start));
                    after.push(Self::text_code(default_name));
                    transforms.push(Transform {
                        start: type_arg.end,
                        end: call_exp.end,
                        codes: after,
                    });
                } else {
                    let mut codes = vec![declare];
                    codes.extend(section(call_exp.start, call_exp.end));
                    codes.push(Self::text_code(END_OF_LINE));
                    codes.extend(section(statement.start, call_exp.start));
                    codes.push(Self::text_code(default_name));
                    transforms.push(Transform {
                        start: statement.start,
                        end: call_exp.end,
                        codes,
                    });
                }
            }
            // A destructuring: the macro's result first, then the pattern.
            Some(name) if !is_identifier(name) => {
                transforms.push(Transform {
                    start: statement.start,
                    end: call_exp.start,
                    codes: vec![declare],
                });
                let mut codes = vec![Self::text_code(END_OF_LINE)];
                codes.extend(section(statement.start, call_exp.start));
                codes.push(Self::text_code(default_name));
                transforms.push(Self::insert(statement.end, codes));
            }
            Some(_) => {}
        }
    }

    /// The changes `generateSetupFunction` makes to `<script setup>`'s code.
    #[expect(
        clippy::too_many_lines,
        reason = "one list in the original, in the same order"
    )]
    fn setup_transforms(&mut self, ranges: &ScriptSetupRanges) -> Vec<Transform> {
        let options = self.options;
        let lib = options.lib;
        let is_ts = options.is_ts;
        let mut transforms: Vec<Transform> = Vec::new();
        let section = |generator: &Self, range: Range, features: Features| {
            generator.section_codes(Source::ScriptSetup, range.start, range.end, features)
        };

        for model in &ranges.define_model {
            if let Some(arg) = model.default_props_arg.filter(|_| !self.before_3_3_12()) {
                let mut codes = vec![Self::text_code(": ")];
                match &ranges.define_props {
                    Some(props) if props.call.call.type_arg.is_some() => {
                        codes.push(Self::text_code("__VLS_Props"));
                    }
                    props => {
                        codes.push(Self::text_code("typeof "));
                        codes.push(Self::text_code(
                            props
                                .as_ref()
                                .and_then(|props| props.call.name.clone())
                                .unwrap_or_else(|| "__VLS_props".to_owned()),
                        ));
                    }
                }
                transforms.push(Self::insert(arg.end, codes));
            }
        }
        if let Some(props) = &ranges.define_props {
            let call_exp = ranges
                .with_defaults
                .map_or(props.call.call.call_exp, |defaults| defaults.call_exp);
            self.define_with_type_transforms(
                &mut transforms,
                &props.call,
                call_exp,
                "__VLS_props",
                "__VLS_Props",
            );
        }
        if let Some(emits) = &ranges.define_emits {
            self.define_with_type_transforms(
                &mut transforms,
                &emits.call,
                emits.call.call.call_exp,
                "__VLS_emit",
                "__VLS_Emit",
            );
        }
        if let Some(slots) = &ranges.define_slots {
            self.define_with_type_transforms(
                &mut transforms,
                slots,
                slots.call.call_exp,
                "__VLS_slots",
                "__VLS_Slots",
            );
            if slots.name.is_none() && !self.before_3_3_12() {
                transforms.push(Self::insert(
                    slots.statement.end,
                    vec![Self::text_code(format!(
                        "{NEW_LINE}void __VLS_slots{END_OF_LINE}"
                    ))],
                ));
            }
        }
        if let Some(expose) = &ranges.define_expose {
            let CallRange {
                call_exp,
                arg,
                type_arg,
                ..
            } = *expose;
            if let Some(type_arg) = type_arg {
                let mut codes = vec![Self::text_code("let __VLS_exposed!: ")];
                codes.extend(section(self, type_arg, ALL));
                codes.push(Self::text_code(END_OF_LINE));
                transforms.push(Self::insert(call_exp.start, codes));
                transforms.push(Transform {
                    start: type_arg.start,
                    end: type_arg.end,
                    codes: vec![Self::text_code("typeof __VLS_exposed")],
                });
            } else if let Some(arg) = arg {
                let mut codes = vec![Self::text_code("const __VLS_exposed = ")];
                codes.extend(section(self, arg, ALL));
                codes.push(Self::text_code(END_OF_LINE));
                transforms.push(Self::insert(call_exp.start, codes));
                transforms.push(Transform {
                    start: arg.start,
                    end: arg.end,
                    codes: vec![Self::text_code("__VLS_exposed")],
                });
            } else {
                transforms.push(Self::insert(
                    call_exp.start,
                    vec![Self::text_code(format!(
                        "const __VLS_exposed = {{}}{END_OF_LINE}"
                    ))],
                ));
            }
        }
        // A cast around a call: `(call() as T)`, or a JSDoc cast.
        let cast = |transforms: &mut Vec<Transform>, call_exp: Range, type_text: &str| {
            transforms.push(Self::insert(
                call_exp.start,
                vec![Self::text_code(if is_ts {
                    "(".to_owned()
                } else {
                    format!("/** @type {{{type_text}}} */ (")
                })],
            ));
            transforms.push(Self::insert(
                call_exp.end,
                vec![Self::text_code(if is_ts {
                    format!(" as {type_text})")
                } else {
                    ")".to_owned()
                })],
            ));
        };
        if options.infer_template_dollar_attrs {
            for call in &ranges.use_attrs {
                cast(
                    &mut transforms,
                    call.call_exp,
                    "typeof __VLS_dollars.$attrs",
                );
            }
        }
        for call in &ranges.use_css_module {
            let type_text = if self.has_type("__VLS_StyleModules") {
                "__VLS_StyleModules"
            } else {
                "{}"
            };
            let call_exp = call.call_exp;
            if let Some(arg) = call.arg {
                let mut before = Vec::new();
                let mut after = Vec::new();
                if is_ts {
                    before.push(Self::text_code("("));
                    after.push(Self::text_code(format!(" as Omit<{type_text}, '$style'>[")));
                    after.extend(section(self, arg, WITHOUT_SEMANTIC));
                    after.push(Self::text_code("])"));
                } else {
                    before.push(Self::text_code(format!(
                        "/** @type {{Omit<{type_text}, '$style'>["
                    )));
                    before.extend(section(self, arg, WITHOUT_SEMANTIC));
                    before.push(Self::text_code("]} */ ("));
                    after.push(Self::text_code(")"));
                }
                transforms.push(Self::insert(call_exp.start, before));
                transforms.push(Self::insert(call_exp.end, after));
                transforms.push(Transform {
                    start: arg.start,
                    end: arg.end,
                    codes: vec![Self::text_code(self.as_type("any"))],
                });
            } else {
                transforms.push(Self::insert(
                    call_exp.start,
                    vec![Self::text_code(if is_ts {
                        "(".to_owned()
                    } else {
                        format!("/** @type {{{type_text}['$style']}} */ (")
                    })],
                ));
                let mut after = Vec::new();
                if is_ts {
                    after.push(Self::text_code(format!(" as {type_text}[")));
                    self.next_token += 1;
                    let boundary = Boundary::start(
                        &mut after,
                        self.next_token,
                        Source::ScriptSetup,
                        call.exp.start,
                        call.exp.end,
                        Features::new(Verification::On),
                    );
                    after.push(Self::text_code("'$style'"));
                    after.push(boundary.end());
                    after.push(Self::text_code("])"));
                } else {
                    after.push(Self::text_code(")"));
                }
                transforms.push(Self::insert(call_exp.end, after));
            }
        }
        if options.infer_template_dollar_slots {
            for call in &ranges.use_slots {
                cast(
                    &mut transforms,
                    call.call_exp,
                    "typeof __VLS_dollars.$slots",
                );
            }
        }
        for template_ref in &ranges.use_template_ref {
            let call = template_ref.call;
            // `Readonly<ShallowRef<__VLS_TemplateRefs['name'] | null>>`
            let ref_type = |generator: &Self| {
                let mut codes = Vec::new();
                match call.arg {
                    Some(arg) => {
                        codes.push(Self::text_code("__VLS_TemplateRefs"));
                        codes.push(Self::text_code("["));
                        codes.extend(section(generator, arg, WITHOUT_SEMANTIC));
                        codes.push(Self::text_code("]"));
                    }
                    None => codes.push(Self::text_code("unknown")),
                }
                codes
            };
            let mut before = Vec::new();
            let mut after = Vec::new();
            if is_ts {
                before.push(Self::text_code("("));
                after.push(Self::text_code(format!(
                    " as Readonly<import('{lib}').ShallowRef<"
                )));
                after.extend(ref_type(self));
                after.push(Self::text_code(" | null>>)"));
            } else {
                before.push(Self::text_code(format!(
                    "/** @type {{Readonly<import('{lib}').ShallowRef<"
                )));
                before.extend(ref_type(self));
                before.push(Self::text_code(" | null>>} */ ("));
                after.push(Self::text_code(")"));
            }
            transforms.push(Self::insert(call.call_exp.start, before));
            transforms.push(Self::insert(call.call_exp.end, after));
            if let Some(arg) = call.arg {
                transforms.push(Transform {
                    start: arg.start,
                    end: arg.end,
                    codes: vec![Self::text_code(self.as_type("any"))],
                });
            }
        }
        transforms
    }

    /// `generateSetupFunction`: `<script setup>`'s code after its imports,
    /// then what is generated from it, the template, and the component.
    fn setup_function(
        &mut self,
        ranges: &ScriptSetupRanges,
        template_self_type: Option<&str>,
        output: Option<Output>,
    ) {
        let mut transforms = self.setup_transforms(ranges);
        // `generateCodeWithTransforms`
        transforms.sort_by_key(|transform| transform.start);
        let mut start = Self::import_section_end(ranges);
        let end = self.options.script_setup.map_or(0, str::len);
        for transform in transforms {
            self.section(Source::ScriptSetup, start, transform.start, ALL);
            self.out.extend(transform.codes);
            start = transform.end;
        }
        self.section(Source::ScriptSetup, start, end, ALL);

        if self.before_3_3_12() {
            self.macros_3_3_11();
        }
        self.models(ranges);
        self.public_props(ranges);
        self.template(template_self_type);

        let Some(output) = output else { return };
        let write_output = |generator: &mut Self| match output {
            Output::Return => generator.text("return "),
            Output::ExportDeclare => {
                generator.export_declare_equal(Source::ScriptSetup, "__VLS_export");
            }
        };
        if self.has_slots_type() {
            self.text("const __VLS_base = ");
            self.component(ranges);
            self.text(END_OF_LINE);
            write_output(self);
            let with_slots = self.local_type(LocalType::WithSlots);
            self.text(self.as_type(&format!("{with_slots}<typeof __VLS_base, __VLS_Slots>")));
            self.text(END_OF_LINE);
        } else {
            write_output(self);
            self.component(ranges);
            self.text(END_OF_LINE);
        }
    }

    /// `generateMacros`: the macros are imported, so that a JavaScript file
    /// resolves them too.
    fn macros(&mut self) {
        let options = self.options;
        let (Some(_), Some(ranges)) = (options.script_setup, options.script_setup_ranges) else {
            return;
        };
        if options.target < 3.3 {
            return;
        }
        let mut used: Vec<(&str, &str)> = Vec::new();
        let named = [
            (
                "defineProps",
                ranges
                    .define_props
                    .as_ref()
                    .map(|props| props.call.call.exp),
            ),
            (
                "defineEmits",
                ranges
                    .define_emits
                    .as_ref()
                    .map(|emits| emits.call.call.exp),
            ),
            (
                "defineSlots",
                ranges.define_slots.as_ref().map(|slots| slots.call.exp),
            ),
            (
                "defineExpose",
                ranges.define_expose.map(|expose| expose.exp),
            ),
            (
                "withDefaults",
                ranges.with_defaults.map(|defaults| defaults.exp),
            ),
        ];
        for (canonical, exp) in named {
            if let Some(exp) = exp {
                used.push((canonical, self.range_text(exp)));
            }
        }
        if !ranges.define_model.is_empty() {
            used.push(("defineModel", "defineModel"));
        }
        if ranges.define_options.is_some() {
            used.push(("defineOptions", "defineOptions"));
        }
        let to_import: Vec<_> = used
            .into_iter()
            .filter(|(_, alias)| !options.setup_bindings.contains(*alias))
            .collect();
        if to_import.is_empty() {
            return;
        }
        if options.core == CoreVersion::V3_3_12 {
            // 3.3.12 imports them.
            self.text("import { ");
            for (canonical, alias) in to_import {
                self.text(if alias == canonical {
                    format!("{canonical}, ")
                } else {
                    format!("{canonical} as {alias}, ")
                });
            }
            self.text(format!("}} from '{}'{END_OF_LINE}", options.lib));
            return;
        }
        self.text(if options.core.is_3_3_14() {
            format!("{NEW_LINE}const {{ ")
        } else {
            "const { ".to_owned()
        });
        for (canonical, alias) in to_import {
            self.text(if alias == canonical {
                format!("{canonical}, ")
            } else {
                format!("{canonical}: {alias}, ")
            });
        }
        self.text("} = ");
        self.text(self.as_type(&format!("typeof import('{}')", options.lib)));
        self.text(END_OF_LINE);
    }

    /// `generatePublicProps`.
    fn public_props(&mut self, ranges: &ScriptSetupRanges) {
        let options = self.options;
        let has_type_arg = ranges
            .define_props
            .as_ref()
            .is_some_and(|props| props.call.call.type_arg.is_some());
        if has_type_arg && let Some(arg) = ranges.with_defaults.and_then(|defaults| defaults.arg) {
            self.text("const __VLS_defaults = ");
            self.section(Source::ScriptSetup, arg.start, arg.end, NAVIGATION);
            self.text(END_OF_LINE);
            if !self.before_3_3_12() {
                self.text(format!("void __VLS_defaults{END_OF_LINE}"));
            }
        }
        let mut prop_types: Vec<String> = Vec::new();
        if options.jsx_slots && self.has_slots_type() {
            let props_children = self.local_type(LocalType::PropsChildren);
            prop_types.push(format!("{props_children}<__VLS_Slots>"));
        }
        if has_type_arg {
            prop_types.push("__VLS_Props".to_owned());
        }
        if !ranges.define_model.is_empty() {
            prop_types.push("__VLS_ModelProps".to_owned());
        }
        let has_arg = ranges
            .define_props
            .as_ref()
            .is_some_and(|props| props.call.call.arg.is_some());
        let used = self.before_3_3_12()
            || options.script_setup_generic.is_some()
            || options.target < 3.6
            || (options.target >= 3.5 && !has_arg);
        if !prop_types.is_empty() && used {
            self.type_alias("__VLS_PublicProps", |generator| {
                generator.text(prop_types.join(" & "));
            });
            if !self.generated_types.contains(&"__VLS_PublicProps") {
                self.generated_types.push("__VLS_PublicProps");
            }
        }
    }

    /// `generateModels`: the props and events of each `defineModel`.
    fn models(&mut self, ranges: &ScriptSetupRanges) {
        if ranges.define_model.is_empty() {
            return;
        }
        let mut default_codes: Vec<String> = Vec::new();
        let mut prop_codes: Vec<Vec<Code>> = Vec::new();
        let mut emit_codes: Vec<Vec<Code>> = Vec::new();
        for model in &ranges.define_model {
            let prop_name = match model.name {
                // The name, without the quotes of the string it is.
                Some(name) => {
                    let text = self.range_text(name);
                    let mut chars = text.chars();
                    chars.next();
                    chars.next_back();
                    js::camelize(chars.as_str())
                }
                None => "modelValue".to_owned(),
            };
            let model_type = if let Some(type_range) = model.r#type {
                self.range_text(type_range).to_owned()
            } else if let (Some(_), Some(local_name)) = (model.runtime_type, model.local_name) {
                format!("typeof {}['value']", self.range_text(local_name))
            } else if model.default_value.is_some() && !prop_name.is_empty() {
                format!("typeof __VLS_defaultModels['{prop_name}']")
            } else {
                "any".to_owned()
            };
            if let Some(default_value) = model.default_value {
                default_codes.push(format!(
                    "'{prop_name}': {},{NEW_LINE}",
                    self.range_text(default_value)
                ));
            }
            prop_codes.push(self.model_prop(model, &prop_name, &model_type));
            // `generateModelEmit`
            let mut emit = vec![
                Self::text_code(format!("'update:{prop_name}': [value: ")),
                Self::text_code(model_type),
            ];
            if !model.required && model.default_value.is_none() {
                emit.push(Self::text_code(" | undefined"));
            }
            emit.push(Self::text_code(format!("]{END_OF_LINE}")));
            emit_codes.push(emit);
        }
        if !default_codes.is_empty() {
            self.text(format!("const __VLS_defaultModels = {{{NEW_LINE}"));
            for code in default_codes {
                self.text(code);
            }
            self.text(format!("}}{END_OF_LINE}"));
            if !self.before_3_3_12() {
                self.text(format!("void __VLS_defaultModels{END_OF_LINE}"));
            }
        }
        self.type_alias("__VLS_ModelProps", |generator| {
            generator.text(format!("{{{NEW_LINE}"));
            generator.out.extend(prop_codes.into_iter().flatten());
            generator.text("}");
        });
        self.type_alias("__VLS_ModelEmit", |generator| {
            generator.text(format!("{{{NEW_LINE}"));
            generator.out.extend(emit_codes.into_iter().flatten());
            generator.text("}");
        });
        // Not `defineModel<...>()`, which a JavaScript file can't have.
        self.typed_var(
            "let",
            "__VLS_modelEmit",
            "__VLS_ShortEmits<__VLS_ModelEmit>",
        );
    }

    /// `generateModelProp`.
    fn model_prop(&mut self, model: &DefineModel, prop_name: &str, model_type: &str) -> Vec<Code> {
        let mut codes = Vec::new();
        // In JavaScript the type is inside a JSDoc comment, which a user's
        // comment could end.
        if let Some(comments) = model.comments.filter(|_| self.options.is_ts) {
            codes.push(Self::text_code(self.range_text(comments)));
            codes.push(Self::text_code(NEW_LINE));
        }
        match model.name {
            Some(name) => {
                self.next_token += 1;
                generate_camelized(
                    &mut codes,
                    self.next_token,
                    self.range_text(name),
                    Source::ScriptSetup,
                    name.start,
                    NAVIGATION,
                );
            }
            None => codes.push(Self::text_code(prop_name)),
        }
        codes.push(Self::text_code(if model.required { ": " } else { "?: " }));
        codes.push(Self::text_code(model_type));
        codes.push(Self::text_code(END_OF_LINE));
        if let Some(modifier_type) = model.modifier_type {
            let modifier_name = if prop_name == "modelValue" {
                "model"
            } else {
                prop_name
            };
            codes.push(Self::text_code(format!(
                "'{modifier_name}Modifiers'?: Partial<Record<{}, true>>{END_OF_LINE}",
                self.range_text(modifier_type)
            )));
        }
        codes
    }

    // ---- `component.ts` ----

    /// `generateComponent`: the component `<script setup>` defines.
    fn component(&mut self, ranges: &ScriptSetupRanges) {
        let options = self.options;
        self.text(format!(
            "(await import('{}')).defineComponent({{{NEW_LINE}",
            options.lib
        ));
        if ranges.define_expose.is_some() {
            self.text(format!("setup: () => __VLS_exposed,{NEW_LINE}"));
        }
        let has_emits_option = self.emits_option(ranges);
        self.props_option(ranges, has_emits_option);
        if options.target >= 3.5
            && options.infer_component_dollar_refs
            && self.has_type("__VLS_TemplateRefs")
        {
            self.text(format!(
                "__typeRefs: {},{NEW_LINE}",
                self.as_type("__VLS_TemplateRefs")
            ));
        }
        if options.target >= 3.5
            && options.infer_component_dollar_el
            && self.has_type("__VLS_RootEl")
        {
            self.text(format!(
                "__typeEl: {},{NEW_LINE}",
                self.as_type("__VLS_RootEl")
            ));
        }
        self.text("})");
    }

    /// `generateEmitsOption`. Returns whether it wrote one.
    fn emits_option(&mut self, ranges: &ScriptSetupRanges) -> bool {
        let options = self.options;
        let emits = ranges.define_emits.as_ref();
        let mut type_codes: Vec<&str> = Vec::new();
        if options.target >= 3.5 && !emits.is_some_and(|emits| emits.has_union_type_arg) {
            if !ranges.define_model.is_empty() {
                type_codes.push("__VLS_ModelEmit");
            }
            if emits.is_some_and(|emits| emits.call.call.type_arg.is_some()) {
                type_codes.push("__VLS_Emit");
            }
        }
        if !type_codes.is_empty() {
            self.text(format!(
                "__typeEmits: {},{NEW_LINE}",
                self.as_type(&type_codes.join(" & "))
            ));
            return true;
        }
        let mut runtime_codes: Vec<Code> = Vec::new();
        if !ranges.define_model.is_empty() {
            runtime_codes.push(Self::text_code(
                self.as_type("__VLS_NormalizeEmits<typeof __VLS_modelEmit>"),
            ));
        }
        if let Some(emits) = emits {
            runtime_codes.push(Self::text_code(self.as_type(&format!(
                "__VLS_NormalizeEmits<typeof {}>",
                emits.call.name.as_deref().unwrap_or("__VLS_emit")
            ))));
        }
        if runtime_codes.is_empty() {
            return false;
        }
        self.text("emits: ");
        self.spread_merge(runtime_codes);
        self.text(format!(",{NEW_LINE}"));
        true
    }

    /// `generatePropsOption`.
    fn props_option(&mut self, ranges: &ScriptSetupRanges, has_emits_option: bool) {
        let options = self.options;
        let props_arg = ranges
            .define_props
            .as_ref()
            .and_then(|props| props.call.call.arg);
        let attrs_type = if has_emits_option {
            "Omit<__VLS_InheritedAttrs, keyof __VLS_EmitProps>"
        } else {
            "__VLS_InheritedAttrs"
        };
        let has_public_props = self.generated_types.contains(&"__VLS_PublicProps");

        // `generateTypePropsOption`
        let mut type_codes: Vec<Code> = Vec::new();
        if options.target >= 3.5 && props_arg.is_none() {
            if self.has_type("__VLS_InheritedAttrs") {
                type_codes.push(Self::text_code(self.as_type(attrs_type)));
            }
            if has_public_props {
                type_codes.push(Self::text_code(self.as_type("__VLS_PublicProps")));
            }
        }
        // `generateRuntimePropsOption`
        let mut runtime_codes: Vec<Code> = Vec::new();
        if ranges.with_defaults.is_some() || type_codes.is_empty() {
            if self.has_type("__VLS_InheritedAttrs") {
                let to_option = self.local_type(LocalType::TypePropsToOption);
                let omit_index = self.local_type(LocalType::OmitIndexSignature);
                runtime_codes.push(Self::text_code(self.as_type(&format!(
                    "{to_option}<__VLS_PickNotAny<{omit_index}<{attrs_type}>, {{}}>>"
                ))));
            }
            if has_public_props && options.target < 3.6 {
                let to_option = self.local_type(LocalType::TypePropsToOption);
                let mut props_type = format!("{to_option}<__VLS_PublicProps>");
                if ranges
                    .with_defaults
                    .is_some_and(|defaults| defaults.arg.is_some())
                {
                    let with_defaults = self.local_type(LocalType::WithDefaults);
                    props_type = format!("{with_defaults}<{props_type}, typeof __VLS_defaults>");
                }
                runtime_codes.push(Self::text_code(self.as_type(&props_type)));
            }
            if let Some(arg) = props_arg {
                runtime_codes.extend(self.section_codes(
                    Source::ScriptSetup,
                    arg.start,
                    arg.end,
                    NAVIGATION,
                ));
            }
        }

        if !type_codes.is_empty() {
            if options.target >= 3.6
                && ranges
                    .with_defaults
                    .is_some_and(|defaults| defaults.arg.is_some())
            {
                self.text(format!("__defaults: __VLS_defaults,{NEW_LINE}"));
            }
            self.text("__typeProps: ");
            self.spread_merge(type_codes);
            self.text(format!(",{NEW_LINE}"));
        }
        if !runtime_codes.is_empty() {
            self.text("props: ");
            self.spread_merge(runtime_codes);
            self.text(format!(",{NEW_LINE}"));
        }
    }

    // ---- `template.ts` ----

    /// `generateTemplate`: the template's code in a function of its own, and
    /// the values and types it reads and gives back.
    fn template(&mut self, self_type: Option<&str>) {
        let options = self.options;
        if self.before_3_3_12() {
            // Before 3.3.12: the bindings the template reads, as a type of
            // the context; the template's code follows as it is.
            self.setup_exposed_3_3_11();
            self.template_ctx(self_type);
            self.template_components();
            self.template_directives();
            let codes = std::mem::take(&mut self.template_and_style_codes);
            self.out.extend(codes);
            return;
        }
        self.template_ctx(self_type);
        self.template_components();
        self.template_directives();
        self.text(format!(
            "void __VLS_ctx, __VLS_components, __VLS_intrinsics, __VLS_directives{END_OF_LINE}"
        ));

        // The types setup and the export need from the template come out of
        // its function's return type.
        if !options.core.is_3_3_14() {
            // Before 3.3.14 the template's code is here as it is.
            let brand = ref_brand_argument(options.lib, options.is_ts);
            for name in options.dot_value_bindings {
                self.text(format!("// @ts-ignore{NEW_LINE}"));
                self.text(format!("__VLS_withDotValue({name}, {brand}){END_OF_LINE}"));
            }
            let codes = std::mem::take(&mut self.template_and_style_codes);
            self.out.extend(codes);
            return;
        }
        let has_define_slots = options
            .script_setup_ranges
            .is_some_and(|ranges| ranges.define_slots.is_some());
        let exported_types: Vec<&str> = [
            "__VLS_Slots",
            "__VLS_InheritedAttrs",
            "__VLS_TemplateRefs",
            "__VLS_RootEl",
            "__VLS_StyleModules",
        ]
        .into_iter()
        .filter(|name| self.has_type(name) && !(*name == "__VLS_Slots" && has_define_slots))
        .collect();
        self.text(format!("const __VLS_template = () => {{{NEW_LINE}"));
        let brand = ref_brand_argument(options.lib, options.is_ts);
        for name in options.dot_value_bindings {
            self.text(format!("__VLS_withDotValue({name}, {brand}){END_OF_LINE}"));
        }
        let codes = std::mem::take(&mut self.template_and_style_codes);
        self.out.extend(codes);
        self.text(format!("return {{{NEW_LINE}"));
        for name in &exported_types {
            self.text(format!("{name}: {},{NEW_LINE}", self.as_type(name)));
        }
        let has_dollars = self.has_type("__VLS_dollars");
        if has_dollars {
            self.text(format!("__VLS_dollars,{NEW_LINE}"));
        }
        self.text(format!("}}{END_OF_LINE}"));
        self.text(format!("}}{END_OF_LINE}"));

        let mut used = vec!["__VLS_template".to_owned()];
        let return_type = "ReturnType<typeof __VLS_template>";
        for name in &exported_types {
            self.type_alias(name, |generator| {
                generator.text(format!("{return_type}['{name}']"));
            });
        }
        if !exported_types.is_empty() {
            used.push(self.as_type(&format!("[{}]", exported_types.join(", "))));
        }
        if has_dollars {
            self.typed_var(
                "let",
                "__VLS_dollars",
                &format!("{return_type}['__VLS_dollars']"),
            );
            used.push("__VLS_dollars".to_owned());
        }
        self.text(format!("void [{}]{END_OF_LINE}", used.join(", ")));
    }

    /// `generateTemplateCtx`: what a template expression reads names from.
    fn template_ctx(&mut self, self_type: Option<&str>) {
        let options = self.options;
        let lib = options.lib;
        let ranges = options.script_setup_ranges;
        let mut exps: Vec<Code> = Vec::new();
        let mut emit_types: Vec<String> = Vec::new();
        let mut prop_types: Vec<String> = Vec::new();

        exps.push(Self::text_code(match self_type {
            Some(self_type) => self.as_type(&format!(
                "InstanceType<__VLS_PickNotAny<typeof {self_type}, new () => {{}}>>"
            )),
            None => self.as_type(&format!("import('{lib}').ComponentPublicInstance")),
        }));
        if self.has_type("__VLS_StyleModules") {
            exps.push(Self::text_code(self.as_type("__VLS_StyleModules")));
        }
        let has_models = ranges.is_some_and(|ranges| !ranges.define_model.is_empty());
        if let Some(emits) = ranges.and_then(|ranges| ranges.define_emits.as_ref()) {
            emit_types.push(format!(
                "typeof {}",
                emits.call.name.as_deref().unwrap_or("__VLS_emit")
            ));
        }
        if has_models {
            emit_types.push("typeof __VLS_modelEmit".to_owned());
        }
        if !emit_types.is_empty() {
            let emits = emit_types.join(" & ");
            self.type_alias("__VLS_EmitProps", |generator| {
                generator.text(format!("__VLS_EmitsToProps<__VLS_NormalizeEmits<{emits}>>"));
            });
            exps.push(Self::text_code(
                self.as_type(&format!("{{ $emit: {emits} }}")),
            ));
        }
        if let Some(props) = ranges.and_then(|ranges| ranges.define_props.as_ref()) {
            prop_types.push(format!(
                "typeof {}",
                props.call.name.as_deref().unwrap_or("__VLS_props")
            ));
        }
        if has_models {
            prop_types.push("__VLS_ModelProps".to_owned());
        }
        if !emit_types.is_empty() {
            prop_types.push("__VLS_EmitProps".to_owned());
        }
        if !prop_types.is_empty() {
            let props = prop_types.join(" & ");
            exps.push(Self::text_code(
                self.as_type(&format!("{{ $props: {props} }}")),
            ));
            exps.push(Self::text_code(self.as_type(&props)));
        }
        if self.generated_types.contains(&"__VLS_SetupExposed") {
            exps.push(Self::text_code(self.as_type("__VLS_SetupExposed")));
        }
        self.text("const __VLS_ctx = ");
        self.spread_merge(exps);
        self.text(END_OF_LINE);
    }

    /// `generateMacros` before 3.3.12: every macro declared, apart from a
    /// name the template reads from `<script setup>`.
    fn macros_3_3_11(&mut self) {
        let options = self.options;
        if options.target < 3.3 {
            return;
        }
        self.text(format!("// @ts-ignore{NEW_LINE}"));
        self.text("declare const { ");
        for name in [
            "defineProps",
            "defineSlots",
            "defineEmits",
            "defineExpose",
            "defineModel",
            "defineOptions",
            "withDefaults",
        ] {
            if !options.exposed.iter().any(|exposed| exposed == name) {
                self.text(format!("{name}, "));
            }
        }
        self.text(format!("}}: typeof import('{}'){END_OF_LINE}", options.lib));
    }

    /// `generateSetupExposed` before 3.3.12: the type of the bindings the
    /// template reads from `<script setup>`.
    fn setup_exposed_3_3_11(&mut self) {
        let options = self.options;
        if options.exposed.is_empty() {
            return;
        }
        self.generated_types.push("__VLS_SetupExposed");
        self.text(format!(
            "type __VLS_SetupExposed = import('{}').ShallowUnwrapRef<{{{NEW_LINE}",
            options.lib
        ));
        for name in options.exposed {
            self.text(format!("{name}: typeof "));
            self.text(name.clone());
            self.text(END_OF_LINE);
        }
        self.text(format!("}}>{END_OF_LINE}"));
    }

    /// `generateExposedType`: the setup bindings a template can use as
    /// components or directives.
    fn exposed_type(&self, bindings: &[String]) -> String {
        let members: Vec<String> = bindings
            .iter()
            .map(|name| format!("{name}: typeof {name};"))
            .collect();
        format!(
            "import('{}').ShallowUnwrapRef<{{\n{}\n}}>",
            self.options.lib,
            members.join("\n")
        )
    }

    /// `generateTemplateComponents`.
    fn template_components(&mut self) {
        let options = self.options;
        let lib = options.lib;
        let mut types: Vec<String> = Vec::new();
        if self.before_3_3_12() {
            if self.generated_types.contains(&"__VLS_SetupExposed") {
                types.push("__VLS_SetupExposed".to_owned());
            }
        } else if !options.local_components.is_empty() {
            types.push(self.exposed_type(options.local_components));
        }
        let components = options
            .script
            .and(options.script_ranges)
            .and_then(|ranges| ranges.export_default.as_ref())
            .and_then(|export| export.options.as_ref())
            .and_then(|options| options.components);
        if let Some(components) = components {
            self.text("const __VLS_componentsOption = ");
            self.section(Source::Script, components.start, components.end, NAVIGATION);
            self.text(END_OF_LINE);
            types.push("typeof __VLS_componentsOption".to_owned());
        }
        let local = if types.is_empty() {
            "{}".to_owned()
        } else {
            types.join(" & ")
        };
        self.type_alias("__VLS_LocalComponents", |generator| generator.text(local));
        let global = if options.target >= 3.5 {
            format!("import('{lib}').GlobalComponents")
        } else {
            format!(
                "import('{lib}').GlobalComponents & Pick<typeof import('{lib}'), 'Transition' | 'TransitionGroup' | 'KeepAlive' | 'Suspense' | 'Teleport'>"
            )
        };
        self.type_alias("__VLS_GlobalComponents", |generator| generator.text(global));
        self.typed_var(
            "let",
            "__VLS_components",
            "__VLS_LocalComponents & __VLS_GlobalComponents",
        );
        let intrinsics = if options.target >= 3.3 {
            format!("import('{lib}/jsx-runtime').JSX.IntrinsicElements")
        } else {
            "globalThis.JSX.IntrinsicElements".to_owned()
        };
        self.typed_var("let", "__VLS_intrinsics", &intrinsics);
    }

    /// `generateTemplateDirectives`.
    fn template_directives(&mut self) {
        let options = self.options;
        let mut types: Vec<String> = Vec::new();
        if self.before_3_3_12() {
            if self.generated_types.contains(&"__VLS_SetupExposed") {
                types.push("__VLS_SetupExposed".to_owned());
            }
        } else if !options.local_directives.is_empty() {
            types.push(self.exposed_type(options.local_directives));
        }
        let directives = options
            .script
            .and(options.script_ranges)
            .and_then(|ranges| ranges.export_default.as_ref())
            .and_then(|export| export.options.as_ref())
            .and_then(|options| options.directives);
        if let Some(directives) = directives {
            self.text("const __VLS_directivesOption = ");
            self.section(Source::Script, directives.start, directives.end, NAVIGATION);
            self.text(END_OF_LINE);
            types.push("__VLS_ResolveDirectives<typeof __VLS_directivesOption>".to_owned());
        }
        let local = if types.is_empty() {
            "{}".to_owned()
        } else {
            types.join(" & ")
        };
        self.type_alias("__VLS_LocalDirectives", |generator| generator.text(local));
        self.typed_var(
            "let",
            "__VLS_directives",
            &format!(
                "__VLS_LocalDirectives & import('{}').GlobalDirectives",
                options.lib
            ),
        );
    }
}
