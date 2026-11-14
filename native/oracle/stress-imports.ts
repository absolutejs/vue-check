#!/usr/bin/env bun
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Files for the import reader's oracle that real projects don't contain:
 * real files (a files.jsonl from dump-imports.ts) cut off at any point and
 * with the scanner's awkward tokens put in at random places, from a seed.
 *
 *   bun native/oracle/stress-imports.ts <files.jsonl> <out-dir> [--count=3000] [--seed=1]
 */
const [source = '', out = '.'] = process.argv.slice(2);
const option = (name: string, fallback: number) => Number(process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=')[1] ?? fallback);
const count = option('count', 3000);
let state = option('seed', 1) >>> 0 || 1;
const random = () => {
	state ^= state << 13;
	state >>>= 0;
	state ^= state >>> 17;
	state ^= state << 5;
	state >>>= 0;

	return state / 0x100000000;
};
const pick = <Item>(items: Item[]) => items[Math.floor(random() * items.length)] as Item;
const texts = readFileSync(source, 'utf8')
	.split('\n')
	.filter(Boolean)
	.map((line) => (JSON.parse(line) as { text: string }).text)
	.filter((text) => text.length > 200 && text.length < 60000);
const PIECES = [
	"import a from './a'", 'import("./b")', "import type { T } from './t'", "import type T from './t'", "import type from './t'", 'import * as ns from "./ns"',
	"import a, { b } from './ab'", "import a, * as b from './ab'", "import a = require('./r')", "export import a = require('./r')", "export * from './e'",
	"export * as e from './e'", "export { a } from './e'", "export type { A } from './e'", "export type * from './e'", "require('./r')", 'require(`./r`)',
	"define(['./d', `./e`], f)", "define('name', ['./d'], f)", "declare module './m' {}", "declare module './m'", 'x.import("./no")', 'import.meta.url',
	'import(`./t`)', 'import(`./${t}`)', 'import x from {', 'import x from *', 'import x, from', 'import type type from "./t"', 'import type as from "./t"',
	'`', '${', '}', '{', "'", '"', '\\', '\\u0061', '\\u{61}', '\\u{', '\\u{110000}', '\\uD83D', '\\uD83D\\uDE00', '\\x4', '\\x41', '\\0', '\\08', '\\377', '\\8', '\\\n', '\\\r\n',
	'/*', '*/', '//', '/', '/=', '\n', '\r\n', '\r', ' ', ' ', ' ', '﻿', '\u0085', '​', '�', '#!', '#!/bin/sh\n', '#a', '#\\u0061', '#', '@',
	'0x', '0x1_f', '0b', '0b102', '0o', '0o78', '017', '019', '019.5', '0_1', '1_000', '1__0', '1e', '1e+', '1e5n', '1n', '1na', '1.5n', '.5', '1.', '1..toString()', '0n', '00', '0.n',
	'?.', '?.5', '??=', '**=', '>>>=', '<<=', '=>', '===', '...', '..', '<<<<<<< HEAD\n', '=======\n', '>>>>>>> branch\n', '|||||||\n', '<<<<<<<\n', '\n<<<<<<< x', '\n=======',
	'é', '日本', '😀', '𝒜', 'á', '\\u00e9x', 'impor\\u0074 "./esc"', 'i\\u{6d}port "./esc"', '\\u0069mport "./esc"', 'from', 'type', 'as', 'define', 'module', 'declare',
	"'./s\\u0061'", "'./s\\x61'", "'./s\\141'", "'./s\\\n'", "`./s\\141`", "'unterminated", '"unterminated', '`unterminated', "import '\\uD800'", "'./😀'", "'./\\😀'", 'import /* c */ "./c"',
	'import // c\n"./c"', 'export {', 'export type', 'export import type a = require("./r")', 'import type a = require("./r")', 'import {a,', 'import * as', 'import a =', "import defer * as d from './d'", "import defer from './d'", "import defer, { a } from './d'", "import type defer from './d'", 'defer', 'accessor', 'satisfies', 'using'
];
mkdirSync(out, { recursive: true });
for (let index = 0; index < count; index++) {
	let text = pick(texts);
	const start = Math.floor(random() * text.length);
	text = text.slice(start, start + 200 + Math.floor(random() * 4000));
	const edits = Math.floor(random() * 12);
	for (let edit = 0; edit < edits; edit++) {
		const at = Math.floor(random() * (text.length + 1));
		const kind = random();
		if (kind < 0.75) text = text.slice(0, at) + (random() < 0.5 ? ' ' : '') + pick(PIECES) + (random() < 0.5 ? ' ' : '') + text.slice(at);
		else if (kind < 0.9) text = text.slice(0, at) + text.slice(at + Math.floor(random() * 30));
		else text = text.slice(0, at);
	}
	if (random() < 0.15) text = pick(['#!/usr/bin/env node\n', '﻿', '#!', '<<<<<<< HEAD\n', '=======\n']) + text;
	/* A file can't hold half a surrogate pair: readFileSync would turn it into U+FFFD. */
	text = text.replace(/[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g, '');
	writeFileSync(join(out, `${String(index).padStart(5, '0')}.ts`), text);
}
console.log(`${count} files in ${out}`);
