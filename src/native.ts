import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, join, relative } from 'node:path';

/*
 * The native codegen (native/, see docs/NATIVE.md): a port of one exact
 * version of Vue's codegen, used only for a project that would run that
 * version with options the port reads. Anything else, and any file the port
 * declines, stays with the project's own JavaScript codegen, so the output is
 * the same either way.
 */

/** The versions of @vue/language-core the port reproduces. */
export const LANGUAGE_CORES = ['3.3.12', '3.3.13', '3.3.14'];
/** The @vue/compiler-dom patches whose parser the port reproduces (3.5.x). */
const COMPILER_DOM_PATCHES = [36, 45] as const;
const ENTITIES = /^7\.0\./;
/** Options that don't reach the code of a .vue file: where the types are
 * (given to the port per file), the Vue version (given too), and the
 * extensions of other kinds of file, which the port isn't asked about. */
const FREE_OPTIONS = new Set(['globalTypesPath', 'isDataAttribute', 'isHtmlAttribute', 'target', 'typesRoot', 'vitePressExtensions']);
/** The most source text handed to one run of the binary. */
const BATCH_BYTES = 16 * 1024 * 1024;

export type NativeMapping = {
	generated: number[];
	lengths: number[];
	source: number[];
	/** 0: no error is reported there; 1: all are; one of Vue's fixed rules; or
	 * 'stateful', a rule that counts the errors it sees (under
	 * `@vue-expect-error`), which is a function only Volar can evaluate. */
	verification: 0 | 1 | 'stateful' | 'ts2339' | 'ts2353' | 'ts6133';
};
export type NativeResult =
	| {
			generated: { id: string; languageId: string; mappings: NativeMapping[]; text: string };
			/** What the generated code imports (see importReader); null when the port can't say. */
			imports: string[] | null;
	  }
	| { poisonsLaterParses: boolean; unsupported: string };

export type NativeCodegen = {
	/** The generated code of each file, in order, or why the port leaves it to the JavaScript codegen. */
	generate: (files: { file: string; text: string }[]) => NativeResult[];
	/** Whether a file has changed how the files after it parse (see below). */
	parserStateLeaked: () => boolean;
};

type VueOptions = Record<string, unknown> & { target?: number; typesRoot?: string };
type Core = {
	CompilerOptionsResolver?: new (ts: unknown) => { build: (defaults: VueOptions) => VueOptions };
	getDefaultCompilerOptions?: (target?: number, lib?: string, strictTemplates?: boolean, typesRoot?: string) => VueOptions;
};

const versionOf = (packageJson: string) => (JSON.parse(readFileSync(packageJson, 'utf8')) as { version: string }).version;

/** The binary npm installed for this platform (one of the optional
 * @absolutejs/vue-check-<platform> packages), when it is this version's. */
const installedBinary = () => {
	const platform = { darwin: 'darwin', linux: 'linux', win32: 'windows' }[process.platform as string];
	const arch = { arm64: 'arm64', x64: 'x64' }[process.arch as string];
	if (!platform || !arch) return null;
	try {
		const manifest = createRequire(import.meta.url).resolve(`@absolutejs/vue-check-${platform}-${arch}/package.json`);
		if (versionOf(manifest) !== versionOf(join(import.meta.dir, '..', 'package.json'))) return null;

		return join(dirname(manifest), 'bin', platform === 'windows' ? 'vue-check-codegen.exe' : 'vue-check-codegen');
	} catch {
		return null;
	}
};

/** VUE_CHECK_NATIVE=0 turns the native codegen off and VUE_CHECK_NATIVE=<path>
 * names a binary; otherwise the installed one, or one built in this checkout. */
const binaryPath = () => {
	const configured = process.env.VUE_CHECK_NATIVE;
	if (configured === '0' || configured === 'false') return null;
	const path = configured || installedBinary() || join(import.meta.dir, '..', 'native', 'target', 'release', 'vue-check-codegen');

	return existsSync(path) ? path : null;
};

