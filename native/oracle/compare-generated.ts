#!/usr/bin/env bun
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Compares the code the original generates for each file, and its mappings,
 * with what the native side generates.
 *
 *   bun native/oracle/compare-generated.ts <out-dir>
 *
 * Reads <out-dir>/oracle.jsonl and <out-dir>/generated.jsonl
 * (`vue-check-codegen generate`).
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/compare-generated.ts <out-dir>');
const lines = (name: string) =>
	readFileSync(join(out, name), 'utf8')
		.split('\n')
		.filter(Boolean)
		.map((line) => JSON.parse(line) as Record<string, any>);
const native = new Map(lines('generated.jsonl').map((record) => [record.file as string, record]));
let same = 0;
let shown = 0;
let different = 0;
let skipped = 0;
const unsupported = new Map<string, number>();
const show = (message: string) => {
	if (shown >= 12) return;
	shown += 1;
	console.log(message);
};
for (const expected of lines('oracle.jsonl')) {
	const actual = native.get(expected.file);
	if (actual?.unsupported) {
		unsupported.set(actual.unsupported, (unsupported.get(actual.unsupported) ?? 0) + 1);
		continue;
	}
	/* A file the original couldn't be asked about (it threw, or the parser's
	 * state had leaked): the native side must have declined it. */
	if (!expected.generated) {
		/* Another version of the codegen than the ported one: nothing to compare. */
		if (expected.generated === undefined && !expected.poisonsLaterParses && !expected.threw) skipped += 1;
		else if (actual?.generated) {
			different += 1;
			show(`DIFFERENT ${expected.file}: native generated a file the original has no record of`);
		} else skipped += 1;
		continue;
	}
	const [wanted, got] = [expected.generated, actual?.generated];
	if (JSON.stringify(wanted) === JSON.stringify(got)) {
		same += 1;
		continue;
	}
	different += 1;
	if (!got) {
		show(`DIFFERENT ${expected.file}: no native result`);
		continue;
	}
	if (wanted.id !== got.id || wanted.languageId !== got.languageId) {
		show(`DIFFERENT ${expected.file} id: expected ${wanted.id} ${wanted.languageId}, native ${got.id} ${got.languageId}`);
		continue;
	}
	if (wanted.text !== got.text) {
		let at = 0;
		while (at < wanted.text.length && wanted.text[at] === got.text[at]) at += 1;
		show(`DIFFERENT ${expected.file} text at ${at}:\n  expected ${JSON.stringify(wanted.text.slice(Math.max(0, at - 100), at + 120))}\n  native   ${JSON.stringify(got.text.slice(Math.max(0, at - 100), at + 120))}`);
		continue;
	}
	const position = (wanted.mappings as unknown[]).findIndex((mapping, at) => JSON.stringify(mapping) !== JSON.stringify(got.mappings[at]));
	const where = position < 0 ? wanted.mappings.length : position;
	const around = (mapping: Record<string, any> | undefined) => (mapping ? `${JSON.stringify(mapping)} ${JSON.stringify(wanted.text.slice(mapping.generated[0], mapping.generated[0] + Math.max(mapping.lengths[0], 30)))}` : 'none');
	show(`DIFFERENT ${expected.file} mapping ${where} of ${wanted.mappings.length}/${got.mappings.length}:\n  expected ${around(wanted.mappings[where])}\n  native   ${around(got.mappings[where])}`);
}
for (const [reason, count] of [...unsupported].sort((a, b) => b[1] - a[1])) console.log(`unsupported ${count}: ${reason}`);
console.log(JSON.stringify({ different, same, skipped, unsupported: [...unsupported.values()].reduce((sum, count) => sum + count, 0) }));
process.exit(different ? 1 : 0);
