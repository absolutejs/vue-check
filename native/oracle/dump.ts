#!/usr/bin/env bun
import { appendFileSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, join, relative } from 'node:path';

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
	/* What the transforms create: joined text, v-if chains and v-for. */
	if (value.type === 8)
		return { children: value.children.map((child: AnyNode | string) => (typeof child === 'string' ? child : node(child))), loc: loc(value.loc), type: 8 };
	if (value.type === 9) return { branches: value.branches.map(node), loc: loc(value.loc), type: 9 };
	if (value.type === 10) return { children: value.children.map(node), condition: expression(value.condition), loc: loc(value.loc), type: 10 };
	if (value.type === 11)
		return {
			children: value.children.map(node),
			loc: loc(value.loc),
			parseResult: {
				index: expression(value.parseResult.index),
				key: expression(value.parseResult.key),
				source: expression(value.parseResult.source),
				value: expression(value.parseResult.value)
			},
			type: 11
		};

	return text(value);
};
const errorOf = (error: AnyNode) => [error.code, error.loc?.start.offset ?? null, error.loc?.start.line ?? null];
const tree = (ast: AnyNode, errors: AnyNode[]) => ({
	children: ast.children.map(node),
	components: ast.components,
	errors: errors.map(errorOf),
	loc: loc(ast.loc)
});

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
/** A template's tree after language-core's transforms, compiled the way its
 * vue-template-html plugin compiles an HTML template. */
const templateHtml = ported ? coreRequire(join(dirname(corePackage), 'lib/plugins/vue-template-html.js')).default({}) : null;
const templateTree = (lang: string, source: string) => {
	const errors: AnyNode[] = [];
	const result = templateHtml.compileSFCTemplate(lang, source, {
		expressionPlugins: ['typescript'],
		onError: (error: AnyNode) => errors.push(error),
		onWarn: () => {}
	});

	return result ? tree(result.ast, errors) : null;
};

/* What language-core reads from each <script> block's TypeScript tree, with
 * its default compiler options. */
const analysis = ported
	? {
			options: coreRequire(join(dirname(corePackage), 'lib/compilerOptions.js')).getDefaultCompilerOptions(),
			script: coreRequire(join(dirname(corePackage), 'lib/parsers/scriptRanges.js')).parseScriptRanges,
			setup: coreRequire(join(dirname(corePackage), 'lib/parsers/scriptSetupRanges.js')).parseScriptSetupRanges,
			ts: projectRequire('typescript')
		}
	: null;
const VALID_LANGS = new Set(['js', 'jsx', 'ts', 'tsx']);
const range = (value: AnyNode | undefined) => (value ? [value.start, value.end] : null);
const callRange = (value: AnyNode) => ({ arg: range(value.arg), callExp: range(value.callExp), exp: range(value.exp), typeArg: range(value.typeArg) });
const macroCall = (value: AnyNode | undefined) => (value ? { ...callRange(value), name: value.name ?? null, statement: range(value.statement) } : null);
const scriptRanges = (value: AnyNode | null | undefined) => {
	if (!analysis || !value || !VALID_LANGS.has(value.lang ?? 'js')) return null;
	const ast = analysis.ts.createSourceFile(`.${value.lang ?? 'js'}`, value.content, 99);
	const { bindings, exportDefault } = analysis.script(analysis.ts, ast, analysis.options);
	const options = exportDefault?.options;

	return {
		bindings: [...bindings],
		exportDefault: exportDefault
			? {
					end: exportDefault.end,
					expression: range(exportDefault.expression),
					isObjectLiteral: exportDefault.isObjectLiteral,
					options: options
						? {
								args: range(options.args),
								components: range(options.components),
								directives: range(options.directives),
								expression: range(options.expression),
								inheritAttrs: options.inheritAttrs ?? null,
								isObjectLiteral: options.isObjectLiteral,
								name: range(options.name)
							}
						: null,
					start: exportDefault.start
				}
			: null
	};
};
const scriptSetupRanges = (value: AnyNode | null | undefined) => {
	if (!analysis || !value || !VALID_LANGS.has(value.lang ?? 'js')) return null;
	const { ts } = analysis;
	const ast = ts.createSourceFile(`.${value.lang ?? 'js'}`, value.content, 99);
	const found = analysis.setup(ts, ast, analysis.options);
	const nodeRange = (node: AnyNode | undefined) => (node ? [ts.getTokenPosOfNode(node, ast), node.end] : null);

	return {
		bindings: [...found.bindings],
		defineEmits: found.defineEmits ? { ...macroCall(found.defineEmits), hasUnionTypeArg: Boolean(found.defineEmits.hasUnionTypeArg) } : null,
		defineExpose: found.defineExpose ? callRange(found.defineExpose) : null,
		defineModel: found.defineModel.map((model: AnyNode) => ({
			arg: range(model.arg),
			comments: range(model.comments),
			defaultPropsArg: range(model.defaultPropsArg),
			defaultValue: range(model.defaultValue),
			localName: range(model.localName),
			modifierType: range(model.modifierType),
			name: range(model.name),
			required: Boolean(model.required),
			runtimeType: range(model.runtimeType),
			type: range(model.type)
		})),
		defineOptions: found.defineOptions ? { inheritAttrs: found.defineOptions.inheritAttrs ?? null, name: found.defineOptions.name ?? null } : null,
		defineProps: found.defineProps
			? {
					...macroCall(found.defineProps),
					destructured: found.defineProps.destructured ? [...found.defineProps.destructured].map(([name, initializer]) => [name, nodeRange(initializer)]) : null,
					destructuredRest: found.defineProps.destructuredRest ?? null
				}
			: null,
		defineSlots: macroCall(found.defineSlots),
		importSectionEndOffset: found.importSectionEndOffset,
		leadingCommentEndOffset: found.leadingCommentEndOffset,
		useAttrs: found.useAttrs.map(callRange),
		useCssModule: found.useCssModule.map(callRange),
		useSlots: found.useSlots.map(callRange),
		useTemplateRef: found.useTemplateRef.map((call: AnyNode) => ({ ...callRange(call), name: call.name ?? null })),
		withDefaults: found.withDefaults ? callRange(found.withDefaults) : null
	};
};