/* The import reader (native/src/imports) is a port of TypeScript's
 * preProcessFile, of the scanner under it and of what those two read. A
 * TypeScript it reproduces is one where all of that is, text for text, what
 * was ported: these are hashes of that source (importReaderSignature), each
 * compared with the port value for value (native/oracle/run-imports.sh). */
const IMPORT_READERS = new Set([
	/* TypeScript 5.7.3, 5.8.2, 5.8.3, 5.9.2 and 5.9.3. */
	'924fcc09914b18743f6c02be832ac0f242116cc3ee4c2f42bb7cbe01545c2db1',
	/* TypeScript 6.0.2 and 6.0.3. */
	'35a547da03174667bf667d928e72724c6b1a601c7ea5d73de6ed3d1e111529b2',
	/* typescript-native-bridge 6.0.3-bridge.19.tsgo.7.0.2 (6.0.3's, bundled with one local renamed). */
	'192b9c616d9ef14401b3d6337b781544cbb17af84e9b27da8015cb09a9e924a3'
]);
const PORTED_FUNCTIONS = [
	'preProcessFile',
	'createScanner',
	'isConflictMarkerTrivia',
	'scanConflictMarkerTrivia',
	'isShebangTrivia',
	'scanShebangTrivia',
	'isWhiteSpaceSingleLine',
	'isLineBreak',
	'isIdentifierStart',
	'isIdentifierPart',
	'isUnicodeIdentifierStart',
	'isUnicodeIdentifierPart',
	'lookupInUnicodeMap',
	'isDigit',
	'isOctalDigit',
	'isHexDigit',
	'charSize',
	'isASCIILetter',
	'isWordCharacter',
	'codePointAt',
	'utf16EncodeAsString'
];
/* The keywords themselves aren't here: preProcessFile treats a keyword and
 * an identifier alike, apart from the eight it names in its own text. */
const PORTED_VARIABLES = ['unicodeESNextIdentifierStart', 'unicodeESNextIdentifierPart', 'shebangTriviaRegex', 'mergeConflictMarkerLength', 'textToKeyword'];

/** A hash of the source of everything the import reader reproduces, in the
 * TypeScript at `file` (its lib/typescript.js); null when a part isn't found. */
export const importReaderSignature = (file: string) => {
	const source = readFileSync(file);
	const hash = createHash('sha256');
	const part = (start: string, end: string) => {
		const from = source.indexOf(start);
		const to = from < 0 ? -1 : source.indexOf(end, from);
		if (to < 0) return false;
		hash.update(source.subarray(from, to));

		return true;
	};
	for (const name of PORTED_FUNCTIONS) if (!part(`\nfunction ${name}(`, '\n}\n')) return null;
	for (const name of PORTED_VARIABLES) if (!part(`\nvar ${name} = `, ';\n')) return null;

	return hash.digest('hex');
};

/**
 * What each file imports, as the project's `ts.preProcessFile(text, true,
 * true).importedFiles` names them, read by the binary across the machine's
 * cores; null for a file the port leaves to TypeScript. Or null, when the
 * project's TypeScript (the one at `typescriptFile`) isn't one the port
 * reproduces.
 */
export const importReader = (typescriptFile: string): ((files: string[]) => (string[] | null)[]) | null => {
	const binary = binaryPath();
	if (!binary) return null;
	try {
		if (!IMPORT_READERS.has(importReaderSignature(typescriptFile) ?? '')) return null;
	} catch {
		return null;
	}

	return (files) => {
		if (files.length === 0) return [];
		const input = files.map((file) => `${JSON.stringify({ file })}\n`).join('');
		const run = spawnSync(binary, ['imports'], { encoding: 'utf8', input, maxBuffer: Number.MAX_SAFE_INTEGER });
		const lines = run.status === 0 ? run.stdout.split('\n').filter(Boolean) : [];
		if (lines.length !== files.length) throw new Error(`the native import reader failed: ${run.stderr || run.error?.message || `exit ${run.status}`}`);

		return lines.map((line) => (JSON.parse(line) as { imports?: string[] }).imports ?? null);
	};
};

