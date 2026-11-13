import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, parse, relative, resolve } from 'node:path';
import { createRequire } from 'node:module';
import * as ownVolar from '@volar/language-core';
import * as ownCore from '@vue/language-core';
import ownTs from 'typescript';

/* The project's own codegen, as its vue-tsc would use: the same
 * @vue/language-core (and its TypeScript) the project installed, so
 * vue-check generates exactly what vue-tsc checks. Ours only when the
 * project has none. */
const projectModules = (root: string) => {
	const projectRequire = createRequire(join(root, 'package.json'));
	/* Resolved through the project's own vue-tsc when it has one (package
	 * managers often nest @vue/language-core under it), then the project,
	 * then ours. */
	const installed = (requirer: NodeJS.Require, name: string) => {
		const resolved = requirer.resolve(name);
		if (!resolved.includes('/node_modules/')) throw new Error(`${name} is not installed here`);

		return resolved;
	};
	const fromVueTsc = (() => {
		try {
			return createRequire(installed(projectRequire, 'vue-tsc/package.json'));
		} catch {
			return null;
		}
	})();
	/* Only what the project installed: Bun's require falls back to its global
	 * install cache for a package the project doesn't have, which could hand
	 * back any version (for typescript, TypeScript 7's native build, which
	 * has no JavaScript API). */
	const load = <T>(name: string, fallback: T, requirers: (NodeJS.Require | null)[], usable: (value: T) => boolean = () => true): T => {
		for (const requirer of requirers) {
			if (!requirer) continue;
			try {
				installed(requirer, name);
				const loaded = requirer(name) as T;
				if (usable(loaded)) return loaded;
			} catch {
				// Not resolvable from here: try the next.
			}
		}

		return fallback;
	};
	const coreRequire = (() => {
		for (const requirer of [fromVueTsc, projectRequire]) {
			if (!requirer) continue;
			try {
				return createRequire(installed(requirer, '@vue/language-core/package.json'));
			} catch {
				// Not here.
			}
		}

		return null;
	})();

	return {
		core: load<typeof ownCore>('@vue/language-core', ownCore, [fromVueTsc, projectRequire]),
		ts: load<typeof ownTs>('typescript', ownTs, [projectRequire, fromVueTsc], (value) => typeof value?.sys?.readFile === 'function'),
		volar: load<typeof ownVolar>('@volar/language-core', ownVolar, [coreRequire, fromVueTsc, projectRequire])
	};
};

/*
 * Step one of vue-check: the TypeScript vue-tsc checks, as files.
 *
 * vue-tsc doesn't type-check Vue itself. @vue/language-core turns each .vue
 * file into TypeScript (with a map back to the source), and tsc checks that.
 * We run the same codegen, write the result beside the project in
 * `.vue-check/`, and let Bun's native checker do tsc's part (see cli.ts).
 */

export type SourceMap = {
	/** Where the generated file's offsets moved by our own edits (see below). */
	shift: number;
	source: string;
	mappings: {
		/** Volar's per-segment features: `verification` says which errors vue-tsc reports there. */
		data: unknown;
		generated: number[];
		generatedLengths: number[];
		lengths: number[];
		source: number[];
	}[];
	/** Volar's own filter, from the project's @volar/language-core. */
	shouldReport: (data: unknown, code: string) => boolean;
};

const REFERENCE = /^\/\/\/\s*<reference\s+(path|types)\s*=\s*["']([^"']+)["']/gm;
const EXTENSIONS: Record<string, '.js' | '.jsx' | '.ts' | '.tsx'> = {
	javascript: '.js',
	javascriptreact: '.jsx',
	typescript: '.ts',
	typescriptreact: '.tsx'
};

/* The .vue files the project's tsconfig includes, found the way vue-tsc finds
 * them: tsconfig's include/exclude with .vue as an extra extension, so
 * node_modules and excluded folders are never walked. */
/** Every root file of the project, Vue's files included (every extension
 * Vue is configured for: .vue, and e.g. .md for VitePress), in the order tsc
 * (and vue-tsc) lists them. */
