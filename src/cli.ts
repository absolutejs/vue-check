#!/usr/bin/env bun
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, relative, resolve } from 'node:path';
import * as ownSourceMap from '@volar/source-map';
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
 *   vue-check [-p tsconfig.json] [--threads=N] [--out-dir=.vue-check]
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
/* Where the generated files go: .vue-check/ in the project, or --out-dir
 * (a tool that keeps a cache folder, such as absolute typecheck). */
const outDir = resolve(root, argument('--out-dir') ?? '.vue-check');
/* VUE_CHECK_PHASES=<file> writes how long each step took: codegen and the
 * check (each its own process) and mapping errors back (this one). */
const phasesFile = process.env.VUE_CHECK_PHASES;

const codegenStarted = performance.now();
const codegen = Bun.spawnSync([process.execPath, join(import.meta.dir, 'codegen.ts'), root, tsconfig, outDir], {
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
const checkConfig = join(outDir, 'tsconfig.json');

const threads = argument('--threads');
/* Anything else is a compiler option for bun check (e.g. --skipLibCheck), as vue-tsc passes them to tsc. */
const OWN = new Set(['-p', '--project', '--threads', '--out-dir']);
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
		: [
				process.execPath,
				'check',
				'-p',
				checkConfig,
				'--no-pretty',
				'--all',
				// Type-check, never a project's own `check` script in its place.
				'--no-script',
				...(threads ? [`--threads=${threads}`] : []),
				...passThrough
			],
	{ cwd: root, stderr: 'pipe', stdout: 'pipe' }
);
const checkMs = performance.now() - checkStarted;
const mapStarted = performance.now();

const configChain = (config: string, seen = new Set<string>()): string[] => {
	if (seen.has(config) || !existsSync(config)) return [];
	seen.add(config);
	const extended = /"extends"\s*:\s*("[^"]+"|\[[^\]]*\])/.exec(readFileSync(config, 'utf8'))?.[1] ?? '';
	const parents = [...extended.matchAll(/"([^"]+)"/g)].map(([, name = '']) =>
		name.startsWith('.') ? resolve(dirname(config), name.endsWith('.json') ? name : `${name}.json`) : ''
	);

	return [config, ...parents.filter(Boolean).flatMap((parent) => configChain(parent, seen))];
};
const optionPosition = (message: string) => {
	const names = [...message.matchAll(/'([A-Za-z]+)'/g)].map(([, name]) => name);
	for (const config of configChain(tsconfig)) {
		const text = readFileSync(config, 'utf8');
		for (const name of names) {
			const index = text.indexOf(`"${name}"`);
			if (index < 0) continue;
			const { column, line } = positionOf(text, index);

			return `${relative(root, config)}(${line},${column})`;
		}
	}

	return `${relative(root, tsconfig)}(1,1)`;
};

/* A message that names a generated file names its Vue file, as vue-tsc's do
 * (its virtual file is the .vue file itself). */
const generatedPaths = Object.entries(codegenResult.maps).map(
	([file, map]) => [join(root, file), join(root, map.source)] as const
);
const sourcePaths = (message: string) =>
	message.includes(outDir)
		? generatedPaths.reduce((text, [generatedFile, source]) => text.split(generatedFile).join(source), message)
		: message;

const ERROR_LINE = /^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$/;
const GLOBAL_ERROR = /^error TS\d+: /;
const lines = checked.stdout.toString().split('\n');
const fileOf = (rawFile: string) => relative(root, resolve(root, rawFile));
/* The maps, as data. A file whose rules aren't all data (see serialize.ts),
 * or with an error code the fixed rules weren't evaluated for, is
 * regenerated and decided by Volar itself, as vue-tsc would. */
const knownCodes = new Set(codegenResult.knownCodes);
const maps: Record<string, SourceMap> = {};
/* The maps made again by Volar itself (below): their rules are its functions. */
const liveMaps = new Set<SourceMap>();
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
		if (map) {
			maps[file] = map;
			liveMaps.add(map);
		}
	}
}

/* Where each line of a text starts, worked out once per text: a project can
 * have thousands of errors in a few files. */