/** The packages Vue's codegen would run with, when they are exactly the ones
 * the port reproduces; otherwise, which one isn't. */
const portedPackages = (coreRequire: NodeJS.Require) => {
	try {
		const languageCore = versionOf(coreRequire.resolve('@vue/language-core/package.json'));
		if (!LANGUAGE_CORES.includes(languageCore)) return `@vue/language-core ${languageCore}`;
		const compilerDom = coreRequire.resolve('@vue/compiler-dom/package.json');
		const compilerDomVersion = versionOf(compilerDom);
		const [major, minor, patch, ...rest] = compilerDomVersion.split(/[.-]/);
		const [first, last] = COMPILER_DOM_PATCHES;
		if (major !== '3' || minor !== '5' || rest.length > 0 || !(Number(patch) >= first && Number(patch) <= last))
			return `@vue/compiler-dom ${compilerDomVersion}`;
		const compilerCore = createRequire(compilerDom).resolve('@vue/compiler-core');
		const decode = createRequire(compilerCore).resolve('entities/decode');
		const entities = versionOf(join(decode.slice(0, decode.lastIndexOf('/entities/') + 10), 'package.json'));
		if (!ENTITIES.test(entities)) return `entities ${entities}`;

		return { compilerDom: coreRequire('@vue/compiler-dom') as { parse: (template: string) => { children: { type: number }[] } }, languageCore };
	} catch (error) {
		return `its packages could not be read (${error instanceof Error ? error.message : String(error)})`;
	}
};

/* The options the port reads as true or false, which a project may set: the
 * checks strict templates turn on, what a template's $attrs, $el, $refs and
 * $slots are typed from, and which attributes fall through. */
const PORTED_FLAGS = new Set([
	'strictVModel',
	'checkUnknownProps',
	'checkUnknownEvents',
	'checkUnknownDirectives',
	'checkUnknownComponents',
	'inferComponentDollarEl',
	'inferComponentDollarRefs',
	'inferTemplateDollarAttrs',
	'inferTemplateDollarEl',
	'inferTemplateDollarRefs',
	'inferTemplateDollarSlots',
	'fallthroughAttributes',
	'checkRequiredFallthroughAttributes',
	'jsxSlots'
]);
const isNames = (value: unknown): value is string[] => Array.isArray(value) && value.every((name) => typeof name === 'string');
/** Each macro or composable, with the list of names it goes by. */
const isNamesOf =
	(known: string[]) =>
	(value: unknown) =>
		typeof value === 'object' && value !== null && !Array.isArray(value) && Object.entries(value).every(([name, names]) => known.includes(name) && isNames(names));
/* The other options the port reads, each with the values it reads them as. */
const PORTED_OPTIONS: Record<string, (value: unknown) => boolean> = {
	composables: isNamesOf(['useAttrs', 'useCssModule', 'useSlots', 'useTemplateRef']),
	dataAttributes: isNames,
	fallthroughComponentNames: isNames,
	htmlAttributes: isNames,
	lib: (value) => typeof value === 'string',
	macros: isNamesOf(['defineProps', 'defineSlots', 'defineEmits', 'defineExpose', 'defineModel', 'defineOptions', 'withDefaults']),
	optionsWrapper: (value) => isNames(value) && (value.length === 0 || value.length === 2),
	resolveStyleClassNames: (value) => typeof value === 'boolean' || value === 'scoped',
	strictCssModules: (value) => typeof value === 'boolean'
};

/**
 * The project's options that aren't the defaults, for the binary; or
 * `{ changed }`, the first one the port doesn't read (or reads only as a
 * boolean, and the project gave something else).
 */