export const parseProject = (ts: typeof ownTs, tsconfig: string, extensions: string[]) => {
	const config = ts.readConfigFile(tsconfig, ts.sys.readFile).config ?? {};

	return ts.parseJsonConfigFileContent(
		config,
		ts.sys,
		dirname(tsconfig),
		undefined,
		tsconfig,
		undefined,
		extensions.map((extension) => ({ extension, isMixedContent: true, scriptKind: ts.ScriptKind.Deferred }))
	);
};
export const projectFilesOf = (ts: typeof ownTs, tsconfig: string, extensions: string[]) =>
	parseProject(ts, tsconfig, extensions).fileNames;
/** The extensions vue-tsc checks: all Vue is configured for. */
export const vueExtensionsOf = (core: typeof ownCore, vueOptions: Parameters<typeof ownCore.getAllExtensions>[0]) =>
	typeof core.getAllExtensions === 'function' ? core.getAllExtensions(vueOptions) : ['.vue'];

/*
 * A known bug in @vue/language-core's codegen (codegen/template/element.js):
 * the component's props copy has its `//` comments turned into `/* … *\/`
 * with a regex that also matches `//` inside strings, so
 * `href="https://example.com"` becomes invalid TypeScript. vue-tsc hides the
 * syntax errors (that line is unmapped and under @ts-ignore) and type-checks
 * anyway; a checker that behaves like tsc stops at syntax errors and checks
 * nothing. The rewrite is undone exactly: each `/*REST *\/ ` becomes
 * `//REST` + the line break it replaced + two spaces, the same length, so
 * every mapping offset still holds and comments mean what they meant.
 * vuejs/language-tools#6360; see UPSTREAM_ISSUES.md for when to remove it.
 */