const lineStartsOf = new Map<string, number[]>();
const lineStarts = (text: string) => {
	let starts = lineStartsOf.get(text);
	if (!starts) {
		starts = [0];
		for (let index = text.indexOf('\n'); index !== -1; index = text.indexOf('\n', index + 1)) starts.push(index + 1);
		lineStartsOf.set(text, starts);
	}

	return starts;
};
const offsetOf = (text: string, line: number, column: number) => {
	const starts = lineStarts(text);

	return (starts[Math.min(line - 1, starts.length)] ?? text.length + 1) + column - 1;
};
const positionOf = (text: string, offset: number) => {
	const starts = lineStarts(text);
	let low = 0;
	let high = starts.length - 1;
	while (low < high) {
		const middle = (low + high + 1) >> 1;
		if ((starts[middle] ?? 0) <= offset) low = middle;
		else high = middle - 1;
	}

	return { column: offset - (starts[low] ?? 0) + 1, line: low + 1 };
};

/* Volar's own SourceMap (the project's, as vue-tsc loads it), so an error is
 * mapped exactly as vue-tsc maps it, filter calls and all. Loaded lazily:
 * a clean project never needs it. */
type VolarMapping = { data: unknown; generatedLengths: number[]; generatedOffsets: number[]; lengths: number[]; sourceOffsets: number[] };
type VolarSourceMap = {
	toSourceRange: (start: number, end: number, fallbackToAnyMatch: boolean, filter: (data: unknown) => boolean) => Iterable<[number, number, ...unknown[]]>;
};
let VolarSourceMapClass: (new (mappings: VolarMapping[]) => VolarSourceMap) | null = null;
const volarSourceMap = () => {
	if (VolarSourceMapClass) return VolarSourceMapClass;
	const projectRequire = createRequire(join(root, 'package.json'));
	const chain = ['vue-tsc/package.json', '@vue/language-core/package.json', '@volar/language-core/package.json'];
	const requirers: NodeJS.Require[] = [];
	let current: NodeJS.Require = projectRequire;
	for (const step of chain) {
		try {
			current = createRequire(current.resolve(step));
			requirers.unshift(current);
		} catch {
			break;
		}
	}
	for (const requirer of [...requirers, projectRequire]) {
		try {
			VolarSourceMapClass = (requirer('@volar/source-map') as { SourceMap: typeof VolarSourceMapClass }).SourceMap;
			if (VolarSourceMapClass) return VolarSourceMapClass;
		} catch {
			// Not resolvable from here.
		}
	}
	VolarSourceMapClass = (ownSourceMap as unknown as { SourceMap: NonNullable<typeof VolarSourceMapClass> }).SourceMap;

	return VolarSourceMapClass;
};
const volarMaps = new Map<SourceMap, VolarSourceMap>();

/* tsc-style output gives where an error starts, not its length. A comment
 * there (an unused @ts-expect-error directive) is the span, as TypeScript
 * reports it; otherwise the token there (a name or a string). */
const SPAN = /(?:\/\/[^\n]*|\/\*[\s\S]*?\*\/|[\p{L}\p{N}_$]+|"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')/uy;

/* The error's length, which tsc-style output leaves out. TypeScript reports
 * an error on a syntax node, and punctuation isn't one, so the innermost node
 * that starts where the error starts is its span ({} for an argument, [x] for
 * a computed name, the identifier for a name). A comment (an unused
 * @ts-expect-error) isn't a node: its text is the span. TypeScript's parser,
 * the project's, is loaded only when a generated file has an error. */
type Ts = typeof import('typescript');
let parser: Ts | null = null;
const projectTypeScript = (): Ts => {
	if (parser) return parser;
	const projectRequire = createRequire(join(root, 'package.json'));
	for (const from of ['vue-tsc/package.json', 'package.json'])
		try {
			parser = createRequire(projectRequire.resolve(from))('typescript') as Ts;

			return parser;
		} catch {
			// Not resolvable from here.
		}
	parser = createRequire(import.meta.path)('typescript') as Ts;

	return parser;
};
const parsedFiles = new Map<string, import('typescript').SourceFile>();
const parsedFile = (file: string, text: string) => {
	const ts = projectTypeScript();
	let sourceFile = parsedFiles.get(file);
	if (!sourceFile) {
		const kind = file.endsWith('x') ? (file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.JSX) : file.endsWith('.js') ? ts.ScriptKind.JS : ts.ScriptKind.TS;
		sourceFile = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, kind);
		parsedFiles.set(file, sourceFile);
	}

	return sourceFile;
};
/** The parser's own diagnostic for an error the checker printed, when the
 * error is a syntax error. */
