#!/usr/bin/env bun
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Generates .vue files whose <script> blocks are valid but unusual: every
 * form of import, declaration and export, the macros in every position, and
 * comments wherever TypeScript attaches them. For the script analysis, which
 * stress.ts's broken scripts never reach. Deterministic.
 *
 *   bun native/oracle/stress-script.ts <out-dir> [--count=3000] [--seed=1]
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/stress-script.ts <out-dir> [--count=3000] [--seed=1]');
const option = (name: string, fallback: string) =>
	process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=')[1] ?? fallback;
const count = Number(option('count', '3000'));

let state = Number(option('seed', '1')) >>> 0;
const random = () => {
	state = (state + 0x6d2b79f5) >>> 0;
	let value = state;
	value = Math.imul(value ^ (value >>> 15), value | 1);
	value ^= value + Math.imul(value ^ (value >>> 7), value | 61);

	return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
};
const below = (limit: number) => Math.floor(random() * limit);
const pick = <T>(items: readonly T[]): T => items[below(items.length)] as T;
const chance = (probability: number) => random() < probability;

let counter = 0;
const fresh = (prefix = 'v') => `${prefix}${(counter += 1)}`;
const NAMES = ['a', 'b', 'props', 'emit', 'slots', 'model', 'Foo', 'Bar', 'é', '$x', '_y'];
const name = () => (chance(0.5) ? fresh() : pick(NAMES));
const COMMENTS = [
	'// line\n', '/* block */ ', '/* block */\n', '/** doc */\n', '/**\n * doc\n */\n', '// @ts-check\n', '// @ts-nocheck\n', '//@ts-check extra\n',
	'/// <reference types="node" />\n', '// é😀\n', '/* é😀 */\n', '/* a */ /* b */\n', '// a\n// b\n', '\n', '\n\n', ' ', '\t', '', '', ''
];
const comment = () => pick(COMMENTS);
const TYPES = ['string', 'number', '{ a: string; b?: number }', 'Foo', 'string[]', "'a' | 'b'", 'T', '{ (e: "a" | "b"): void }', '{ (e: "a", v: number): void; (e: "b"): void }', '{ a: [x: number] }'];
const type = () => pick(TYPES);
const VALUES = ['1', "'s'", '`t`', '{}', '[]', '{ a: 1 }', 'undefined', 'null', 'true', '() => 1', 'function () {}', 'x as any', 'foo!', '(1)', '`a${1}b`'];
const value = () => pick(VALUES);
const OBJECTS = [
	'{}', "{ type: String }", "{ type: String, default: 'a' }", '{ default: () => 1 }', '{ default: (props) => props.a }', '{ default: (props: any) => 1 }',
	'{ default(props) { return 1 } }', '{ default(this: any, props) { return 1 } }', '{ default(...args) { return 1 } }', '{ required: true }',
	'{ required: false, type: [String, Number] }', "{ 'type': String }", '{ ["type"]: String }', '{ type }', '{ ...rest }', '{ get default() { return 1 } }',
	'{ default: function (props) {} }', '{ default: async (props, extra) => 1 }', '{ type: String, required: true, default: 1 }'
];
const MACROS = [
	() => `defineProps<${type()}>()`,
	() => `defineProps(${pick(['', "['a', 'b']", '{ a: String }', '{ a: { type: String } }'])})`,
	() => `withDefaults(defineProps<${type()}>(), ${pick(['{}', '{ a: 1 }', "{ a: 'x', b: () => [] }"])})`,
	() => `withDefaults(defineProps<${type()}>())`,
	() => `defineEmits<${type()}>()`,
	() => `defineEmits(${pick(['', "['a']", '{ a: null }'])})`,
	() => `defineSlots<${type()}>()`,
	() => `defineExpose(${pick(['', '{}', '{ a }'])})`,
	() => `defineModel${chance(0.5) ? `<${type()}${chance(0.3) ? `, ${type()}` : ''}>` : ''}(${pick(['', "'name'", '`name`', pick(OBJECTS), `'name', ${pick(OBJECTS)}`, 'name', 'name, opts', '...args'])})`,
	() => `defineOptions(${pick(['{}', "{ name: 'Foo' }", '{ inheritAttrs: false }', "{ name: `Foo`, inheritAttrs: true }", "{ 'name': 'x' }", 'opts', ''])})`,
	() => `useAttrs()`,
	() => `useSlots()`,
	() => `useCssModule(${pick(['', "'x'"])})`,
	() => `useTemplateRef${chance(0.3) ? `<${type()}>` : ''}(${pick(["'ref'", 'key', ''])})`,
	() => `templateRef('x')`,
	() => `other(${value()})`
];
const macro = () => pick(MACROS)();
const wrapped = (call: string) =>
	pick([call, call, call, `(${call})`, `${call} as any`, `${call}!`, `await ${call}`, `[${call}]`, `{ a: ${call} }`, `${call}.value`, `fn(${call})`, `${call} satisfies unknown`, `cond ? ${call} : 1`, `(() => ${call})()`, `function () { ${call} }`, `{ m() { ${call} } }`, `class { p = ${call} }`, `class { m() { ${call} } }`]);
