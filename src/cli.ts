#!/usr/bin/env bun
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import type { SourceMap, UnparsedFile } from './generate';
import { deserialize, type Serialized } from './serialize';

/*
 * vue-check: vue-tsc's errors, from Bun's native TypeScript checker.
 *
 *   1. codegen.ts (its own process) writes the TypeScript Vue's codegen makes
 *      for each .vue file into .vue-check/ (the same code vue-tsc checks), and
 *      exits, returning its memory.
 *   2. `bun check` checks the project and those files together: rootDirs
 *      joins .vue-check/ to the project, so `import X from './X.vue'` finds
 *      `X.vue.ts` and .ts files see components' real types.
 *   3. Errors in generated files are mapped back to the .vue file's line and
 *      column, and printed the way vue-tsc prints them. Only the files with
 *      errors are regenerated for this (codegen is deterministic).
 *
 * Each step runs after the previous one has exited, so the peak memory is
 * the largest step, not their sum.
 *
 *   vue-check [-p tsconfig.json] [--threads=N]
 */

const argument = (name: string) => {
	const index = process.argv.findIndex((value) => value === name || value.startsWith(`${name}=`));
	if (index < 0) return undefined;
	const value = process.argv[index];

	return value?.includes('=') ? value.split('=')[1] : process.argv[index + 1];
};

const root = process.cwd();
const tsconfig = resolve(root, argument('-p') ?? argument('--project') ?? 'tsconfig.json');
if (!existsSync(tsconfig)) {
	console.error(`vue-check: ${relative(root, tsconfig)} not found`);
	process.exit(2);
}
const outDir = join(root, '.vue-check');
/* VUE_CHECK_PHASES=<file> writes how long each step took: codegen and the
 * check (each its own process) and mapping errors back (this one). */
const phasesFile = process.env.VUE_CHECK_PHASES;

const codegenStarted = performance.now();
const codegen = Bun.spawnSync([process.execPath, join(import.meta.dir, 'codegen.ts'), root, tsconfig], {
	cwd: root,
	stderr: 'inherit',
	stdout: 'inherit'
});
const codegenMs = performance.now() - codegenStarted;
if (codegen.exitCode !== 0) {
	console.error('vue-check: Vue codegen failed (above), so the project was NOT type-checked.');
	process.exit(3);
}
const codegenResult = JSON.parse(readFileSync(join(outDir, 'codegen.json'), 'utf8')) as Serialized & { unparsed: UnparsedFile[] };
const unparsedFiles = codegenResult.unparsed;
if (unparsedFiles.length > 0) {
	for (const item of unparsedFiles)
		console.error(`vue-check: the TypeScript generated for ${item.file} doesn't parse (generated line ${item.line}: ${item.message}).`);
	console.error('vue-check: the project was NOT type-checked. Please report the file(s) above.');
	process.exit(3);
}
const checkConfig = join(outDir, 'tsconfig.json');

const threads = argument('--threads');
/* Anything else is a compiler option for bun check (e.g. --skipLibCheck), as vue-tsc passes them to tsc. */
const OWN = new Set(['-p', '--project', '--threads']);
const passThrough = process.argv.slice(2).filter((value, index, all) => {
	const name = value.split('=')[0] ?? '';
	if (OWN.has(name)) return false;
	const previous = all[index - 1];

	return !(previous && OWN.has(previous) && !previous.includes('='));
});
/* VUE_CHECK_TSC=<tsc> checks with that tsc instead of bun check: the same
 * codegen and mapping, only the checker differs. The benchmark uses real
 * tsc 7 this way to prove bun check agrees with TypeScript 7. */
const oracleTsc = process.env.VUE_CHECK_TSC;
const checkStarted = performance.now();
const checked = Bun.spawnSync(
	oracleTsc
		? [oracleTsc, '-p', checkConfig, '--noEmit', '--pretty', 'false', ...passThrough]
		: [process.execPath, 'check', '-p', checkConfig, '--no-pretty', '--all', ...(threads ? [`--threads=${threads}`] : []), ...passThrough],
	{ cwd: root, stderr: 'pipe', stdout: 'pipe' }
);
const checkMs = performance.now() - checkStarted;
const mapStarted = performance.now();

const ERROR_LINE = /^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$/;
const lines = checked.stdout.toString().split('\n');
const fileOf = (rawFile: string) => relative(root, resolve(root, rawFile));
/* The maps, as data. A file whose rules aren't all data (see serialize.ts),
 * or with an error code the fixed rules weren't evaluated for, is
 * regenerated and decided by Volar itself, as vue-tsc would. */
