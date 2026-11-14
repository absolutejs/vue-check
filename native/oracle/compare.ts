#!/usr/bin/env bun
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Compares the native parser's output with the oracle's, value for value.
 *
 *   bun native/oracle/compare.ts <out-dir>
 *
 * Reads <out-dir>/oracle.jsonl (dump.ts) and <out-dir>/native.jsonl
 * (`vue-check-codegen parse`). Prints the first difference of each file that
 * differs, and exits 1 if any does. A file the native side marks unsupported
 * is listed, not compared: the JavaScript codegen handles it.
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/compare.ts <out-dir>');
const lines = (name: string) =>
	readFileSync(join(out, name), 'utf8')
		.split('\n')
		.filter(Boolean)
		.map((line) => JSON.parse(line) as Record<string, any>);

/* Before 3.3.12 the script analysis keeps its bindings as ranges (the port
 * keeps what 3.3.12 does: each name and what kind it is), a style's `module`
 * as written, and none of what 3.3.12 added. Those are left to the layers
 * above, which use them. */
const core = (() => {
	try {
		return readFileSync(join(out, 'core.txt'), 'utf8').trim();
	} catch {
		return '';
	}
})();
const lenient = ['3.3.9', '3.3.10', '3.3.11'].includes(core);
const LEFT_TO_LATER = /\.(?:bindings|components|module|defaultPropsArg)(?:\.|$)/;

/** The path and values of the first difference, or null. */
const difference = (expected: unknown, actual: unknown, path: string): string | null => {
	if (expected === actual) return null;
	if (lenient && LEFT_TO_LATER.test(path)) return null;
	if (typeof expected !== 'object' || typeof actual !== 'object' || expected === null || actual === null)
		return `${path}: expected ${JSON.stringify(expected)}, native ${JSON.stringify(actual)}`;
	if (Array.isArray(expected) !== Array.isArray(actual)) return `${path}: expected ${JSON.stringify(expected)?.slice(0, 200)}, native ${JSON.stringify(actual)?.slice(0, 200)}`;
	const keys = [...new Set([...Object.keys(expected), ...Object.keys(actual)])];
	for (const key of keys) {
		const found = difference((expected as Record<string, unknown>)[key], (actual as Record<string, unknown>)[key], `${path}.${key}`);
		if (found) return found;
	}

	return null;
};

const native = new Map(lines('native.jsonl').map((record) => [record.file as string, record]));
let same = 0;
let poisons = 0;
const different: string[] = [];
const unsupported: string[] = [];
const threw: string[] = [];
for (const expected of lines('oracle.jsonl')) {
	const actual = native.get(expected.file);
	if (!actual) {
		different.push(`${expected.file}: no native result`);
		continue;
	}
	/* A file that changes how later files parse: both sides must say so, and
	 * its own result isn't compared (the project can't use the native codegen). */
	if (Boolean(actual.poisonsLaterParses) !== Boolean(expected.poisonsLaterParses)) {
		different.push(`${expected.file}: poisons later parses: expected ${expected.poisonsLaterParses}, native ${actual.poisonsLaterParses}`);
		continue;
	}
	if (expected.poisonsLaterParses) {
		poisons += 1;
		continue;
	}
	if (actual.unsupported) {
		unsupported.push(`${expected.file}: ${actual.unsupported}`);
		continue;
	}
	/* The original threw and the native side returned a result. */
	if (expected.threw) {
		threw.push(`${expected.file}: ${expected.threw}`);
		continue;
	}
	/* The transforms' errors aren't ported: the codegen never reads them. */
	if (expected.template) delete expected.template.errors;
	if (actual.template) delete actual.template.errors;
	/* Blocks and the template are only in the oracle for the ported version. */
	const found = expected.sfc.template === undefined
		? difference(expected.sfc.ast, actual.sfc.ast, 'sfc.ast')
		: difference(expected.sfc, actual.sfc, 'sfc') ??
			difference(expected.template, actual.template, 'template') ??
			difference(expected.script, actual.script, 'script') ??
			difference(expected.scriptSetup, actual.scriptSetup, 'scriptSetup');
	if (found) different.push(`${expected.file}: ${found.slice(0, 600)}`);
	else same += 1;
}
for (const line of different) console.log(`DIFFERENT ${line}`);
for (const line of unsupported) console.log(`unsupported ${line}`);
for (const line of threw) console.log(`oracle threw ${line}`);
console.log(JSON.stringify({ different: different.length, oracleThrew: threw.length, poisonsLaterParses: poisons, same, unsupported: unsupported.length }));
process.exit(different.length + threw.length > 0 ? 1 : 0);