const PATTERNS = [
	() => name(),
	() => name(),
	() => `{ ${name()} }`,
	() => `{ ${name()}, ${name()} = ${value()}, ...${name()} }`,
	() => `{ a: ${name()}, b: { ${name()} } = {} }`,
	() => `{ a: [${name()}, , ${name()} = 1, ...${name()}] }`,
	() => `[${name()}, ${name()}]`,
	() => `{ ['x']: ${name()} = ${value()} }`
];
const pattern = () => pick(PATTERNS)();
const IMPORTS = [
	() => `import ${name()} from './${name()}.vue'`,
	() => `import ${name()} from "./x"`,
	() => `import ${name()}, { ${name()}, type ${name()}, b as ${name()} } from 'y'`,
	() => `import { default as ${name()} } from './c.vue'`,
	() => `import { default as ${name()} } from 'z'`,
	() => `import * as ${name()} from 'vue'`,
	() => `import type { ${name()} } from 'vue'`,
	() => `import type ${name()} from './t.vue'`,
	() => `import 'side-effect'`,
	() => `import {} from 'empty'`,
	() => `export { x } from 'x'`,
	() => `export * from 'x'`,
	() => `export type ${fresh('T')} = ${type()}`,
	() => `export interface ${fresh('I')} { a: string }`,
	() => `import ${name()} = require('x')`
];
const STATEMENTS = [
	() => `${pick(['const', 'let', 'var'])} ${pattern()} = ${wrapped(macro())}`,
	() => `${pick(['const', 'let', 'var'])} ${pattern()} = ${wrapped(macro())}`,
	() => `${wrapped(macro())}`,
	() => `const ${name()} = ${value()}, ${name()} = ${wrapped(macro())}`,
	() => `const ${name()}: ${type()} = ${macro()}`,
	() => `export const ${name()} = ${macro()}`,
	() => `declare const ${name()}: ${type()}`,
	() => `function ${name()}() { ${macro()} }`,
	() => `async function* ${name()}() {}`,
	() => `class ${name()} { static { ${macro()} } }`,
	() => `enum ${fresh('E')} { A, B }`,
	() => `type ${fresh('T')} = ${type()}`,
	() => `interface ${fresh('I')} { a: string }`,
	() => `if (cond) { ${macro()} }`,
	() => `if (cond) ${macro()}; else { const ${name()} = ${macro()} }`,
	() => `for (const ${name()} of list) { ${macro()} }`,
	() => `{ const ${name()} = ${macro()} }`,
	() => `try { ${macro()} } catch { ${macro()} } finally { ${macro()} }`,
	() => `label: ${macro()}`,
	() => `;`,
	() => `declare function ${name()}(): void`,
	() => `namespace ${fresh('N')} { export const a = 1 }`
];
const statement = () => pick(STATEMENTS)();
const separator = () => pick([';\n', '\n', ';', ' ;\n\n', '\n\n', '; // trailing\n', ' /* trailing */\n', ';\n']);
const body = (most: number) => {
	let text = chance(0.3) ? comment() : '';
	const imports = below(4);
	for (let index = 0; index < imports; index += 1) text += `${comment()}${pick(IMPORTS)()}${separator()}`;
	const statements = below(most + 1);
	for (let index = 0; index < statements; index += 1) text += `${comment()}${chance(0.1) ? pick(IMPORTS)() : statement()}${separator()}`;

	return text + (chance(0.3) ? comment() : '');
};
const EXPORTS = [
	() => `export default {}`,
	() => `export default { name: 'Foo', components: { A }, directives: { vFoo }, inheritAttrs: false }`,
	() => `export default defineComponent({ name: \`Foo\`, components: {}, inheritAttrs: true as boolean })`,
	() => `export default (defineComponent({ ['name']: 'x' }) as any)!`,
	() => `export default <any>{ name: "Bar", props: { a: String } } satisfies object`,
	() => `export default defineComponent(options)`,
	() => `export default defineComponent()`,
	() => `export default defineComponent?.({ name: 'q' })`,
	() => `export default function () {}`,
	() => `export default function named() {}`,
	() => `export default class {}`,
	() => `export default class Named {}`,
	() => `export default interface Shape { a: string }`,
	() => `export default (function () {})`,
	() => `export default async () => {}`,
	() => `export default foo`,
	() => `export = { name: 'x' }`,
	() => `export default { name, components, 'directives': {}, get inheritAttrs() { return true }, name() {} }`
];

rmSync(out, { force: true, recursive: true });
mkdirSync(join(out, 'node_modules'), { recursive: true });
writeFileSync(join(out, 'package.json'), '{ "private": true }\n');
for (let index = 0; index < count; index += 1) {
	counter = 0;
	const lang = pick(['ts', 'ts', 'ts', 'tsx']);
	let source = '';
	if (chance(0.5)) {
		const exported = chance(0.8) ? `${comment()}${pick(EXPORTS)()}${separator()}` : '';
		source += `<script lang="${lang}">${chance(0.5) ? '\n' : ''}${body(3)}${exported}${chance(0.3) ? body(2) : ''}</script>\n`;
	}
	if (chance(0.85)) source += `<script setup lang="${lang}"${chance(0.2) ? ' generic="T"' : ''}>${chance(0.7) ? '\n' : ''}${body(7)}</script>\n`;
	source += '<template><div /></template>\n';
	writeFileSync(join(out, `${String(index).padStart(5, '0')}.vue`), source);
}
console.log(`${count} files in ${out}`);