const syntaxErrorAt = (file: string, text: string, position: number, code: string) =>
	(
		(parsedFile(file, text) as unknown as { parseDiagnostics?: { code: number; length: number; messageText: unknown; start: number }[] })
			.parseDiagnostics ?? []
	).find((diagnostic) => diagnostic.start === position && String(diagnostic.code) === code);
/* An error's length read from the text, for the two tokens most errors start
 * on. A plain name: ASCII, not a keyword, and not one of the objects a template's
 * names are read from. In a file that isn't JSX, TypeScript's parser makes
 * exactly an Identifier of it, and no smaller node starts there, so its
 * length is the error's without parsing the file. Anything else (a keyword
 * starts a larger node, a name outside ASCII may be longer than this reads)
 * is asked of the parser. */
const PLAIN_NAME = /^[A-Za-z_$][A-Za-z0-9_$]*/;
const KEYWORDS = new Set(
	(
		'abstract accessor any as asserts assert async await bigint boolean break case catch class const constructor continue debugger declare default defer delete ' +
		'do else enum export extends false finally for from function get global if implements import in infer instanceof interface intrinsic is keyof let module ' +
		'namespace never new null number object of out override package private protected public readonly require return satisfies set static string super ' +
		'switch symbol this throw true try type typeof undefined unique unknown using var void while with yield'
	).split(' ')
);
/* A string on one line, as TypeScript scans it: to the closing quote, a
 * backslash taking the character after it. It is one node, and none smaller
 * starts there. One the pattern doesn't end (a line continuation, a missing
 * quote) is asked of the parser. */
const PLAIN_STRING = /(?:"(?:[^"\\\r\n\u2028\u2029]|\\[^\r\n\u2028\u2029])*"|'(?:[^'\\\r\n\u2028\u2029]|\\[^\r\n\u2028\u2029])*')/y;
const plainTokenAt = (file: string, text: string, position: number) => {
	if (file.endsWith('x')) return null;
	const first = text.charCodeAt(position);
	if (first === 34 || first === 39) {
		PLAIN_STRING.lastIndex = position;

		return PLAIN_STRING.exec(text)?.[0].length ?? null;
	}
	const name = plainName(text, position);
	if (name === null || KEYWORDS.has(name)) return null;
	if (!CONTEXT_OBJECTS.has(name)) return name.length;
	/* A name read from the component, `__VLS_ctx.name`: the error is on the
	 * access (see parsedSpanAt), which ends with the name. After `typeof` the
	 * two may be a type's qualified name and not an access, so that is asked
	 * of the parser, as is anything but a dot and a plain name. */
	if (text.charCodeAt(position + name.length) !== 46 || TYPEOF_BEFORE.test(text.slice(Math.max(0, position - 64), position))) return null;
	const property = plainName(text, position + name.length + 1);

	return property === null ? null : name.length + 1 + property.length;
};
const TYPEOF_BEFORE = /typeof\s*$/;
const plainName = (text: string, position: number) => {
	const name = PLAIN_NAME.exec(text.slice(position, position + 256))?.[0];
	if (name === undefined || name.length === 256) return null;
	const next = text.charCodeAt(position + name.length);

	// More of the name may follow: an escape, or a character outside ASCII.
	return next === 92 || next > 127 ? null : name;
};
/* How many spans were read from the text, and how many needed the parser. */
const spans = { parsed: 0, read: 0 };
const parsedSpanAt = (file: string, text: string, position: number, code: string, lexical: string | undefined) => {
	spans.parsed++;
	const ts = projectTypeScript();
	const sourceFile = parsedFile(file, text);
	/* A syntax error's span is the parser's own diagnostic: exact. */
	const syntax = syntaxErrorAt(file, text, position, code);
	if (syntax) return syntax.length;
	let found: import('typescript').Node | undefined;
	const visit = (node: import('typescript').Node) => {
		if (position < node.getStart(sourceFile) || position >= node.end) return;
		if (node.getStart(sourceFile) === position) found = node;
		ts.forEachChild(node, visit);
	};
	ts.forEachChild(sourceFile, visit);
	/* A name read from the component is written `__VLS_ctx.name`, and Vue maps
	 * the two together as the name. TypeScript never reports an error on
	 * `__VLS_ctx` alone: one that starts there is on the access, or on an
	 * expression that begins with it (`__VLS_ctx.list[index]`), and the access
	 * is the least of those. */
	if (found && ts.isIdentifier(found) && CONTEXT_OBJECTS.has(found.text) && ts.isPropertyAccessExpression(found.parent) && found.parent.expression === found)
		found = found.parent;

	return Math.max(1, found ? found.end - position : (lexical?.length ?? 1));
};
const spanAt = (file: string, text: string, position: number, code: string) => {
	SPAN.lastIndex = position;
	const lexical = SPAN.exec(text)?.[0];
	if (lexical?.startsWith('//') || lexical?.startsWith('/*')) return lexical.length;
	/* Only once the checker is known to have type-checked: until then an
	 * error may be a syntax error, whose span is the parser's. */
	const name = typeChecked ? plainTokenAt(file, text, position) : null;
	if (name === null) return parsedSpanAt(file, text, position, code, lexical);
	if (verifySpans && parsedSpanAt(file, text, position, code, lexical) !== name)
		throw new Error(`vue-check: ${file} at ${position}: TS${code} spans ${name} read from the text, and not as parsed`);
	spans.read++;

	return name;
};
/* What Vue's codegen reads a template's names from. */
const CONTEXT_OBJECTS = new Set(['__VLS_ctx', '__VLS_dollars']);

