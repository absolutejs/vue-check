#!/usr/bin/env bun
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Plants known type errors across a project copy, so a differential run proves
 * both checkers find the same problems (a clean project only proves they both
 * say nothing). Deterministic: the same files and edits every time.
 *
 *   bun bench/mutate.ts <project-copy> [--count=40]
 */
const root = process.argv[2];
if (!root) throw new Error('usage: bun bench/mutate.ts <project-copy>');
const count = Number(process.argv.find((v) => v.startsWith('--count='))?.split('=')[1] ?? '40');
const files = (readdirSync(join(root, 'src'), { recursive: true }) as string[])
	.filter((file) => /\.(vue|ts)$/.test(file) && !file.endsWith('.d.ts'))
	.sort();
const step = Math.max(1, Math.floor(files.length / count));
const planted: string[] = [];
for (let index = 0; planted.length < count && index < files.length; index += step) {
	const file = files[index]!;
	const path = join(root, 'src', file);
	let text = readFileSync(path, 'utf8');
	const kind = planted.length % 4;
	if (file.endsWith('.ts')) {
		text += `\nexport const vueCheckMutation${planted.length}: string = ${planted.length};\n`;
	} else if (kind === 0 && /<script setup lang="ts">/.test(text)) {
		text = text.replace(/<script setup lang="ts">\n?/, (m) => `${m}const vueCheckMutation${planted.length}: number = 'not a number';\n`);
	} else if (kind === 1 && /<template>/.test(text)) {
		text = text.replace(/<template>\n?/, (m) => `${m}  <span :title="vueCheckUnknownName${planted.length}" />\n`);
	} else if (kind === 2 && /<template>/.test(text)) {
		text = text.replace(/<template>\n?/, (m) => `${m}  <span>{{ 'text'.toUpperCas${planted.length}() }}</span>\n`);
	} else if (/<script setup lang="ts">/.test(text)) {
		text = text.replace(/<script setup lang="ts">\n?/, (m) => `${m}const vueCheckMutation${planted.length}: boolean = 1;\n`);
	} else continue;
	writeFileSync(path, text);
	planted.push(`${file} (kind ${file.endsWith('.ts') ? 'ts' : kind})`);
}
console.log(JSON.stringify({ planted, total: planted.length }, null, 2));
