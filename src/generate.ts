import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
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
	const fromVueTsc = (() => {
		try {
			return createRequire(projectRequire.resolve('vue-tsc/package.json'));
		} catch {
			return null;
		}
	})();
	const load = <T>(name: string, fallback: T, requirers: (NodeJS.Require | null)[]): T => {
		for (const requirer of requirers) {
			if (!requirer) continue;
			try {
				return requirer(name) as T;
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
				return createRequire(requirer.resolve('@vue/language-core/package.json'));
			} catch {
				// Not here.
			}
		}

		return null;
	})();

	return {
		core: load<typeof ownCore>('@vue/language-core', ownCore, [fromVueTsc, projectRequire]),
		ts: load<typeof ownTs>('typescript', ownTs, [projectRequire, fromVueTsc]),
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

const REFERENCE = /\/\/\/ <reference types="([^"]+)" \/>/g;

/* The .vue files the project's tsconfig includes, found the way vue-tsc finds
 * them: tsconfig's include/exclude with .vue as an extra extension, so
 * node_modules and excluded folders are never walked. */
const vueFilesOf = (ts: typeof ownTs, tsconfig: string) => {
	const config = ts.readConfigFile(tsconfig, ts.sys.readFile).config ?? {};
	const parsed = ts.parseJsonConfigFileContent(config, ts.sys, dirname(tsconfig), undefined, tsconfig, undefined, [
		{ extension: '.vue', isMixedContent: true, scriptKind: ts.ScriptKind.Deferred }
	]);

	return parsed.fileNames.filter((file) => file.endsWith('.vue'));
};

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
 * Remove once fixed upstream (vuejs/language-tools).
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
 * Another codegen quirk vue-tsc hides: an event with a modifier and no
 * handler (`@click.stop=""`) becomes `onClick: (__VLS_ctx.)`, which doesn't
 * parse. `__VLS_ctx.` becomes `null!` padded to the same length: an
 * expression of type never, which reports nothing, as vue-tsc shows nothing
 * for an empty handler.
 */
const EMPTY_HANDLER = /__VLS_ctx\.(?=\))/g;
export const repairEmptyHandlers = (text: string) => text.replace(EMPTY_HANDLER, 'null!     ');

/** Every known codegen repair, each keeping the text's length. */
export const repairGenerated = (text: string) => repairEmptyHandlers(repairPropsCopy(text));

export type UnparsedFile = { file: string; line: number; message: string };

/**
 * Generates the TypeScript for the project's .vue files and writes it to
 * outDir. With `only` (absolute .vue paths), it writes nothing and returns
 * just those files' maps: codegen is deterministic, so regenerating a file
 * gives the same text and offsets the check saw.
 */
export const generate = (options: { only?: string[]; outDir: string; root: string; tsconfig: string }) => {
	const { core, ts, volar } = projectModules(options.root);
	const parsed = core.createParsedCommandLine(ts, ts.sys, options.tsconfig);
	const plugin = core.createVueLanguagePlugin(ts, parsed.options, parsed.vueOptions, (id: string) => id);
	const writing = !options.only;
	if (writing) {
		rmSync(options.outDir, { force: true, recursive: true });
		mkdirSync(options.outDir, { recursive: true });
	}
	const maps: Record<string, SourceMap> = {};
	const unparsed: UnparsedFile[] = [];
	const files = options.only ?? vueFilesOf(ts, options.tsconfig);
	for (const file of files) {
		const text = readFileSync(file, 'utf8');
		const code = plugin.createVirtualCode?.(file, 'vue', ts.ScriptSnapshot.fromString(text), {
			getAssociatedScript: () => undefined
		});
		if (!code) continue;
		for (const embedded of volar.forEachEmbeddedCode(code)) {
			// script_raw is the untouched <script>; script_ts/tsx is what vue-tsc checks.
			if (!embedded.id.startsWith('script_') || embedded.id === 'script_raw') continue;
			const extension = embedded.languageId === 'typescriptreact' ? '.tsx' : '.ts';
			const target = join(options.outDir, relative(options.root, file) + extension);
			const original = embedded.snapshot.getText(0, embedded.snapshot.getLength());
			// Helper types are referenced relative to the .vue file's folder; the copy lives elsewhere.
			const written = repairGenerated(original).replace(
				REFERENCE,
				(_match, reference: string) => `/// <reference path="${resolve(dirname(file), reference)}" />`
			);
			if (writing) {
				mkdirSync(dirname(target), { recursive: true });
				writeFileSync(target, written);
			}
			/* A checker that behaves like tsc stops at any syntax error and checks
			 * nothing, so every generated file has to parse. Found here, with
			 * TypeScript's parser (no type-checking), for every file at once. */
			const parsedFile = ts.createSourceFile(target, written, ts.ScriptTarget.Latest, false, extension === '.tsx' ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
			const syntax = (parsedFile as unknown as { parseDiagnostics?: { messageText: unknown; start?: number }[] }).parseDiagnostics ?? [];
			for (const diagnostic of writing ? syntax.slice(0, 1) : [])
				unparsed.push({
					file: relative(options.root, file),
					line: parsedFile.getLineAndCharacterOfPosition(diagnostic.start ?? 0).line + 1,
					message: typeof diagnostic.messageText === 'string' ? diagnostic.messageText : 'syntax error'
				});
			maps[relative(options.root, target)] = {
				mappings: embedded.mappings.map((mapping) => ({
					data: mapping.data,
					generated: mapping.generatedOffsets,
					generatedLengths: mapping.generatedLengths ?? mapping.lengths,
					lengths: mapping.lengths,
					source: mapping.sourceOffsets
				})),
				shift: written.length - original.length,
				shouldReport: (data, code) =>
					volar.shouldReportDiagnostics(data as Parameters<typeof volar.shouldReportDiagnostics>[0], 'undefined', code),
				source: relative(options.root, file)
			};
		}
	}
	return { core, maps, ts, unparsed };
};
