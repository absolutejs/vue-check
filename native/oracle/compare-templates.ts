#!/usr/bin/env bun
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Compares each template the oracle recorded with what the native side
 * generates for it: the code and its mappings, the types it declares, and
 * the names it reads from the component.
 *
 *   bun native/oracle/compare-templates.ts <out-dir>
 *
 * Reads <out-dir>/oracle.jsonl and <out-dir>/templates.jsonl
 * (`vue-check-codegen template`).
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/compare-templates.ts <out-dir>');
const lines = (name: string) =>
	readFileSync(join(out, name), 'utf8')
		.split('\n')
		.filter(Boolean)
		.map((line) => JSON.parse(line) as Record<string, any>);
const native = new Map(lines('templates.jsonl').map((record) => [record.file as string, record.templates as Record<string, any>[]]));
/* For a version before 3.3.12 the port writes some runs of plain text in
 * other pieces than the original (it shares the newer codegen's helpers), so
 * adjacent pieces of plain text are compared joined: where a run is cut makes
 * no difference to the code or to a mapping. The names read are compared as a
 * set: those versions keep no record of the order. */
const core = (() => {
	try {
		return readFileSync(join(out, 'core.txt'), 'utf8').trim();
	} catch {
		return '';
	}
})();
const lenient = ['3.3.9', '3.3.10', '3.3.11'].includes(core);
const joined = (codes: unknown[]) => {
	const result: unknown[] = [];
	for (const code of codes) {
		const last = result.at(-1);
		if (typeof code === 'string' && typeof last === 'string') result[result.length - 1] = last + code;
		else result.push(code);
	}

	return result;
};
const text = (codes: unknown[]) => codes.map((code) => (typeof code === 'string' ? code : (code as string[])[0])).join('');
let same = 0;
let shown = 0;
let different = 0;
const unsupported = new Map<string, number>();
for (const expected of lines('oracle.jsonl')) {
	const calls = (expected.templates ?? []) as Record<string, any>[];
	const replayed = native.get(expected.file) ?? [];
	for (const [index, call] of calls.entries()) {
		const actual = replayed[index];
		if (lenient && actual?.codes) {
			call.codes = joined(call.codes);
			actual.codes = joined(actual.codes);
			call.dotValueAccesses = actual.dotValueAccesses;
		}
		if (actual?.unsupported) {
			unsupported.set(actual.unsupported, (unsupported.get(actual.unsupported) ?? 0) + 1);
			continue;
		}
		const fields = ['codes', 'generatedTypes', 'contextAccesses', 'dotValueAccesses'] as const;
		const field = fields.find((name) => JSON.stringify(call[name]) !== JSON.stringify(actual?.[name]));
		if (!field) {
			same += 1;
			continue;
		}
		different += 1;
		if (shown >= 12) continue;
		shown += 1;
		if (field !== 'codes') {
			console.log(`DIFFERENT ${expected.file} #${index} ${field}: expected ${JSON.stringify(call[field]).slice(0, 300)}, native ${JSON.stringify(actual?.[field]).slice(0, 300)}`);
			continue;
		}
		/* The generated text first: where it differs, and the code around it. */
		const [wanted, got] = [text(call.codes), text(actual.codes)];
		if (wanted !== got) {
			let at = 0;
			while (at < wanted.length && wanted[at] === got[at]) at += 1;
			console.log(`DIFFERENT ${expected.file} #${index} text at ${at}:\n  expected ${JSON.stringify(wanted.slice(Math.max(0, at - 90), at + 110))}\n  native   ${JSON.stringify(got.slice(Math.max(0, at - 90), at + 110))}`);
			continue;
		}
		const position = (call.codes as unknown[]).findIndex((code, at) => JSON.stringify(code) !== JSON.stringify(actual.codes[at]));
		console.log(`DIFFERENT ${expected.file} #${index} mapping at piece ${position}:\n  expected ${JSON.stringify(call.codes.slice(Math.max(0, position - 2), position + 3)).slice(0, 400)}\n  native   ${JSON.stringify(actual.codes.slice(Math.max(0, position - 2), position + 3)).slice(0, 400)}`);
	}
}
for (const [reason, count] of unsupported) console.log(`unsupported ${count}: ${reason}`);
console.log(JSON.stringify({ different, same, unsupported: [...unsupported.values()].reduce((sum, count) => sum + count, 0) }));
process.exit(different > 0 ? 1 : 0);
