#!/usr/bin/env bun
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import ts from 'typescript';
import { generate } from './generate';
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
const { core, maps, ts: projectTs, unparsed } = generate({ outDir, root, tsconfig });

/* The project's own options, plus the generated tree as one directory with it. */
// Read the way tsc reads it: comments and trailing commas allowed.
const projectConfig = (ts.readConfigFile(tsconfig, ts.sys.readFile).config ?? {}) as { exclude?: string[]; include?: string[] };
const include = (projectConfig.include ?? ['**/*']).map((pattern) => join('..', pattern));
// tsc's default excludes are relative to the config's folder, which is now .vue-check/.
const exclude = (projectConfig.exclude ?? ['node_modules', 'bower_components', 'jspm_packages']).map((pattern) => join('..', pattern));
writeFileSync(
	join(outDir, 'tsconfig.json'),
	JSON.stringify({
		compilerOptions: { allowArbitraryExtensions: true, rootDirs: [root, outDir] },
		extends: tsconfig,
		exclude,
		include: [...include, './**/*.ts', './**/*.tsx']
	})
);
/* Every diagnostic code the project's TypeScript defines, for the fixed rules
 * (an internal table: without it, every error is decided by Volar itself). */
const diagnostics = (projectTs as unknown as { Diagnostics?: Record<string, { code: number }> }).Diagnostics ?? {};
const diagnosticCodes = [...new Set(Object.values(diagnostics).map((message) => message.code))].sort((a, b) => a - b);
const codeFeatures = (core as unknown as { codeFeatures?: Record<string, { verification?: unknown }> }).codeFeatures ?? {};
writeFileSync(join(outDir, 'codegen.json'), JSON.stringify({ ...serialize(maps, codeFeatures, diagnosticCodes), unparsed }));
