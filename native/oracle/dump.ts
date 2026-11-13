#!/usr/bin/env bun
import { appendFileSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join, relative } from 'node:path';

/*
 * The oracle for the native template parser: what the project's own
 * @vue/compiler-dom and @vue/language-core make of every .vue file, as JSON
 * the native side reproduces (native/src/dump.rs) and compare.ts checks value
 * for value. Runs in a container (run.sh): it loads the project's packages.
 *
 *   bun native/oracle/dump.ts <project-dir> <out-dir>
 *
 * Writes <out-dir>/files.jsonl (each file's text, the native side's input)
 * and <out-dir>/oracle.jsonl.
 */
const [root, out] = process.argv.slice(2);
if (!root || !out) throw new Error('usage: bun native/oracle/dump.ts <project-dir> <out-dir>');

/* The packages vue-tsc would use: @vue/language-core through the project's
 * vue-tsc, and the @vue/compiler-dom that language-core itself resolves. */
const projectRequire = createRequire(join(root, 'package.json'));
const fromVueTsc = createRequire(projectRequire.resolve('vue-tsc/package.json'));
const corePackage = fromVueTsc.resolve('@vue/language-core/package.json');
const coreRequire = createRequire(corePackage);
const CompilerDOM = coreRequire('@vue/compiler-dom');
/* The parser decodes entities with the `entities` package its
 * @vue/compiler-core resolves, whose behavior has changed between versions. */
const entitiesVersion = () => {
	try {
		const compilerCore = createRequire(coreRequire.resolve('@vue/compiler-dom')).resolve('@vue/compiler-core');
		const decode = createRequire(compilerCore).resolve('entities/decode');

		return JSON.parse(readFileSync(join(decode.slice(0, decode.lastIndexOf('/entities/') + 10), 'package.json'), 'utf8')).version as string;
	} catch {
		return 'unknown';
	}
};
const versions = {
	compilerDom: coreRequire('@vue/compiler-dom/package.json').version as string,
	entities: entitiesVersion(),
	languageCore: coreRequire(corePackage).version as string
};
/* The block splitting is language-core's own, and only compared for the
 * version the native codegen ports. */
const ported = versions.languageCore === '3.3.14';
const fileVue = ported
	? coreRequire(join(dirname(corePackage), 'lib/plugins/file-vue.js')).default({ vueCompilerOptions: { extensions: ['.vue'] } })
	: null;

type Loc = { end?: { line: number; offset: number }; source?: string; start: { line: number; offset: number } };
type AnyNode = Record<string, any>;

const loc = (value: Loc | undefined) =>
	value ? [value.start.offset, value.start.line, value.end?.offset ?? 0, value.end?.line ?? 0, value.source ?? ''] : null;
const expression = (node: AnyNode | undefined) =>
	node ? { constType: node.constType, content: node.content, isStatic: node.isStatic, loc: loc(node.loc), type: 4 } : null;
const text = (node: AnyNode | undefined) => (node ? { content: node.content, loc: loc(node.loc), type: node.type } : null);
const prop = (node: AnyNode) =>
	node.type === 6
		? { loc: loc(node.loc), name: node.name, nameLoc: loc(node.nameLoc), type: 6, value: text(node.value) }
		: {
				arg: expression(node.arg),
				exp: expression(node.exp),
				forParseResult: node.forParseResult
					? {
							index: expression(node.forParseResult.index),
							key: expression(node.forParseResult.key),
							source: expression(node.forParseResult.source),
							value: expression(node.forParseResult.value)
						}
					: null,
				loc: loc(node.loc),
				modifiers: node.modifiers.map(expression),
				name: node.name,
				rawName: node.rawName,
				type: 7
			};
const node = (value: AnyNode): unknown => {
	if (value.type === 1)
		return {
			children: value.children.map(node),
			innerLoc: loc(value.innerLoc),
			isSelfClosing: Boolean(value.isSelfClosing),
			loc: loc(value.loc),
			ns: value.ns,
			props: value.props.map(prop),
			tag: value.tag,
			tagType: value.tagType,
			type: 1
		};
	if (value.type === 5) return { content: expression(value.content), loc: loc(value.loc), type: 5 };

	return text(value);
};
const errorOf = (error: AnyNode) => [error.code, error.loc?.start.offset ?? null, error.loc?.start.line ?? null];
const tree = (ast: AnyNode, errors: AnyNode[]) => ({ children: ast.children.map(node), errors: errors.map(errorOf), loc: loc(ast.loc) });