/**
 * A generated-file error, where vue-tsc would report it: the position in the
 * .vue file, or null when vue-tsc wouldn't report it. Volar's toSourceRange
 * with its diagnostic filter (shouldReportDiagnostics, or the same rules as
 * data), as Volar's transformDiagnostic calls it for vue-tsc.
 */
const fileTexts = new Map<string, string>();
const fileText = (file: string) => {
	let text = fileTexts.get(file);
	if (text === undefined) {
		text = readFileSync(join(root, file), 'utf8');
		fileTexts.set(file, text);
	}

	return text;
};
/* VUE_CHECK_VERIFY_SPANS: also do what the shortcut below skips, and fail if
 * the two disagree. For the benchmark. */
const verifySpans = Boolean(process.env.VUE_CHECK_VERIFY_SPANS);
/* The stretches of a generated file covered by a mapping that reports a
 * given error code, merged and in order. Volar maps an error only through a
 * mapping that holds where it starts (both ends included) and reports its
 * code, so an error that starts outside all of them maps nowhere: known from
 * the map's numbers alone, without building Volar's own lookup for a file
 * whose errors are all in Vue's scaffolding. Only for a map whose rules are
 * data: asking one of Volar's own functions here would count as a call. */
type Covered = { ends: number[]; starts: number[] };
const coveredOf = new Map<SourceMap, Map<string, Covered>>();
const covered = (map: SourceMap, code: string) => {
	let byCode = coveredOf.get(map);
	if (!byCode) {
		byCode = new Map();
		coveredOf.set(map, byCode);
	}
	let found = byCode.get(code);
	if (found) return found;
	const ranges: [number, number][] = [];
	for (const mapping of map.mappings) {
		if (!map.shouldReport(mapping.data, code)) continue;
		const lengths = mapping.generatedLengths ?? mapping.lengths;
		for (const [index, start] of mapping.generated.entries()) ranges.push([start, start + (lengths[index] ?? 0)]);
	}
	ranges.sort((first, second) => first[0] - second[0]);
	found = { ends: [], starts: [] };
	for (const [start, end] of ranges) {
		const last = found.ends.length - 1;
		if (last >= 0 && start <= (found.ends[last] ?? 0)) found.ends[last] = Math.max(found.ends[last] ?? 0, end);
		else {
			found.starts.push(start);
			found.ends.push(end);
		}
	}
	byCode.set(code, found);

	return found;
};
const isCovered = (map: SourceMap, code: string, offset: number) => {
	const { ends, starts } = covered(map, code);
	let low = 0;
	let high = starts.length - 1;
	while (low <= high) {
		const middle = (low + high) >> 1;
		if ((starts[middle] ?? 0) > offset) high = middle - 1;
		else if ((ends[middle] ?? 0) < offset) low = middle + 1;
		else return true;
	}

	return false;
};
const sourcePosition = (file: string, map: SourceMap, line: number, column: number, code: string) => {
	const generated = fileText(file);
	const position = offsetOf(generated, line, column);
	const start = position - map.shift;
	const outside = !liveMaps.has(map) && !isCovered(map, code, start);
	if (outside && !verifySpans) return null;
	let volar = volarMaps.get(map);
	if (!volar) {
		const SourceMapClass = volarSourceMap();
		volar = new SourceMapClass(
			map.mappings.map((mapping) => ({
				data: mapping.data,
				generatedLengths: mapping.generatedLengths,
				generatedOffsets: mapping.generated,
				lengths: mapping.lengths,
				sourceOffsets: mapping.source
			}))
		);
		volarMaps.set(map, volar);
	}
	const shouldReport = (data: unknown) => map.shouldReport(data, code);
	const mapped = (end: number) => {
		for (const [sourceStart] of volar.toSourceRange(start, end, true, shouldReport)) return sourceStart;

		return null;
	};
	/* Volar maps a range by where it starts: the mappings that hold the start
	 * and report this code, and then the end through the same mapping or
	 * another. With no such mapping for the start there is none for the range,
	 * whatever its length: most of what a checker reports in generated code is
	 * in Vue's scaffolding, which maps nowhere, and the error's length (a
	 * parse of the file with TypeScript) isn't needed to know it. Not for a
	 * map whose rules are Volar's own functions, which count their calls. */
	const mappedByRange = () => mapped(start + spanAt(file, generated, position, code));
	if (outside) {
		if (mapped(start) !== null || mappedByRange() !== null)
			throw new Error(`vue-check: ${file}(${line},${column}) TS${code} maps, and no mapping covers where it starts`);

		return null;
	}
	if (!liveMaps.has(map) && mapped(start) === null) {
		if (verifySpans && mappedByRange() !== null)
			throw new Error(`vue-check: ${file}(${line},${column}) TS${code} maps by its range and not by its start`);

		return null;
	}
	const sourceStart = mappedByRange();

	return sourceStart === null ? null : positionOf(fileText(map.source), sourceStart);
};

