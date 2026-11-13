#!/usr/bin/env bun
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';

/*
 * Plants known type errors across a project copy, so a differential run proves
 * both checkers find the same problems (a clean project only proves they both
 * say nothing). Deterministic: the same files and edits every time.
 *
 *   bun bench/mutate.ts <project-copy> [--count=200] [--dir=src]
 *
 * Each kind exercises a different part of the Vue mapping: script setup,
 * template bindings, interpolations, directives, events, props on components
 * (where Vue's fixed suppression rules apply), @vue-expect-error used and
 * unused (Vue's stateful rule), plain .ts files, and a .ts file that types a
 * component's props through its import (the rootDirs join). Template edits
 * alternate between the start and the end of the template.
 */
const root = process.argv[2];
if (!root) throw new Error('usage: bun bench/mutate.ts <project-copy>');
const option = (name: string, fallback: string) =>
	process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=')[1] ?? fallback;
const count = Number(option('count', '200'));
const directory = join(root, option('dir', 'src'));

const files = (readdirSync(directory, { recursive: true }) as string[])
	.filter((file) => /\.(vue|ts)$/.test(file) && !file.endsWith('.d.ts') && !file.includes('node_modules'))
	.sort();
const vueFiles = files.filter((file) => file.endsWith('.vue'));
const tsFiles = files.filter((file) => file.endsWith('.ts'));

const SCRIPT_SETUP_TS = /<script\b(?=[^>]*\bsetup\b)(?=[^>]*\blang="ts")[^>]*>\n?/;
const TEMPLATE_START = /^<template>\n?/m;
const TEMPLATE_END = /\n<\/template>\s*(?![\s\S]*\n<\/template>)/;

type Kind = { name: string; target: 'ts' | 'vue'; plant: (text: string, id: number, file: string) => string | null };

/** Adds `markup` at the start or the end of the root template. */
const inTemplate = (text: string, markup: string, id: number) => {
	if (!TEMPLATE_START.test(text)) return null;
	if (id % 2 === 1 && TEMPLATE_END.test(text)) return text.replace(TEMPLATE_END, (end) => `\n  ${markup}${end}`);

	return text.replace(TEMPLATE_START, (start) => `${start}  ${markup}\n`);
};
const inScriptSetup = (text: string, code: string) =>
	SCRIPT_SETUP_TS.test(text) ? text.replace(SCRIPT_SETUP_TS, (start) => `${start}${code}\n`) : null;

const KINDS: Kind[] = [
	{ name: 'script-setup-type', target: 'vue', plant: (text, id) => inScriptSetup(text, `const vueCheckMutation${id}: number = 'not a number';`) },
	{ name: 'template-unknown-binding', target: 'vue', plant: (text, id) => inTemplate(text, `<span :title="vueCheckUnknown${id}" />`, id) },
	{ name: 'template-interpolation-method', target: 'vue', plant: (text, id) => inTemplate(text, `<span>{{ 'text'.toUpperCas${id}() }}</span>`, id) },
	{ name: 'v-if-unknown', target: 'vue', plant: (text, id) => inTemplate(text, `<i v-if="vueCheckFlag${id}" />`, id) },
	{
		name: 'v-for-item-type',
		target: 'vue',
		plant: (text, id) => inTemplate(text, `<i v-for="item in [${id}]" :key="item">{{ item.length }}</i>`, id)
	},
	{ name: 'event-unknown-handler', target: 'vue', plant: (text, id) => inTemplate(text, `<button @click="vueCheckHandler${id}" />`, id) },
	{ name: 'component-prop-type', target: 'vue', plant: (text, id) => inTemplate(text, `<Transition :appear="${id}"><i /></Transition>`, id) },
	{ name: 'expect-error-unused', target: 'vue', plant: (text, id) => inTemplate(text, `<!-- @vue-expect-error -->\n  <i data-id="${id}" />`, id) },
	{
		name: 'expect-error-used',
		target: 'vue',
		plant: (text, id) => inTemplate(text, `<!-- @vue-expect-error -->\n  <i :title="vueCheckExpected${id}" />`, id)
	},
	{ name: 'ternary-member', target: 'vue', plant: (text, id) => inTemplate(text, `<span>{{ (${id} > 1 ? 'a' : 'b').toFixed() }}</span>`, id) },
	{ name: 'ts-file-type', target: 'ts', plant: (text, id) => `${text}\nexport const vueCheckMutation${id}: string = ${id};\n` },
	{
		name: 'ts-component-props',
		target: 'ts',
		plant: (text, id, file) => {
			const component = vueFiles[id % vueFiles.length];
			if (!component) return null;
			let path = relative(dirname(join(directory, file)), join(directory, component));
			if (!path.startsWith('.')) path = `./${path}`;

			return `import type VueCheckComponent${id} from '${path}';\n${text}\nexport const vueCheckProps${id}: InstanceType<typeof VueCheckComponent${id}>['$props'] = { vueCheckUnknownProp${id}: 1 };\n`;
		}
	}
];

const planted: { file: string; kind: string }[] = [];
const cursor = { ts: 0, vue: 0 };
const misses = { ts: 0, vue: 0 };
const exhausted = (target: 'ts' | 'vue') => misses[target] > (target === 'vue' ? vueFiles : tsFiles).length * 3;
for (let id = 0; planted.length < count && !(exhausted('ts') && exhausted('vue')); id++) {
	const kind = KINDS[id % KINDS.length]!;
	const pool = kind.target === 'vue' ? vueFiles : tsFiles;
	if (pool.length === 0 || exhausted(kind.target)) {
		misses[kind.target] = Number.POSITIVE_INFINITY;
		continue;
	}
	/* Spread over the project: a fixed stride through the files, wrapping. */
	const stride = Math.max(1, Math.floor(pool.length / Math.ceil(count / 2)));
	const step = cursor[kind.target]++;
	const file = pool[(step * stride + Math.floor((step * stride) / pool.length)) % pool.length]!;
	const path = join(directory, file);
	const next = kind.plant(readFileSync(path, 'utf8'), id, file);
	if (next === null) {
		misses[kind.target]++;
		continue;
	}
	writeFileSync(path, next);
	planted.push({ file: relative(root, path), kind: kind.name });
}
const byKind = Object.fromEntries(KINDS.map((kind) => [kind.name, planted.filter((item) => item.kind === kind.name).length]));
console.log(JSON.stringify({ byKind, planted: planted.length }, null, 2));
