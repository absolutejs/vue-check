#!/usr/bin/env bun
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Compares each template expression the oracle recorded with what the native
 * side generates for the same call: the code, its mappings, and the names it
 * reads from the component.
 *
 *   bun native/oracle/compare-interpolations.ts <out-dir>
 *
 * Reads <out-dir>/oracle.jsonl and <out-dir>/interpolations.jsonl
 * (`vue-check-codegen interpolate`).
 */
const out = process.argv[2];
if (!out) throw new Error('usage: bun native/oracle/compare-interpolations.ts <out-dir>');
const lines = (name: string) =>
	readFileSync(join(out, name), 'utf8')
		.split('\n')
		.filter(Boolean)
		.map((line) => JSON.parse(line) as Record<string, any>);
const native = new Map(lines('interpolations.jsonl').map((record) => [record.file as string, record.interpolations as Record<string, any>[]]));
let same = 0;
let threw = 0;
let invalid = 0;
const invalidExamples: string[] = [];
const different: string[] = [];
const unsupported = new Map<string, number>();
for (const expected of lines('oracle.jsonl')) {
	const calls = (expected.interpolations ?? []) as Record<string, any>[];
	const replayed = native.get(expected.file) ?? [];
	for (const [index, call] of calls.entries()) {
		const actual = replayed[index];
		/* The original threw: the native side must not return a result. */
		if (call.threw) {
			if (actual?.unsupported) threw += 1;
			else different.push(`${expected.file}: the original threw on ${JSON.stringify(call.prefix + call.code + call.suffix).slice(0, 300)}\n  ${call.threw}`);
			continue;
		}
		if (actual?.unsupported) {
			unsupported.set(actual.unsupported, (unsupported.get(actual.unsupported) ?? 0) + 1);
			continue;
		}
		const wanted = JSON.stringify({ accesses: call.accesses, codes: call.codes });
		const got = JSON.stringify({ accesses: actual?.accesses, codes: actual?.codes });
		if (wanted === got) same += 1;
		else if (call.typescriptErrors > 0) {
			/* Code TypeScript rejects and the native parser accepts: the two then
			 * read it differently. Counted apart: it isn't a difference on valid code. */
			invalid += 1;
			if (invalidExamples.length < 5) invalidExamples.push(JSON.stringify(call.prefix + call.code + call.suffix).slice(0, 160));
		} else if (different.length < 40)
			different.push(`${expected.file}: ${JSON.stringify(call.prefix + call.code + call.suffix).slice(0, 200)}\n  expected ${wanted.slice(0, 700)}\n  native   ${got.slice(0, 700)}`);
		else different.push('');
	}
}
for (const line of different.filter(Boolean)) console.log(`DIFFERENT ${line}`);
for (const example of invalidExamples) console.log(`invalid for TypeScript, accepted natively: ${example}`);
for (const [reason, count] of unsupported) console.log(`unsupported ${count}: ${reason}`);
console.log(JSON.stringify({ different: different.length, invalidForTypeScriptOnly: invalid, originalThrew: threw, same, unsupported: [...unsupported.values()].reduce((sum, count) => sum + count, 0) }));
process.exit(different.length > 0 ? 1 : 0);