/* The codegen itself, run as vue-tsc runs it, with generateInterpolation
 * wrapped: each call's inputs and what it generated are recorded, so the
 * native side can be given the same call. */
const core = ported ? coreRequire(corePackage.replace(/package\.json$/, 'index.js')) : null;
const codeFeatures: Record<string, AnyNode> = core?.codeFeatures ?? {};
/** Which errors a piece of code reports: 0 none, 1 all, a fixed rule's name, or 'stateful'. */
const verificationOf = (features: AnyNode | undefined) => {
	const verification = features?.verification;
	if (!verification) return 0;
	if (typeof verification !== 'object' || !verification.shouldReport) return 1;
	if (verification === codeFeatures.doNotReportTs2353AndTs2561?.verification) return 'ts2353';
	if (verification === codeFeatures.doNotReportTs6133?.verification) return 'ts6133';

	return 'stateful';
};
/** Pieces of code as data: text, or [text, source, offset, verification, combine group]. */
const codesOf = (codes: unknown[]) => {
	const tokens = new Map<symbol, number>();

	return codes.map((code) => {
		if (typeof code === 'string') return code;
		const [text, source, offset, features] = code as [string, string | undefined, number, AnyNode];
		const token = features?.__combineToken as symbol | undefined;
		if (token !== undefined && !tokens.has(token)) tokens.set(token, tokens.size);

		return [text, source ?? null, offset, verificationOf(features), token === undefined ? null : tokens.get(token)];
	});
};
let interpolations: unknown[] = [];
if (ported) {
	const module = coreRequire(join(dirname(corePackage), 'lib/codegen/template/interpolation.js'));
	const original = module.generateInterpolation;
	module.generateInterpolation = function* (options: AnyNode, ctx: AnyNode, block: AnyNode, features: AnyNode, code: string, start: number, prefix = '', suffix = '', inNarrowing = false) {
		const call = {
			code,
			dollarVars: [...ctx.dollarVars],
			dotValueBindings: [...options.dotValueBindings],
			inNarrowing,
			lib: options.vueCompilerOptions.lib,
			prefix,
			reassertBindings: [...(options.reassertBindings ?? [])],
			scopes: ctx.scopes.map((scope: Set<string>) => [...scope]),
			scriptLang: options.scriptLang,
			setupBindings: [...options.setupBindings],
			setupConsts: [...options.setupConsts],
			setupRefs: [...options.setupRefs],
			source: block.name,
			start,
			suffix,
			verification: verificationOf(features)
		};
		const accessMark = ctx.accessLog.length;
		const codes: unknown[] = [];
		for (const generated of original(options, ctx, block, features, code, start, prefix, suffix, inNarrowing)) {
			/* Copied now: the template codegen replaces a piece's features once it has it. */
			codes.push(typeof generated === 'string' ? generated : [...generated]);
			yield generated;
		}
		interpolations.push({ ...call, accesses: ctx.accessLog.slice(accessMark), codes: codesOf(codes) });
	};
}
/* generateTemplate wrapped the same way: its options and what it returned. */
let templates: unknown[] = [];
if (ported) {
	const module = coreRequire(join(dirname(corePackage), 'lib/codegen/template/index.js'));
	const original = module.generateTemplate;
	module.generateTemplate = (options: AnyNode) => {
		const result = original(options);
		const compiler = options.vueCompilerOptions;
		templates.push({
			codes: codesOf(result.codes),
			compiler: {
				checkRequiredFallthroughAttributes: compiler.checkRequiredFallthroughAttributes,
				checkUnknownComponents: compiler.checkUnknownComponents,
				checkUnknownDirectives: compiler.checkUnknownDirectives,
				checkUnknownEvents: compiler.checkUnknownEvents,
				checkUnknownProps: compiler.checkUnknownProps,
				dataAttributes: compiler.dataAttributes,
				fallthroughAttributes: compiler.fallthroughAttributes,
				fallthroughComponentNames: compiler.fallthroughComponentNames,
				htmlAttributes: compiler.htmlAttributes,
				inferComponentDollarEl: compiler.inferComponentDollarEl,
				inferComponentDollarRefs: compiler.inferComponentDollarRefs,
				inferTemplateDollarAttrs: compiler.inferTemplateDollarAttrs,
				inferTemplateDollarEl: compiler.inferTemplateDollarEl,
				inferTemplateDollarRefs: compiler.inferTemplateDollarRefs,
				inferTemplateDollarSlots: compiler.inferTemplateDollarSlots,
				lib: compiler.lib,
				strictVModel: compiler.strictVModel
			},
			componentName: options.componentName,
			contextAccesses: [...result.contextAccesses.keys()],
			dotValueAccesses: [...result.dotValueAccesses],
			dotValueBindings: [...options.dotValueBindings],
			generatedTypes: [...result.generatedTypes],
			hasAst: Boolean(options.template.ast),
			hasDefineSlots: Boolean(options.hasDefineSlots),
			importedComponents: [...options.importedComponents],
			inheritAttrs: options.inheritAttrs,
			isVapor: Boolean(options.isVapor),
			lang: options.template.lang,
			propsAssignName: options.propsAssignName ?? null,
			reassertBindings: [...options.reassertBindings],
			scriptLang: options.scriptLang,
			setupBindings: [...options.setupBindings],
			setupConsts: [...options.setupConsts],
			setupRefs: [...options.setupRefs],
			slotsAssignName: options.slotsAssignName ?? null,
			template: options.template.content
		});

		return result;
	};
}
/* The Vue version the code is generated for: vue-tsc reads it from the
 * project's installed vue; TARGET sets it here (99, the default, is "latest"). */