const knownCodes = new Set(codegenResult.knownCodes);
const maps: Record<string, SourceMap> = {};
const regenerate = new Set<string>();
for (const line of lines) {
	const match = ERROR_LINE.exec(line);
	if (!match) continue;
	const file = fileOf(match[1] ?? '');
	const serial = codegenResult.maps[file];
	if (!serial) continue;
	if (serial.regenerate || !knownCodes.has(Number((match[5] ?? '').replace('TS', '')))) regenerate.add(file);
	else maps[file] ??= deserialize(serial, codegenResult.rules);
}
if (regenerate.size > 0) {
	const sources = [...regenerate].map((file) => join(root, codegenResult.maps[file]?.source ?? ''));
	const live = (await import('./generate')).generate({ only: sources, outDir, root, tsconfig }).maps;
	for (const file of regenerate) {
		const map = live[file];
		if (map) maps[file] = map;
	}
}

const offsetOf = (text: string, line: number, column: number) => {
	const lines = text.split('\n');
	let offset = 0;
	for (let index = 0; index < line - 1; index++) offset += (lines[index]?.length ?? 0) + 1;

	return offset + column - 1;
};
const positionOf = (text: string, offset: number) => {
	const before = text.slice(0, offset).split('\n');

	return { column: (before.at(-1)?.length ?? 0) + 1, line: before.length };
};

/**
 * A generated-file error, where vue-tsc would report it: the position in the
 * .vue file, or null when vue-tsc wouldn't report it there. As Volar does for
 * vue-tsc, a segment counts only if its `verification` feature allows this
 * error code (Volar's own shouldReportDiagnostics decides).
 */
const sourcePosition = (file: string, map: SourceMap, line: number, column: number, code: string) => {
	const generated = readFileSync(join(root, file), 'utf8');
	const position = offsetOf(generated, line, column);
	const at = position - map.shift;
	/* tsc-style output gives where an error starts, not its length; the
	 * token there (a name or a string) is its span, as TypeScript reports it. */
	const token = /^(?:[\p{L}\p{N}_$]+|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/u.exec(generated.slice(position));
	const end = at + Math.max(1, token?.[0].length ?? 1);
	const source = () => readFileSync(join(root, map.source), 'utf8');
	/* As Volar maps a diagnostic for vue-tsc: its start and its end each have
	 * to fall in a segment whose `verification` allows this code. A start
	 * inside a segment wins over one at a segment's end (where two meet, the
	 * next one owns the position). */
	const allowed = map.mappings.filter((mapping) => map.shouldReport(mapping.data, code));
	const find = (offset: number, inclusiveEnd: boolean) => {
		for (const mapping of allowed)
			for (let index = 0; index < mapping.generated.length; index++) {
				const start = mapping.generated[index] ?? 0;
				const length = mapping.generatedLengths[index] ?? 0;
				const inside = inclusiveEnd ? offset >= start && offset <= start + length : offset >= start && offset < start + length;
				if (inside) return { index, mapping, start };
			}

		return null;
	};
	const startHit = find(at, false) ?? find(at, true);
	if (!startHit || !find(end, true)) return null;
	const { index, mapping, start } = startHit;

	return positionOf(source(), (mapping.source[index] ?? 0) + Math.min(at - start, mapping.lengths[index] ?? 0));
};

let errors = 0;
const unparsed = new Set<string>();
for (const line of lines) {
	const match = ERROR_LINE.exec(line);
	if (!match) {
		if (line.startsWith(' ') && errors > 0) console.log(line);
		continue;
	}
	const [, rawFile = '', rawLine = '0', rawColumn = '0', severity, code, message] = match;
	const file = fileOf(rawFile);
	const map = maps[file];
	/* A syntax error in generated code means nothing was type-checked (a
	 * checker stops at one, as tsc does), whether or not vue-tsc would show
	 * it. Never hide that. */
	if (map && code?.startsWith('TS1')) unparsed.add(map.source);
	const position = map
		? sourcePosition(file, map, Number(rawLine), Number(rawColumn), (code ?? '').replace('TS', ''))
		: { column: Number(rawColumn), line: Number(rawLine) };
	// Not where vue-tsc reports it: the codegen's own scaffolding.
	if (!position) continue;
	errors++;
	console.log(`${map ? map.source : file}(${position.line},${position.column}): ${severity} ${code}: ${message}`);
}
if (phasesFile)
	writeFileSync(
		phasesFile,
		JSON.stringify({
			checkMs,
			codegenMs,
			mapMs: performance.now() - mapStarted,
			regeneratedFiles: regenerate.size,
			vueFiles: Object.keys(codegenResult.maps).length
		})
	);
if (unparsed.size > 0) {
	console.error(
		`vue-check: the TypeScript generated for ${[...unparsed].join(', ')} doesn't parse, so the project was NOT type-checked. Please report it with that file.`
	);
	process.exit(3);
}
if (checked.exitCode !== 0 && errors === 0 && checked.stderr.length > 0) process.stderr.write(checked.stderr);
process.exit(errors > 0 ? 2 : checked.exitCode === 0 ? 0 : 1);
