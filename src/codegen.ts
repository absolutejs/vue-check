#!/usr/bin/env bun
import { writeFileSync } from 'node:fs';
import { isAbsolute, join, relative, resolve } from 'node:path';
import { generate, parseProject } from './generate';
import { serialize } from './serialize';

/*
 * Step one of vue-check, in its own process: Vue's codegen for every .vue
 * file, written to .vue-check/, plus the tsconfig the check runs on. Then it
 * exits, so the memory codegen used (TypeScript, Vue's compiler, every
 * parsed file) is returned before the check starts.
 *
 *   bun src/codegen.ts <root> <tsconfig>
 *
 * Writes .vue-check/codegen.json: the source maps as data (see serialize.ts)
 * and any generated file that doesn't parse.
 */

const [root = process.cwd(), tsconfig = join(root, 'tsconfig.json')] = process.argv.slice(2);
const outDir = join(root, '.vue-check');
const { core, extensions, maps, ts: projectTs, unparsed } = generate({ outDir, root, tsconfig });

/* The project's own options, plus the generated tree as one directory with
 * it. The root files are listed explicitly, in the order tsc lists the
 * project's (each Vue file replaced by its generated TypeScript, in place):
 * the order files enter a program decides the order types are created, and
 * so how TypeScript prints them in messages. */
const generatedFor = new Map(Object.entries(maps).map(([file, map]) => [join(root, map.source), join(root, file)]));
const project = parseProject(projectTs, tsconfig, extensions);
const files = project.fileNames.flatMap((file) => {
	if (!extensions.some((extension) => file.endsWith(extension))) return [file];
	const generatedFile = generatedFor.get(file);

	return generatedFile ? [generatedFile] : [];
});
/* rootDirs joins the trees for relative imports only. A path alias
 * (`@/components/Foo.vue`) gets the same join: each target also points into
 * .vue-check/, as absolute paths so the base they resolve from can't differ. */
const { options } = project;
const pathsBase = options.baseUrl ?? (options as { pathsBasePath?: string }).pathsBasePath ?? root;
const paths = options.paths
	? Object.fromEntries(
			Object.entries(options.paths).map(([alias, targets]) => [
				alias,
				targets.flatMap((target) => {
					const absolute = isAbsolute(target) ? target : resolve(pathsBase, target);
					const inside = relative(root, absolute);

					return inside.startsWith('..') || isAbsolute(inside) ? [absolute] : [absolute, join(outDir, inside)];
				})
			])
		)
	: undefined;
writeFileSync(
	join(outDir, 'tsconfig.json'),
	JSON.stringify({
		compilerOptions: { allowArbitraryExtensions: true, rootDirs: [root, outDir], ...(paths ? { paths } : {}) },
		extends: tsconfig,
		files: files.map((file) => relative(outDir, file)),
		include: []
	})
);
/* Every diagnostic code the project's TypeScript defines, for the fixed rules
 * (an internal table: without it, every error is decided by Volar itself). */
const diagnostics = (projectTs as unknown as { Diagnostics?: Record<string, { code: number }> }).Diagnostics ?? {};
const diagnosticCodes = [...new Set(Object.values(diagnostics).map((message) => message.code))].sort((a, b) => a - b);
const codeFeatures = (core as unknown as { codeFeatures?: Record<string, { verification?: unknown }> }).codeFeatures ?? {};
writeFileSync(join(outDir, 'codegen.json'), JSON.stringify({ ...serialize(maps, codeFeatures, diagnosticCodes), unparsed }));