/* Errors only a type check finds (an unused name, an unknown one, a type that
 * doesn't fit). A checker that behaves like tsc reports syntax errors alone
 * when there is one, so one of these means it found none: nothing it printed
 * needs asking TypeScript's parser whether it is a syntax error. */
const TYPE_CHECK_ONLY = new Set(['TS2304', 'TS2322', 'TS2339', 'TS2344', 'TS2345', 'TS6133', 'TS6196']);
const typeChecked = lines.some((line) => TYPE_CHECK_ONLY.has(ERROR_LINE.exec(line)?.[5] ?? ''));
/* A project's `rootDir` is checked against where each file is, and a
 * generated file is in .vue-check/, outside it. TS6059 names the file and the
 * rootDir: for a generated file whose Vue file is under that rootDir the
 * error is this tool's own doing and vue-tsc has none. One whose Vue file is
 * outside it too is the project's, and is reported for the Vue file. */
const ROOT_DIR_ERROR = /^File '([^']+)' is not under 'rootDir' '([^']+)'/;
const sourceOfGenerated = new Map(generatedPaths);
const isOwnRootDirError = (message: string) => {
	const [, file = '', rootDir = ''] = ROOT_DIR_ERROR.exec(message) ?? [];
	const source = sourceOfGenerated.get(file);
	if (!source) return false;
	const inside = relative(rootDir, source);

	return inside !== '' && !inside.startsWith('..');
};
let errors = 0;
/* Every error the checker printed, before mapping drops vue-tsc's hidden ones. */
let reported = 0;
let droppedJavaScriptGrammar = false;
/* Syntax errors the checker stopped on in generated code that map nowhere:
 * artifacts of codegen, which vue-tsc hides (and then type-checks). The
 * first of each file. */
const hiddenSyntaxErrors = new Map<string, { line: number; message: string }>();
for (const line of lines) {
	const match = ERROR_LINE.exec(line);
	if (!match) {
		/* An error with no position (an invalid option, a missing file): the
		 * project's own, printed as tsc prints it. */
		if (GLOBAL_ERROR.test(line)) {
			if (line.startsWith('error TS6059: ') && isOwnRootDirError(line.slice('error TS6059: '.length))) continue;
			reported++;
			errors++;
			console.log(line);
		} else if (line.startsWith(' ') && errors > 0) console.log(line);
		continue;
	}
	reported++;
	const [, rawFile = '', rawLine = '0', rawColumn = '0', severity, code, message] = match;
	if (code === 'TS6059' && isOwnRootDirError(message ?? '')) {
		reported--;
		continue;
	}
	const file = fileOf(rawFile);
	if (file === relative(root, checkConfig)) {
		/* An option error the generated config inherits: where vue-tsc reports
		 * it, at the option in the project's own config (or a config it extends). */
		const at = optionPosition(message ?? '');
		console.log(`${at}: ${severity} ${code}: ${sourcePaths(message ?? '')}`);
		errors++;
		continue;
	}
	const map = maps[file];
	const position = map
		? sourcePosition(file, map, Number(rawLine), Number(rawColumn), (code ?? '').replace('TS', ''))
		: { column: Number(rawColumn), line: Number(rawLine) };
	// Not where vue-tsc reports it: the codegen's own scaffolding.
	if (!position) {
		if (map && /^TS8\d{3}$/.test(code ?? '')) droppedJavaScriptGrammar = true;
		if (map && !typeChecked && !hiddenSyntaxErrors.has(map.source)) {
			const generated = fileText(file);
			const syntax = syntaxErrorAt(file, generated, offsetOf(generated, Number(rawLine), Number(rawColumn)), (code ?? '').replace('TS', ''));
			if (syntax)
				hiddenSyntaxErrors.set(map.source, {
					line: Number(rawLine),
					message: typeof syntax.messageText === 'string' ? syntax.messageText : 'syntax error'
				});
		}
		continue;
	}
	errors++;
	console.log(`${map ? map.source : file}(${position.line},${position.column}): ${severity} ${code}: ${sourcePaths(message ?? '')}`);
}
if (phasesFile)
	writeFileSync(
		phasesFile,
		JSON.stringify({
			checkMs,
			codegenMs,
			mapMs: performance.now() - mapStarted,
			regeneratedFiles: regenerate.size,
			spansParsed: spans.parsed,
			spansRead: spans.read,
			vueFiles: Object.keys(codegenResult.maps).length
		})
	);
/* A file the codegen can't make checkable as vue-tsc checks it. */
const fatal = unparsedFiles.filter((item) => item.fatal);
if (fatal.length > 0) {
	for (const item of fatal) console.error(`vue-check: ${item.file}: ${item.message}.`);
	console.error('vue-check: the project was NOT type-checked as vue-tsc would.');
	process.exit(3);
}
/* The checker stopped on JavaScript grammar errors (TS8xxx are syntax errors
 * to tsc) in generated code that vue-tsc hides: nothing was type-checked. */
if (droppedJavaScriptGrammar && errors === 0) {
	console.error(
		'vue-check: the checker stopped on TypeScript-only syntax in generated JavaScript, so the project was NOT type-checked. Please report it.'
	);
	process.exit(3);
}
/* Generated code with a syntax error that maps nowhere. A checker that
 * behaves like tsc reports only syntax errors once there is one, and
 * type-checks nothing. When one of them is the user's own (it maps back),
 * that is vue-tsc's output too. When every one was hidden, vue-tsc would have
 * type-checked and the checker didn't: never report that as clean. The
 * checker parses every file, so its own output says whether this happened. */
if (hiddenSyntaxErrors.size > 0 && errors === 0) {
	for (const [file, item] of hiddenSyntaxErrors)
		console.error(`vue-check: the TypeScript generated for ${file} doesn't parse (generated line ${item.line}: ${item.message}).`);
	console.error('vue-check: the project was NOT type-checked. Please report the file(s) above.');
	process.exit(3);
}
/* The checker failed without printing a single error: it didn't check (a
 * crash, a bad config). Never report that as clean. (Errors it printed that
 * vue-tsc hides are a clean result, as with vue-tsc.) */
if (checked.exitCode !== 0 && reported === 0) {
	process.stderr.write(checked.stdout);
	process.stderr.write(checked.stderr);
	console.error('vue-check: the checker failed (above), so the project was NOT type-checked.');
	process.exit(3);
}
process.exit(errors > 0 ? 2 : 0);