const target = process.env.TARGET ? Number(process.env.TARGET) : undefined;
/* The options of a project that sets none, as vue-tsc resolves them. */
const vueOptions = core && analysis ? new core.CompilerOptionsResolver(analysis.ts).build(core.getDefaultCompilerOptions(target)) : null;
const languagePlugin = (() => {
	if (!core || !analysis) return null;
	const { ts } = analysis;
	if (typeof core.createGlobalTypesWriter === 'function') vueOptions.globalTypesPath = core.createGlobalTypesWriter(vueOptions, () => {});

	return core.createVueLanguagePlugin(ts, {}, vueOptions, (id: string) => id);
})();
/** The folder of language-core's type declarations as a file's reference
 * lines name it: an input of the codegen, which the JavaScript side works out. */
const typesPathOf = (path: string) => {
	const typesRoot: string | undefined = vueOptions?.typesRoot;
	if (!typesRoot) return null;
	if (!isAbsolute(typesRoot)) return typesRoot;
	const relativePath = relative(dirname(path), typesRoot);

	return relativePath !== typesRoot && !relativePath.startsWith('./') && !relativePath.startsWith('../') ? `./${relativePath}` : relativePath;
};
const volar = ported ? createRequire(corePackage)('@volar/language-core') : null;
/** The TypeScript vue-tsc checks for a file, and its mappings. */
const generated = (path: string, source: string) => {
	if (!languagePlugin || !analysis || !volar) return null;
	const code = languagePlugin.createVirtualCode(path, 'vue', analysis.ts.ScriptSnapshot.fromString(source), { getAssociatedScript: () => undefined });
	if (!code) return null;
	for (const embedded of volar.forEachEmbeddedCode(code)) {
		if (!embedded.id.startsWith('script_') || embedded.id === 'script_raw') continue;

		return {
			id: embedded.id,
			languageId: embedded.languageId,
			mappings: embedded.mappings.map((mapping: AnyNode) => ({
				generated: mapping.generatedOffsets,
				generatedLengths: mapping.generatedLengths ?? null,
				lengths: mapping.lengths,
				source: mapping.sourceOffsets,
				verification: verificationOf(mapping.data)
			})),
			text: embedded.snapshot.getText(0, embedded.snapshot.getLength())
		};
	}

	return null;
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
	files.push(JSON.stringify({ file, target: vueOptions?.target ?? null, text: source, typesPath: typesPathOf(path) }));
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
			record.script = scriptRanges(descriptor.script);
			record.scriptSetup = scriptSetupRanges(descriptor.scriptSetup);
			const template = descriptor.template;
			record.template = template ? templateTree(template.lang ?? 'html', template.content) : null;
			poisoned = leaked();
			if (!poisoned) {
				interpolations = [];
				templates = [];
				record.generated = generated(path, source);
				record.interpolations = interpolations;
				record.templates = templates;
				poisoned = leaked();
			}
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