export const portedOptions = (core: unknown, ts: unknown, vueOptions: unknown): { changed: string } | { options: Record<string, unknown> } => {
	const { CompilerOptionsResolver, getDefaultCompilerOptions } = core as Core;
	const project = vueOptions as VueOptions;
	if (
		typeof getDefaultCompilerOptions !== 'function' ||
		typeof CompilerOptionsResolver !== 'function' ||
		typeof project.target !== 'number' ||
		typeof project.typesRoot !== 'string'
	)
		return { changed: 'vueCompilerOptions' };
	/* What a project that sets no option gets. */
	const defaults = new CompilerOptionsResolver(ts).build(getDefaultCompilerOptions(project.target, 'vue', false, project.typesRoot));
	const options: Record<string, unknown> = {};
	for (const key of new Set([...Object.keys(defaults), ...Object.keys(project)])) {
		if (FREE_OPTIONS.has(key)) continue;
		const value = project[key];
		if (JSON.stringify(value) === JSON.stringify(defaults[key])) continue;
		const ported = PORTED_FLAGS.has(key) ? typeof value === 'boolean' : (PORTED_OPTIONS[key]?.(value) ?? false);
		if (!ported) return { changed: key };
		options[key] = value;
	}

	return { options };
};

/**
 * The native codegen for this project, or why it isn't one the port
 * reproduces. `coreRequire` resolves from the @vue/language-core the
 * project's codegen runs.
 */
export const nativeCodegen = (core: unknown, ts: unknown, coreRequire: NodeJS.Require, vueOptions: unknown): NativeCodegen | { unavailable: string } => {
	const binary = binaryPath();
	if (!binary) return { unavailable: 'no binary' };
	const options = vueOptions as VueOptions;
	const packages = portedPackages(coreRequire);
	if (typeof packages === 'string') return { unavailable: `the project runs ${packages}` };
	const ported = portedOptions(core, ts, options);
	if ('changed' in ported) return { unavailable: `the project sets the Vue compiler option ${ported.changed}` };
	const typesRoot = options.typesRoot ?? '';
	/* As the codegen's reference lines name the types folder (generateGlobalTypesReference). */
	const typesPathOf = (file: string) => {
		if (!isAbsolute(typesRoot)) return typesRoot;
		const relativePath = relative(dirname(file), typesRoot);

		return relativePath !== typesRoot && !relativePath.startsWith('./') && !relativePath.startsWith('../') ? `./${relativePath}` : relativePath;
	};

	return {
		generate: (files) => {
			const results: NativeResult[] = [];
			for (let start = 0; start < files.length; ) {
				let end = start;
				let bytes = 0;
				while (end < files.length && (end === start || bytes + (files[end]?.text.length ?? 0) <= BATCH_BYTES)) {
					bytes += files[end]?.text.length ?? 0;
					end += 1;
				}
				const input = files
					.slice(start, end)
					.map(({ file, text }) => `${JSON.stringify({ core: packages.languageCore, file, options: ported.options, target: options.target, text, typesPath: typesPathOf(file) })}\n`)
					.join('');
				const run = spawnSync(binary, ['generate'], { encoding: 'utf8', input, maxBuffer: Number.MAX_SAFE_INTEGER });
				const lines = run.status === 0 ? run.stdout.split('\n').filter(Boolean) : [];
				if (lines.length !== end - start) throw new Error(`the native codegen failed: ${run.stderr || run.error?.message || `exit ${run.status}`}`);
				for (const line of lines) results.push(JSON.parse(line) as NativeResult);
				start = end;
			}

			return results;
		},
		/* Vue's parser keeps its v-pre state between files and never clears it:
		 * a file that ends inside v-pre, or that makes the parser throw
		 * halfway, changes how every file after it parses. True once that has
		 * happened in this process. */
		parserStateLeaked: () => packages.compilerDom.parse('{{ a }}').children[0]?.type !== 5
	};
};
