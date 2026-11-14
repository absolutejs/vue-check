#!/usr/bin/env bun
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Compares the native import reader's output with the oracle's
 * (dump-imports.ts), file by file: the same names in the same order.
 *
 *   bun native/oracle/compare-imports.ts <out-dir>
 */
const [out = '.'] = process.argv.slice(2);
const lines = (name: string) =>
	readFileSync(join(out, name), 'utf8')
		.split('\n')
		.filter(Boolean)
		.map((line) => JSON.parse(line) as { file: string; imports?: string[]; unsupported?: string });
const expected = lines('oracle.jsonl');
const actual = lines('native.jsonl');
const counts = { different: 0, same: 0, unsupported: 0 };
for (const [index, oracle] of expected.entries()) {
	const native = actual[index];
	if (native?.file !== oracle.file) throw new Error(`line ${index + 1}: ${native?.file} is not ${oracle.file}`);
	if (native.unsupported !== undefined) {
		counts.unsupported++;
		console.log(`unsupported ${oracle.file}: ${native.unsupported}`);
	} else if (JSON.stringify(native.imports) === JSON.stringify(oracle.imports)) counts.same++;
	else {
		counts.different++;
		if (counts.different <= 20) {
			const at = (oracle.imports ?? []).findIndex((name, position) => name !== native.imports?.[position]);
			console.log(`different ${oracle.file}: at ${at}, expected ${JSON.stringify(oracle.imports?.slice(Math.max(0, at - 1), at + 2))}, got ${JSON.stringify(native.imports?.slice(Math.max(0, at - 1), at + 2))} (${oracle.imports?.length} and ${native.imports?.length})`);
		}
	}
}
console.log(JSON.stringify(counts));
process.exit(counts.different > 0 ? 1 : 0);
