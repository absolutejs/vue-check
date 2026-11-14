#!/usr/bin/env bun
import { writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';

/*
 * The oracle for template expressions alone: generated expressions and
 * statements of every syntax kind, each given to the project's own
 * generateInterpolation with a generated context (which names are bindings,
 * constants, refs, in scope). Real templates exercise little of this.
 * Deterministic. Runs in a container (run-expressions.sh).
 *
 *   bun native/oracle/dump-expressions.ts <project-dir> <out-dir> [--count=20000] [--seed=1]
 *
 * Writes <out-dir>/oracle.jsonl in the shape dump.ts records interpolations,
 * for `vue-check-codegen interpolate` and compare-interpolations.ts.
 */
const [root, out] = process.argv.slice(2);
if (!root || !out) throw new Error('usage: bun native/oracle/dump-expressions.ts <project-dir> <out-dir>');
const option = (name: string, fallback: string) =>
	process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=')[1] ?? fallback;
const count = Number(option('count', '20000'));

const projectRequire = createRequire(join(root, 'package.json'));
const corePackage = createRequire(projectRequire.resolve('vue-tsc/package.json')).resolve('@vue/language-core/package.json');
const coreRequire = createRequire(corePackage);
const lib = (path: string) => coreRequire(join(dirname(corePackage), 'lib', path));
const version = coreRequire(corePackage).version as string;
if (version !== '3.3.14') throw new Error(`@vue/language-core ${version} isn't the ported version`);
const ts = projectRequire('typescript');
const { generateInterpolation } = lib('codegen/template/interpolation.js');
const { createTemplateCodegenContext } = lib('codegen/template/context.js');
const { codeFeatures } = lib('codegen/codeFeatures.js');
const vueCompilerOptions = lib('compilerOptions.js').getDefaultCompilerOptions();

let state = Number(option('seed', '1')) >>> 0;
const random = () => {
	state = (state + 0x6d2b79f5) >>> 0;
	let value = state;
	value = Math.imul(value ^ (value >>> 15), value | 1);
	value ^= value + Math.imul(value ^ (value >>> 7), value | 61);

	return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
};
const below = (limit: number) => Math.floor(random() * limit);
const pick = <T>(items: readonly T[]): T => items[below(items.length)] as T;
const chance = (probability: number) => random() < probability;
const some = <T>(items: readonly T[], probability: number) => items.filter(() => chance(probability));
const list = (make: () => string, most: number, separator = ', ') => Array.from({ length: below(most + 1) }, make).join(separator);

const NAMES = ['a', 'b', 'c', 'foo', 'bar', 'props', 'emit', 'item', 'index', '$event', 'Foo', 'x1', 'é', '_p'];
const OTHER = ['Math', 'console', 'undefined', 'this', 'true', 'null', 'require', '__VLS_x', 'Number', 'window', 'arguments'];
const id = () => (chance(0.85) ? pick(NAMES) : pick(OTHER));
const name = () => pick(NAMES);
const literal = () => pick(['1', "'s'", '`t`', '0n', '/re/g', 'true', 'null', '1.5e3', '"é😀"']);
const type = (depth = 0): string =>
	pick([
		() => 'string',
		() => `typeof ${name()}`,
		() => `typeof ${name()}.${name()}.${name()}`,
		() => `{ [${name()}]: string; ${name()}: typeof ${name()} }`,
		() => `Array<typeof ${name()}>`,
		() => `(p: typeof ${name()}) => void`,
		() => `{ m(): typeof ${name()}; [${name()}.${name()}](): void }`,
		() => 'T',
		() => `typeof this`,
		() => `${name()}.${name()}<string>`,
		() => (depth < 2 ? `${type(depth + 1)} | ${type(depth + 1)}` : 'number'),
		() => `typeof import('x')`,
		() => `[x: typeof ${name()}, ...rest: string[]]`,
		() => `keyof typeof ${name()}`
	])();
const pattern = (depth = 0): string =>
	pick([
		() => name(),
		() => name(),
		() => `{ ${name()}, ${name()}: ${depth < 2 ? pattern(depth + 1) : name()}${chance(0.5) ? ` = ${expression(3)}` : ''}, ...${name()} }`,
		() => `[${name()}, , ${depth < 2 ? pattern(depth + 1) : name()} = ${expression(3)}, ...${name()}]`,
		() => `{ [${expression(3)}]: ${name()} }`,
		() => `{ ${name()} = ${expression(3)} }`
	])();
const parameter = () => `${pattern()}${chance(0.2) ? `: ${type()}` : ''}${chance(0.3) ? ` = ${expression(3)}` : ''}`;
const parameters = () => [list(parameter, 2), chance(0.15) ? `...${name()}` : ''].filter(Boolean).join(', ');
const target = (depth = 0): string =>
	pick([
		() => id(),
		() => `${id()}.${name()}`,
		() => `${id()}[${expression(3)}]`,
		() => `[${id()}, , ${depth < 2 ? target(depth + 1) : id()} = ${expression(3)}, ...${id()}]`,
		() => `({ ${name()}, ${name()}: ${depth < 2 ? target(depth + 1) : id()}, ${name()} = ${expression(3)}, [${expression(3)}]: ${id()}, ...${id()} })`,
		() => `(${id()})`,
		() => `${id()}!`,
		() => `(${id()} as any)`
	])();
const body = () => pick([() => expression(2), () => `{ ${statements(2)} }`, () => `{ 'use strict'; ${statements(2)} }`, () => `({ ${name()} })`, () => '{}'])();
function expression(depth = 0): string {
	if (depth > 3) return chance(0.7) ? id() : literal();
	const next = () => expression(depth + 1);

	return pick([
		id, id, id, literal,
		() => `${next()}.${name()}`,
		() => `${next()}?.${name()}`,
		() => `${next()}[${next()}]`,
		() => `${next()}?.[${next()}]`,
		() => `${next()}(${list(next, 2)})`,
		() => `${next()}?.(${next()}, ...${next()})`,
		() => `${id()}<${type()}>(${next()})`,
		() => `new ${id()}(${list(next, 2)})`,
		() => `new ${id()}`,
		() => `new (${next()})(${next()})`,
		() => `new ${id()}.${name()}<${type()}>()`,
		() => `${id()}\`a\${${next()}}b\``,
		() => `\`\${${next()}} and \${${next()}}\``,
		() => `(${next()})`,
		() => `${next()}!`,
		() => `<${type()}>${id()}`,
		() => `${next()} as ${type()}`,
		() => `${next()} satisfies ${type()}`,
		() => `${next()}, ${next()}`,
		() => `${next()} ${pick(['||', '&&', '??'])} ${next()}`,
		() => `${target()} ${pick(['=', '+=', '||=', '&&=', '??=', '**='])} ${next()}`,
		() => `${next()} ${pick(['===', '!==', '==', '!=', 'instanceof', 'in', '+', '<', '%', '**', '|', '>>>'])} ${next()}`,
		() => `${next()} ? ${next()} : ${next()}`,
		() => `${pick(['!', '-', '+', '~', 'typeof ', 'void ', 'delete ', 'await '])}${next()}`,
		() => `${pick(['++', '--'])}${id()}`,
		() => `${id()}.${name()}${pick(['++', '--'])}`,
		() => `${chance(0.3) ? 'async ' : ''}(${parameters()})${chance(0.2) ? `: ${type()}` : ''} => ${body()}`,
		() => `${name()} => ${body()}`,
		() => `function ${chance(0.5) ? name() : ''}(${parameters()}) { ${statements(depth + 1)} }`,
		() => `function* (this: ${type()}, ${parameter()}) { yield ${next()} }`,
		() => `class ${chance(0.5) ? name() : ''} ${chance(0.4) ? `extends ${next()} ` : ''}{ ${classBody(depth + 1)} }`,
		() => `{ ${list(() => property(depth + 1), 3)} }`,
		() => `[${list(() => (chance(0.2) ? `...${next()}` : chance(0.1) ? '' : next()), 3)}]`,
		() => `import(${next()})`,
		() => `import.meta.${name()}`,
		() => `${id()}<${type()}>`,
		() => `#${name()} in ${next()}`
	])();
}
function property(depth: number): string {
	const next = () => expression(depth + 1);

	return pick([
		() => name(),
		() => `${name()}: ${next()}`,
		() => `'${name()}': ${next()}`,
		() => `[${next()}]: ${next()}`,
		() => `...${next()}`,
		() => `${name()}(${parameters()}) { ${statements(depth + 1)} }`,
		() => `get [${next()}]() { return ${next()} }`,
		() => `set ${name()}(${pattern()}) { ${next()} }`,
		() => `async *[${next()}](${parameter()}) {}`
	])();
}
function classBody(depth: number): string {
	const next = () => expression(depth + 1);

	return list(
		() =>
			pick([
				() => `${name()} = ${next()};`,
				() => `[${next()}]: ${type()} = ${next()};`,
				() => `static { ${statements(depth + 1)} }`,
				() => `${name()}(${parameters()}) { ${statements(depth + 1)} }`,
				() => `constructor(${parameters()}) { ${statements(depth + 1)} }`,
				() => `get [${next()}]() { return ${next()} }`,
				() => `static accessor ${name()} = ${next()};`,
				() => `declare ${name()}: ${type()};`
			])(),
		3,
		' '
	);
}
function statement(depth: number): string {
	if (depth > 3) return `${expression(depth)};`;
	const next = () => statement(depth + 1);
	const value = () => expression(depth + 1);

	return pick([
		() => `${value()};`, () => `${value()};`, () => `${value()};`,
		() => `if (${value()}) ${next()} else ${next()}`,
		() => `if (${value()}) { ${statements(depth + 1)} }`,
		() => `for (${pick(['const', 'let', 'var'])} ${pattern()} of ${value()}) { ${statements(depth + 1)} }`,
		() => `for (${target()} of ${value()}) ${next()}`,
		() => `for (const ${name()} in ${value()}) ${next()}`,
		() => `for (let ${name()} = ${value()}, ${pattern()} = ${value()}; ${value()}; ${value()}) ${next()}`,
		() => `for (${value()}; ; ) { break; }`,
		() => `while (${value()}) ${next()}`,
		() => `do ${next()} while (${value()});`,
		() => `switch (${value()}) { case ${value()}: ${statements(depth + 1)} break; default: function ${name()}() {} ${next()} }`,
		() => `try { ${statements(depth + 1)} } catch (${pattern()}) { ${statements(depth + 1)} } finally { ${next()} }`,
		() => `try { ${next()} } catch { ${next()} }`,
		() => `${pick(['const', 'let', 'var'])} ${pattern()}${chance(0.3) ? `: ${type()}` : ''} = ${value()};`,
		() => `function ${name()}(${parameters()})${chance(0.2) ? `: ${type()}` : ''} { ${statements(depth + 1)} }`,
		() => `return ${value()};`,
		() => `return;`,
		() => `${name()}: ${next()}`,
		() => `{ ${statements(depth + 1)} }`,
		() => `class ${name()} { ${classBody(depth + 1)} }`,
		() => `enum ${name()} { A = ${value()}, B }`,
		() => `throw ${value()};`,
		() => `type ${name()} = ${type()};`,
		() => `interface ${name()} { a: ${type()} }`,
		() => `;`
	])();
}
function statements(depth: number): string {
	return list(() => statement(depth), 3, ' ');
}

const FEATURES = [codeFeatures.all, codeFeatures.all, codeFeatures.withoutCompletion, codeFeatures.navigation, codeFeatures.doNotReportTs6133];
const WRAPS = [['(', ')'], ['(', ')'], ['(', ');\n'], ['', ''], ['[__VLS_tryAsConstant(', ')]'], ['() => {', '}']] as const;
const calls: unknown[] = [];
const records: string[] = [];
for (let index = 0; index < count; index += 1) {
	const [prefix, suffix] = pick(WRAPS);
	const isStatements = prefix === '' || prefix === '() => {';
	let code = isStatements ? statements(0) : expression(0);
	if (chance(0.03)) code = pick(['', ' ', name(), `${name()} `, '\n']);
	const setupBindings = new Set(some(NAMES, 0.5));
	const setupConsts = new Set(some([...setupBindings], 0.3));
	const setupRefs = new Set(some(NAMES, 0.1));
	const dotValueBindings = new Set(some([...setupBindings], 0.4));
	const reassertBindings = chance(0.7) ? new Set(some([...dotValueBindings], 0.7)) : undefined;
	const ctx = createTemplateCodegenContext();
	for (let depth = below(3); depth > 0; depth -= 1) ctx.scope().declare(...some(NAMES, 0.15));
	for (const dollar of some(['$slots', '$attrs', '$refs', '$el', '$event'], 0.2)) ctx.dollarVars.add(dollar);
	const options = { dotValueBindings, reassertBindings, scriptLang: pick(['ts', 'ts', 'js']), setupBindings, setupConsts, setupRefs, typescript: ts, vueCompilerOptions };
	const features = pick(FEATURES);
	const inNarrowing = chance(0.3);
	const start = below(500);
	const block = { content: code, name: pick(['template', 'template', 'style_0']) };
	const call = {
		code,
		dollarVars: [...ctx.dollarVars],
		dotValueBindings: [...dotValueBindings],
		inNarrowing,
		lib: vueCompilerOptions.lib,
		prefix,
		reassertBindings: [...(reassertBindings ?? [])],
		scopes: ctx.scopes.map((scope: Set<string>) => [...scope]),
		scriptLang: options.scriptLang,
		setupBindings: [...setupBindings],
		setupConsts: [...setupConsts],
		setupRefs: [...setupRefs],
		source: block.name,
		start,
		suffix,
		/* Whether TypeScript's parser reports an error in it: from an error, it
		 * recovers its own way, which the native side doesn't reproduce. */
		typescriptErrors: (ts.createSourceFile('/dummy.ts', prefix + code + suffix, 99) as any).parseDiagnostics.length,
		verification: features.verification ? (features === codeFeatures.doNotReportTs6133 ? 'ts6133' : 1) : 0
	};
	const tokens = new Map<symbol, number>();
	/* The original throws on some expressions; vue-tsc then has no code for the file. */
	try {
		const codes = [...generateInterpolation(options, ctx, block, features, code, start, prefix, suffix, inNarrowing)].map((generated: any) => {
			if (typeof generated === 'string') return generated;
			const [text, source, offset, data] = generated;
			const token = data?.__combineToken;
			if (token !== undefined && !tokens.has(token)) tokens.set(token, tokens.size);
			const verification = !data?.verification ? 0 : data.verification === codeFeatures.doNotReportTs6133.verification ? 'ts6133' : 1;

			return [text, source ?? null, offset, verification, token === undefined ? null : tokens.get(token)];
		});
		calls.push({ ...call, accesses: [...ctx.accessLog], codes });
	} catch (error) {
		calls.push({ ...call, threw: String(error) });
	}
	if (calls.length === 500 || index === count - 1) {
		records.push(JSON.stringify({ file: `expressions-${records.length}`, interpolations: calls.splice(0) }));
	}
}
writeFileSync(join(out, 'oracle.jsonl'), records.join('\n') + '\n');
console.log(`${count} expressions, @vue/language-core ${version}, typescript ${ts.version}`);
