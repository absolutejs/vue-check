#!/usr/bin/env bun
import { existsSync, mkdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, parse, relative, resolve } from 'node:path';
import { generate, mirrorOf, parseProject } from './generate';
import { serialize } from './serialize';

/*
 * Step one of vue-check, in its own process: Vue's codegen for every .vue
 * file, written to .vue-check/, plus the tsconfig the check runs on. Then it
 * exits, so the memory codegen used (TypeScript, Vue's compiler, every
 * parsed file) is returned before the check starts.
 *
 *   bun src/codegen.ts <root> <tsconfig> [outDir]
 *
 * Writes .vue-check/codegen.json: the source maps as data (see serialize.ts)
 * and any generated file that doesn't parse.
 */

const [root = process.cwd(), tsconfig = join(root, 'tsconfig.json'), outDir = join(root, '.vue-check')] = process.argv.slice(2);
const { core, extensions, maps, references, ts: projectTs, unparsed } = generate({ outDir, root, tsconfig });

/* A generated twin resolves imports from its own folder: bare imports walk up
 * node_modules folders, and the nearest package.json sets a file's module
 * format. So each folder above a generated file's source that has either gets
 * a link to it at the same place in the mirror, and a twin resolves every
 * import to exactly the files its source would. */
const linked = new Set<string>();
for (const map of Object.values(maps)) {
	for (let folder = dirname(join(root, map.source)); !linked.has(folder); folder = dirname(folder)) {
		linked.add(folder);
		for (const entry of ['node_modules', 'package.json']) {
			const real = join(folder, entry);
			if (!existsSync(real)) continue;
			const mirrored = join(mirrorOf(outDir, folder), entry);
			mkdirSync(dirname(mirrored), { recursive: true });
			if (!existsSync(mirrored)) symlinkSync(real, mirrored);
		}
		if (folder === parse(folder).root) break;
	}
}

/* Each relative reference directive's target, at the place the generated
 * file looks for it (after the folder links, so a target under node_modules
 * is already reached through them). */
for (const { link, real } of references)
	for (const suffix of ['', '.d.ts', '.ts', '/index.d.ts']) {
		if (!existsSync(real + suffix) || existsSync(link + suffix)) continue;
		mkdirSync(dirname(link + suffix), { recursive: true });
		symlinkSync(real + suffix, link + suffix);
		break;
	}

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
 * (`@/components/Foo.vue`) gets the same join: each target also points at its
 * twin in .vue-check/, as absolute paths so the base they resolve from can't
 * differ. */
const { options } = project;
const pathsBase = options.baseUrl ?? (options as { pathsBasePath?: string }).pathsBasePath ?? root;
const joined = (target: string) => {
	const absolute = isAbsolute(target) ? target : resolve(pathsBase, target);

	return [absolute, mirrorOf(outDir, absolute)];
};
const aliases: Record<string, string[]> = Object.fromEntries(
	Object.entries(options.paths ?? {}).map(([alias, targets]) => [alias, targets.flatMap(joined)])
);
/* TypeScript 7 (bun check) has removed baseUrl. Its documented replacement
 * keeps the meaning: paths are absolute already, and bare imports resolve
 * under baseUrl through a catch-all alias, tried after every other one. */
if (options.baseUrl) aliases['*'] = [...(aliases['*'] ?? []), ...joined('*')];
const paths = Object.keys(aliases).length > 0 ? aliases : undefined;
/* A relative `types` entry resolves from the directory of the config being
 * compiled, which is now .vue-check/: keep it where vue-tsc finds it. */
const types = options.types?.map((entry) => (entry.startsWith('./') || entry.startsWith('../') ? resolve(dirname(tsconfig), entry) : entry));
writeFileSync(
	join(outDir, 'tsconfig.json'),
	JSON.stringify({
		compilerOptions: {
			allowArbitraryExtensions: true,
			// Every file and its generated twin, wherever it is (see mirrorOf).
			rootDirs: [parse(root).root, outDir],
			...(paths ? { paths } : {}),
			...(types ? { types } : {}),
			...(options.baseUrl ? { baseUrl: null } : {})
		},
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
