import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, join, relative } from 'node:path';

/*
 * The native codegen (native/, see docs/NATIVE.md): a port of one exact
 * version of Vue's codegen, used only for a project that would run that
 * version with its default options. Anything else, and any file the port
 * declines, stays with the project's own JavaScript codegen, so the output is
 * the same either way.
 */

const LANGUAGE_CORE = '3.3.14';
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
	/** 0: no error is reported there; 1: all are; or one of Vue's fixed rules. */
	verification: 0 | 1 | 'ts2353' | 'ts6133';
};
export type NativeResult =
	| { generated: { id: string; languageId: string; mappings: NativeMapping[]; text: string } }
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

const binaryPath = () => {
	const configured = process.env.VUE_CHECK_NATIVE;
	if (configured === '0' || configured === 'false') return null;
	const path = configured || join(import.meta.dir, '..', 'native', 'target', 'release', 'vue-check-codegen');

	return existsSync(path) ? path : null;
};

const versionOf = (packageJson: string) => (JSON.parse(readFileSync(packageJson, 'utf8')) as { version: string }).version;

/** The packages Vue's codegen would run with, when they are exactly the ones
 * the port reproduces; otherwise, which one isn't. */
const portedPackages = (coreRequire: NodeJS.Require) => {
	try {
		const languageCore = versionOf(coreRequire.resolve('@vue/language-core/package.json'));
		if (languageCore !== LANGUAGE_CORE) return `@vue/language-core ${languageCore}`;
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

		return { compilerDom: coreRequire('@vue/compiler-dom') as { parse: (template: string) => { children: { type: number }[] } } };
	} catch (error) {
		return `its packages could not be read (${error instanceof Error ? error.message : String(error)})`;
	}
};

/** The first of the project's options that isn't the default the port was written for. */
const changedOption = (core: Core, ts: unknown, vueOptions: VueOptions) => {
	if (
		typeof core.getDefaultCompilerOptions !== 'function' ||
		typeof core.CompilerOptionsResolver !== 'function' ||
		typeof vueOptions.target !== 'number' ||
		typeof vueOptions.typesRoot !== 'string'
	)
		return 'vueCompilerOptions';
	/* What a project that sets no option gets. */
	const defaults = new core.CompilerOptionsResolver(ts).build(core.getDefaultCompilerOptions(vueOptions.target, 'vue', false, vueOptions.typesRoot));
	const keys = new Set([...Object.keys(defaults), ...Object.keys(vueOptions)]);
	for (const key of keys) {
		if (FREE_OPTIONS.has(key)) continue;
		if (JSON.stringify(vueOptions[key]) !== JSON.stringify(defaults[key])) return key;
	}

	return null;
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
	const changed = changedOption(core as Core, ts, options);
	if (changed) return { unavailable: `the project sets the Vue compiler option ${changed}` };
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
					.map(({ file, text }) => `${JSON.stringify({ file, target: options.target, text, typesPath: typesPathOf(file) })}\n`)
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
