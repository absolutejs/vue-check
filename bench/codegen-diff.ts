#!/usr/bin/env bun
import { existsSync, lstatSync, readdirSync, readFileSync, readlinkSync } from 'node:fs';
import { join, relative } from 'node:path';

/*
 * The bar for the native codegen: run on the same project, it must write the
 * same output folder as the JavaScript codegen (src/codegen.ts). Every
 * generated file byte for byte, and codegen.json (every source map, every
 * segment's verification rule, the unparsed list) equal. Identical input to
 * the checker means identical errors by construction.
 *
 *   bun bench/codegen-diff.ts <expected-out-dir> <actual-out-dir>
 *
 * Exits 0 when identical, 1 with the first differences otherwise.
 */

const [expected, actual] = process.argv.slice(2);
if (!expected || !actual) throw new Error('usage: bun bench/codegen-diff.ts <expected-out-dir> <actual-out-dir>');

/* Every file under a folder, and every link as what it points to: the links
 * to node_modules and package.json (see src/codegen.ts) aren't followed. */
const filesOf = (root: string) => {
	const found: string[] = [];
	const walk = (folder: string) => {
		for (const entry of readdirSync(folder)) {
			const path = join(folder, entry);
			const status = lstatSync(path);
			if (status.isDirectory()) walk(path);
			else if (!path.endsWith('codegen.json') && !path.endsWith('tsconfig.json')) found.push(relative(root, path));
		}
	};
	walk(root);

	return found.sort();
};
const contentOf = (path: string) => (lstatSync(path).isSymbolicLink() ? `link to ${readlinkSync(path)}` : readFileSync(path, 'utf8'));

const problems: string[] = [];
const expectedFiles = filesOf(expected);
const actualFiles = new Set(filesOf(actual));
for (const file of expectedFiles) {
	if (!actualFiles.has(file)) {
		problems.push(`missing: ${file}`);
		continue;
	}
	actualFiles.delete(file);
	const want = contentOf(join(expected, file));
	const got = contentOf(join(actual, file));
	if (want === got) continue;
	let at = 0;
	while (at < want.length && want[at] === got[at]) at++;
	const line = want.slice(0, at).split('\n').length;
	problems.push(`differs: ${file} at line ${line}\n  expected: ${JSON.stringify(want.slice(at, at + 80))}\n  actual:   ${JSON.stringify(got.slice(at, at + 80))}`);
}
for (const file of actualFiles) problems.push(`unexpected: ${file}`);

/* The source maps and the rest of codegen.json, compared as data. */
const json = (root: string) => JSON.parse(readFileSync(join(root, 'codegen.json'), 'utf8')) as Record<string, unknown>;
const firstDifference = (want: unknown, got: unknown, path: string): string | null => {
	if (JSON.stringify(want) === JSON.stringify(got)) return null;
	if (typeof want !== 'object' || typeof got !== 'object' || want === null || got === null)
		return `${path}: expected ${JSON.stringify(want)?.slice(0, 120)}, got ${JSON.stringify(got)?.slice(0, 120)}`;
	const keys = [...new Set([...Object.keys(want), ...Object.keys(got)])];
	for (const key of keys) {
		const difference = firstDifference((want as Record<string, unknown>)[key], (got as Record<string, unknown>)[key], `${path}.${key}`);
		if (difference) return difference;
	}

	return `${path}: differs`;
};
if (!existsSync(join(actual, 'codegen.json'))) problems.push('missing: codegen.json');
else {
	const difference = firstDifference(json(expected), json(actual), 'codegen.json');
	if (difference) problems.push(difference);
}

if (problems.length === 0) {
	console.log(`identical: ${expectedFiles.length} generated files and codegen.json`);
	process.exit(0);
}
console.log(`${problems.length} difference(s):`);
for (const problem of problems.slice(0, 20)) console.log(problem);
process.exit(1);