const PROPS_COPY = /(= __VLS_asFunctionalComponent\d?\([^\n]*\(\{\n\/\/ @ts-ignore\n)([^\n]*)/g;
export const repairPropsCopy = (text: string) =>
	text.replace(PROPS_COPY, (_match, head: string, line: string) => {
		let repaired = '';
		let rest = line;
		for (;;) {
			const open = rest.indexOf('/*');
			const close = open < 0 ? -1 : rest.indexOf(' */', open + 2);
			if (open < 0 || close < 0) break;
			repaired += `${rest.slice(0, open)}//${rest.slice(open + 2, close)}\n  `;
			rest = rest.slice(close + 3);
		}

		return head + repaired + rest;
	});

/*
 * Another codegen quirk vue-tsc hides: an event with no handler
 * (`@click=""`, with or without modifiers) becomes `onClick: (__VLS_ctx.)`,
 * which doesn't parse. `__VLS_ctx.` becomes `null!` padded to the same
 * length: an expression of type never, which reports nothing, as vue-tsc
 * shows nothing for an empty handler. vuejs/language-tools#6361; see
 * UPSTREAM_ISSUES.md for when to remove it.
 */
const EMPTY_HANDLER = /__VLS_ctx\.(?=\))/g;
export const repairEmptyHandlers = (text: string) => text.replace(EMPTY_HANDLER, 'null!     ');

/** Every known codegen repair, each keeping the text's length. */
export const repairGenerated = (text: string) => repairEmptyHandlers(repairPropsCopy(text));

/** A file the checker can't check as vue-tsc does. `fatal`: stop whatever
 * else was reported. */
export type UnparsedFile = { fatal?: true; file: string; line: number; message: string };

/**
 * The language a generated file is checked in. A plain <script> is
 * JavaScript, as vue-tsc's virtual file is. But some versions of Vue's
 * codegen (3.2) write typed scaffolding (declare, type aliases, casts) into
 * the JavaScript of a <script setup> without lang="ts": a checker that
 * behaves like tsc reports that as JavaScript grammar errors, which are
 * syntax errors to it, and checks nothing, where vue-tsc hides them. Without
 * checkJs, vue-tsc reports nothing in such a file and only uses its types, so
 * it's checked as TypeScript with its own errors off. With checkJs it would
 * be checked as JavaScript, which TypeScript can't stand in for.
 */
export const twinLanguage = (ts: typeof ownTs, extension: '.js' | '.jsx' | '.ts' | '.tsx', generated: string, checksJs: boolean) => {
	if ((extension !== '.js' && extension !== '.jsx') || !hasTypeScriptSyntax(ts, generated, extension === '.jsx'))
		return { extension, prefix: '', unsupported: false };

	return { extension: extension === '.jsx' ? '.tsx' : '.ts', prefix: '// @ts-nocheck\n', unsupported: checksJs };
};

/* Whether code has syntax only TypeScript allows: a type, a non-null
 * assertion, or a `declare`. (Parsed as TypeScript, where JSDoc types aren't
 * syntax nodes.) */
const hasTypeScriptSyntax = (ts: typeof ownTs, text: string, jsx: boolean) => {
	const sourceFile = ts.createSourceFile('check.ts', text, ts.ScriptTarget.Latest, false, jsx ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
	let found = false;
	const visit = (node: ownTs.Node) => {
		if (found) return;
		if (
			ts.isTypeNode(node) ||
			ts.isNonNullExpression(node) ||
			ts.isTypeAliasDeclaration(node) ||
			ts.isInterfaceDeclaration(node) ||
			(ts.canHaveModifiers(node) && (ts.getModifiers(node) ?? []).some((modifier) => modifier.kind === ts.SyntaxKind.DeclareKeyword))
		) {
			found = true;

			return;
		}
		ts.forEachChild(node, visit);
	};
	ts.forEachChild(sourceFile, visit);

	return found;
};

/** Where a file's generated twin lives: its absolute path, under outDir (see
 * codegen.ts: rootDirs ["/", outDir] joins every file to its twin). */
export const mirrorOf = (outDir: string, file: string) => join(outDir, relative(parse(file).root, file));

/**
 * Generates the TypeScript for the project's .vue files and writes it to
 * outDir. With `only` (absolute .vue paths), it writes nothing and returns
 * just those files' maps: codegen is deterministic, so regenerating a file
 * gives the same text and offsets the check saw.
 */
export const generate = (options: { only?: string[]; outDir: string; root: string; tsconfig: string }) => {
	const { core, ts, volar } = projectModules(options.root);
	const parsed = core.createParsedCommandLine(ts, ts.sys, options.tsconfig);
	/* As vue-tsc sets up its plugin: versions that keep Vue's global types in
	 * a file have vue-tsc write it (createGlobalTypesWriter); later ones don't. */
	const withWriter = core as unknown as {
		createGlobalTypesWriter?: (vueOptions: unknown, writeFile: typeof ts.sys.writeFile) => unknown;
	};
	if (typeof withWriter.createGlobalTypesWriter === 'function')
		(parsed.vueOptions as { globalTypesPath?: unknown }).globalTypesPath = withWriter.createGlobalTypesWriter(parsed.vueOptions, ts.sys.writeFile);
	const plugin = core.createVueLanguagePlugin(ts, parsed.options, parsed.vueOptions, (id: string) => id);
	const extensions = vueExtensionsOf(core, parsed.vueOptions);
	const writing = !options.only;
	if (writing) {
		rmSync(options.outDir, { force: true, recursive: true });
		mkdirSync(options.outDir, { recursive: true });
	}
	const maps: Record<string, SourceMap> = {};
	const unparsed: UnparsedFile[] = [];
	const references: { link: string; real: string }[] = [];
	const isVue = (file: string) => extensions.some((extension) => file.endsWith(extension));
	const projectFiles = options.only ? [] : projectFilesOf(ts, options.tsconfig, extensions);
	/*
	 * The Vue files to generate: the project's own, then every one the program
	 * reaches by import (a monorepo's workspace packages, say), as vue-tsc
	 * creates a virtual file for any Vue file its program resolves. Imports are
	 * read with TypeScript's preProcessFile (no full parse) from the generated
	 * code and from source files outside node_modules, and resolved as the
	 * project resolves them.
	 */
	const queue = options.only ? [...options.only] : projectFiles.filter(isVue);
	const queued = new Set(queue);
	const scanned = new Set<string>();
	const resolutionCache = ts.createModuleResolutionCache(dirname(options.tsconfig), (name) => name, parsed.options);
	const pathsBase = parsed.options.baseUrl ?? (parsed.options as { pathsBasePath?: string }).pathsBasePath ?? dirname(options.tsconfig);
	const aliasTargets = (name: string) =>
		Object.entries(parsed.options.paths ?? {}).flatMap(([pattern, targets]) => {
			const star = pattern.indexOf('*');
			const matched =
				star < 0
					? name === pattern
						? ''
						: null
					: name.startsWith(pattern.slice(0, star)) && name.endsWith(pattern.slice(star + 1))
						? name.slice(star, name.length - (pattern.length - star - 1))
						: null;

			return matched === null ? [] : targets.map((target) => resolve(pathsBase, target.replace('*', matched)));
		});
	/* A Vue file imported from a package (`pkg/components/Foo.vue`, through
	 * its folder or its `exports`): the import resolves inside node_modules,
	 * where rootDirs doesn't join the file to its generated twin and an
	 * `exports` target is looked up exactly as written. codegen.ts gives each
	 * such specifier a path alias to the twin. */
	const packageImports = new Map<string, string>();
	const vueTarget = (name: string, from: string) => {
		const candidates = name.startsWith('.') || isAbsolute(name) ? [resolve(dirname(from), name)] : aliasTargets(name);
		for (const candidate of candidates) if (existsSync(candidate)) return candidate;
		try {
			const target = createRequire(from).resolve(name);
			const known = packageImports.get(name);
			if (known === undefined) packageImports.set(name, target);
			else if (known !== target)
				unparsed.push({
					fatal: true,
					file: relative(options.root, from),
					line: 1,
					message: `its import of '${name}' resolves to a different file than the same import elsewhere in the project, which vue-check cannot check as vue-tsc does yet`
				});

			return target;
		} catch {
			return null;
		}
	};
	const scan = (file: string, text: string) => {
		for (const { fileName: name } of ts.preProcessFile(text, true, true).importedFiles) {
			if (isVue(name)) {
				const target = vueTarget(name, file);
				if (target && !queued.has(target)) {
					queued.add(target);
					queue.push(target);
				}
				continue;
			}
			const resolved = ts.resolveModuleName(name, file, parsed.options, ts.sys, resolutionCache).resolvedModule?.resolvedFileName;
			if (!resolved || resolved.endsWith('.d.ts') || resolved.includes('/node_modules/') || scanned.has(resolved)) continue;
			scanned.add(resolved);
			scan(resolved, readFileSync(resolved, 'utf8'));
		}
	};
	if (!options.only)
		for (const file of projectFiles)
			if (!isVue(file) && !file.endsWith('.d.ts') && !file.includes('/node_modules/') && !scanned.has(file)) {
				scanned.add(file);
				scan(file, readFileSync(file, 'utf8'));
			}
	for (let file = queue.shift(); file !== undefined; file = queue.shift()) {
		const text = readFileSync(file, 'utf8');
		const languageId = plugin.getLanguageId?.(file) ?? 'vue';
		const code = plugin.createVirtualCode?.(file, languageId, ts.ScriptSnapshot.fromString(text), {
			getAssociatedScript: () => undefined
		});
		if (!code) continue;
		for (const embedded of volar.forEachEmbeddedCode(code)) {
			// script_raw is the untouched <script>; script_ts/tsx is what vue-tsc checks.
			if (!embedded.id.startsWith('script_') || embedded.id === 'script_raw') continue;
			/* The generated code's own language: a plain <script> is JavaScript, as
			 * vue-tsc's virtual file is (so checkJs decides, not us). */
			const original = embedded.snapshot.getText(0, embedded.snapshot.getLength());
			const twin = twinLanguage(ts, EXTENSIONS[embedded.languageId] ?? '.ts', original, Boolean(parsed.options.checkJs) || /@ts-check\b/.test(text));
			if (twin.unsupported)
				unparsed.push({
					fatal: true,
					file: relative(options.root, file),
					line: 1,
					message:
						'its JavaScript <script setup> is type-checked as JavaScript (checkJs or @ts-check), and Vue’s codegen writes TypeScript into it, which vue-check cannot check as vue-tsc does yet'
				});
			const { extension, prefix } = twin;
			const target = mirrorOf(options.outDir, file) + extension;
			if (!options.only) scan(file, original);
			const written = prefix + repairGenerated(original);
			/* A relative reference directive (Vue's global types, a user's
			 * `/// <reference path="./globals.d.ts" />`) resolves from the
			 * generated file's folder. The text stays as written; codegen.ts
			 * links each target into the mirror, as it links node_modules. */
			for (const [, kind, reference = ''] of written.matchAll(REFERENCE))
				if (reference.startsWith('./') || reference.startsWith('../') || (kind === 'path' && !isAbsolute(reference)))
					references.push({ link: resolve(dirname(target), reference), real: resolve(dirname(file), reference) });
			if (writing) {
				mkdirSync(dirname(target), { recursive: true });
				writeFileSync(target, written);
			}
			/* A checker that behaves like tsc stops at any syntax error and checks
			 * nothing. A syntax error in the user's own code (it maps back to the
			 * .vue file, as vue-tsc would report it) is theirs to see, so the check
			 * runs and reports it, as vue-tsc does. One that maps nowhere is an
			 * artifact of codegen: vue-tsc hides it, but the checker would stop on
			 * it, so the project couldn't be checked. Found here, with TypeScript's
			 * parser, for every file at once. */
			const scriptKind = { '.js': ts.ScriptKind.JS, '.jsx': ts.ScriptKind.JSX, '.ts': ts.ScriptKind.TS, '.tsx': ts.ScriptKind.TSX }[extension];
			const parsedFile = ts.createSourceFile(target, written, ts.ScriptTarget.Latest, false, scriptKind);
			const syntax =
				(parsedFile as unknown as { parseDiagnostics?: { code: number; length?: number; messageText: unknown; start?: number }[] })
					.parseDiagnostics ?? [];
			const shift = written.length - original.length;
			const sourceMap = syntax.length > 0 && writing ? new volar.SourceMap(embedded.mappings) : null;
			const unmapped = syntax.find((diagnostic) => {
				if (!sourceMap) return false;
				const start = (diagnostic.start ?? 0) - shift;
				const end = start + (diagnostic.length ?? 0);
				for (const _range of sourceMap.toSourceRange(start, end, true, (data) =>
					volar.shouldReportDiagnostics(data, 'undefined', String(diagnostic.code))
				))
					return false;

				return true;
			});
			if (unmapped)
				unparsed.push({
					file: relative(options.root, file),
					line: parsedFile.getLineAndCharacterOfPosition(unmapped.start ?? 0).line + 1,
					message: typeof unmapped.messageText === 'string' ? unmapped.messageText : 'syntax error'
				});
			maps[relative(options.root, target)] = {
				mappings: embedded.mappings.map((mapping) => ({
					data: mapping.data,
					generated: mapping.generatedOffsets,
					generatedLengths: mapping.generatedLengths ?? mapping.lengths,
					lengths: mapping.lengths,
					source: mapping.sourceOffsets
				})),
				shift,
				shouldReport: (data, code) =>
					volar.shouldReportDiagnostics(data as Parameters<typeof volar.shouldReportDiagnostics>[0], 'undefined', code),
				source: relative(options.root, file)
			};
		}
	}
	return { core, extensions, maps, packageImports, references, ts, unparsed };
};