const attr = (value: unknown) => (value === undefined ? null : value === true ? true : { offset: (value as AnyNode).offset, text: (value as AnyNode).text });
const block = (value: AnyNode | null | undefined) =>
	value
		? {
				attrs: Object.entries(value.attrs),
				content: value.content,
				end: value.loc.end.offset,
				generic: attr(value.__generic),
				lang: value.lang ?? null,
				module: attr(value.__module),
				scoped: Boolean(value.scoped),
				setup: value.setup ?? null,
				src: attr(value.__src),
				start: value.loc.start.offset,
				startLine: value.loc.start.line,
				type: value.type
			}
		: null;

/** The file's tree in single-file-component mode, as parseSfc.ts asks for it. */
const sfcTree = (source: string) => {
	const errors: AnyNode[] = [];
	const ast = CompilerDOM.parse(source, {
		comments: true,
		isNativeTag: () => true,
		isPreTag: () => true,
		onError: (error: AnyNode) => errors.push(error),
		parseMode: 'sfc'
	});

	return tree(ast, errors);
};
/** A template's tree, parsed as template/compile.ts parses it. */
const templateTree = (source: string) => {
	const errors: AnyNode[] = [];
	const ast = CompilerDOM.parse(source, {
		comments: true,
		expressionPlugins: ['typescript'],
		onError: (error: AnyNode) => errors.push(error),
		onWarn: () => {}
	});

	return tree(ast, errors);
};

const SKIPPED = new Set(['node_modules', '.git', '.vue-check', 'dist']);
const vueFiles = (folder: string): string[] =>
	readdirSync(folder, { withFileTypes: true })
		.sort((a, b) => (a.name < b.name ? -1 : 1))
		.flatMap((entry) => {
			if (SKIPPED.has(entry.name)) return [];
			const path = join(folder, entry.name);
			if (entry.isDirectory()) return vueFiles(path);

			return entry.isFile() && entry.name.endsWith('.vue') ? [path] : [];
		});

/* The parser keeps its v-pre and <pre> state between parses and never clears
 * it: a file that ends inside v-pre, or that makes the parser throw halfway,
 * changes how every later file parses. That file is recorded, and the rest
 * runs in a new process. */
const leaked = () => CompilerDOM.parse('{{ a }}').children[0]?.type !== 5;
const from = Number(process.argv.find((value) => value.startsWith('--from='))?.split('=')[1] ?? 0);
const paths = vueFiles(root);
const files: string[] = [];
const oracle: string[] = [];
const flush = () => {
	const write = from === 0 ? writeFileSync : appendFileSync;
	write(join(out, 'files.jsonl'), files.map((line) => `${line}\n`).join(''));
	write(join(out, 'oracle.jsonl'), oracle.map((line) => `${line}\n`).join(''));
};
for (const [index, path] of paths.entries()) {
	if (index < from) continue;
	const file = relative(root, path);
	const source = readFileSync(path, 'utf8');
	files.push(JSON.stringify({ file, text: source }));
	const record: Record<string, unknown> = { file, versions };
	let poisoned = false;
	try {
		const ast = sfcTree(source);
		poisoned = leaked();
		if (fileVue && !poisoned) {
			const { descriptor } = fileVue.parseSFC2(path, 'vue', source);
			record.sfc = {
				ast,
				comments: descriptor.comments,
				customBlocks: descriptor.customBlocks.map(block),
				script: block(descriptor.script),
				scriptSetup: block(descriptor.scriptSetup),
				styles: descriptor.styles.map(block),
				template: block(descriptor.template)
			};
			const template = descriptor.template;
			record.template = template && (template.lang ?? 'html').match(/^(html|md)$/) ? templateTree(template.content) : null;
			poisoned = leaked();
		} else record.sfc = { ast };
	} catch (error) {
		record.threw = String(error);
		poisoned = true;
	}
	record.poisonsLaterParses = poisoned;
	oracle.push(JSON.stringify(record));
	if (poisoned) {
		flush();
		const rest = Bun.spawnSync([process.execPath, import.meta.path, root, out, `--from=${index + 1}`], { stderr: 'inherit', stdout: 'inherit' });
		process.exit(rest.exitCode);
	}
}
flush();
console.log(`${paths.length} files, @vue/language-core ${versions.languageCore}, @vue/compiler-dom ${versions.compilerDom}, entities ${versions.entities}`);
