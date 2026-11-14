//! The helper types the script codegen declares when it uses them
//! (`codegen/localTypes.ts`, 3.3.14), in TypeScript and as `JSDoc` typedefs.
//! Generated from the original; do not edit by hand. `\u{1}` stands for the
//! library's name (`vueCompilerOptions.lib`).

pub const PRETTIFY_LOCAL_TS: &str = "type __VLS_PrettifyLocal<T> = (T extends any ? { [K in keyof T]: T[K]; } : { [K in keyof T as K]: T[K]; }) & {};\n";
pub const PRETTIFY_LOCAL_JS: &str = "/**\n * @template T\n * @typedef {(T extends any ? { [K in keyof T]: T[K]; } : { [K in keyof T as K]: T[K]; }) & {}} __VLS_PrettifyLocal\n */\n";
pub const WITH_DEFAULTS_TS: &str = "type __VLS_WithDefaults<P, D> = {\n\t[K in keyof P & string]: K extends keyof D\n\t\t? __VLS_PrettifyLocal<P[K] & { default: D[K] }>\n\t\t: P[K]\n};\n";
pub const WITH_DEFAULTS_JS: &str = "/**\n * @template P, D\n * @typedef {{ [K in keyof Pick<P, keyof P>]: K extends keyof D ? __VLS_PrettifyLocal<P[K] & { default: D[K] }> : P[K] }} __VLS_WithDefaults\n */\n";
pub const WITH_SLOTS_TS: &str =
    "type __VLS_WithSlots<T, S> = T & {\n\tnew(): {\n\t\t$slots: S;\n\t}\n};\n";
pub const WITH_SLOTS_JS: &str =
    "/**\n * @template T, S\n * @typedef {T & { new(): { $slots: S; } }} __VLS_WithSlots\n */\n";
pub const PROPS_CHILDREN_TS: &str = "type __VLS_PropsChildren<S> = {\n\t[K in keyof (\n\t\tboolean extends (\n\t\t\t// @ts-ignore\n\t\t\tJSX.ElementChildrenAttribute extends never\n\t\t\t\t? true\n\t\t\t\t: false\n\t\t)\n\t\t\t? never\n\t\t\t// @ts-ignore\n\t\t\t: JSX.ElementChildrenAttribute\n\t)]?: S;\n};\n";
pub const PROPS_CHILDREN_JS: &str = "// @ts-ignore\n/** @template S @typedef {{ [K in keyof (boolean extends (JSX.ElementChildrenAttribute extends never ? true : false) ? never : JSX.ElementChildrenAttribute)]?: S; }} __VLS_PropsChildren */\n";
pub const TYPE_PROPS_TO_OPTION_TS: &str = "type __VLS_TypePropsToOption<T> = {\n\t[K in keyof T & string]-?: {} extends Pick<T, K>\n\t\t? { type: import('\u{1}').PropType<Required<T>[K]> }\n\t\t: { type: import('\u{1}').PropType<T[K]>, required: true }\n};\n";
pub const TYPE_PROPS_TO_OPTION_JS: &str = "/**\n * @template T\n * @typedef {{ [K in keyof T]-?: {} extends Pick<T, K> ? { type: import('\u{1}').PropType<Required<T>[K]> } : { type: import('\u{1}').PropType<T[K]>, required: true } }} __VLS_TypePropsToOption\n */\n";
pub const OMIT_INDEX_SIGNATURE_TS: &str = "type __VLS_OmitIndexSignature<T> = { [K in keyof T as {} extends Record<K, unknown> ? never : K]: T[K]; };\n";
pub const OMIT_INDEX_SIGNATURE_JS: &str = "/**\n * @template T\n * @typedef {{ [K in keyof T as {} extends Record<K, unknown> ? never : K]: T[K] }} __VLS_OmitIndexSignature\n */\n";
