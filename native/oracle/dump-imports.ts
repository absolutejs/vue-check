#!/usr/bin/env bun
import { readdirSync, readFileSync, writeFileSync, lstatSync } from 'node:fs';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { importReaderSignature } from '../../src/native';

/*
 * The oracle for the import reader (native/src/imports): what the project's
 * own TypeScript lists as each file's imports, `preProcessFile(text, true,
 * true).importedFiles`. Every script file under the folder, node_modules
 * included: packages hold syntax a project's own code doesn't.
 *
 *   bun native/oracle/dump-imports.ts <dir> <out-dir> [--max-mb=400]
 *
 * Writes files.jsonl ({file, text}) and oracle.jsonl ({file, imports}), and
 * prints the signature of this TypeScript: the hash vue-check compares before
 * it uses the port (src/native.ts lists the ones this oracle has passed on).
 */
const [directory = '.', out = '.'] = process.argv.slice(2);
const maxBytes = Number(process.argv.find((value) => value.startsWith('--max-mb='))?.split('=')[1] ?? 400) * 1024 * 1024;
/* TYPESCRIPT_FROM: a folder whose node_modules has the TypeScript to ask, when the folder read has none. */
const typescriptFrom = createRequire(join(process.env.TYPESCRIPT_FROM ?? process.cwd(), 'package.json'));
const ts = typescriptFrom('typescript') as typeof import('typescript');
const signature = importReaderSignature(typescriptFrom.resolve('typescript'));
const SCRIPT = /\.(?:[cm]?[jt]sx?)$/;
const files: string[] = [];
const oracle: string[] = [];
let bytes = 0;
const walk = (folder: string) => {
	for (const name of readdirSync(folder).sort()) {
		if (bytes > maxBytes) return;
		const path = join(folder, name);
		const stat = lstatSync(path);
		if (stat.isSymbolicLink()) continue;
		if (stat.isDirectory()) {
			if (name !== '.git') walk(path);
			continue;
		}
		if (!SCRIPT.test(name) || stat.size > 4 * 1024 * 1024) continue;
		const text = readFileSync(path, 'utf8');
		// A file that isn't UTF-8 is read differently by the two sides; the port declines it.
		if (Buffer.byteLength(text) !== stat.size) continue;
		bytes += stat.size;
		files.push(JSON.stringify({ file: path, text }));
		oracle.push(JSON.stringify({ file: path, imports: ts.preProcessFile(text, true, true).importedFiles.map((imported) => imported.fileName) }));
	}
};
walk(directory);
writeFileSync(join(out, 'files.jsonl'), files.map((line) => `${line}\n`).join(''));
writeFileSync(join(out, 'oracle.jsonl'), oracle.map((line) => `${line}\n`).join(''));
console.log(JSON.stringify({ files: files.length, megabytes: Math.round(bytes / 1024 / 1024), signature, typescript: ts.version }));
