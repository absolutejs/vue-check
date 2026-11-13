#!/usr/bin/env bun
import { existsSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';

/*
 * Vue's own conformance suite: every case in vuejs/language-tools'
 * test-workspace/tsc (the projects vue-tsc's typecheck test runs), through the
 * same four-way differential as bench/run.ts. Most cases assert their errors
 * with @ts-expect-error / @vue-expect-error, so a clean result proves both no
 * false positives and every expected error; the rest leave known errors that
 * must come out identically.
 *
 *   bun bench/conformance.ts <workspace> [--jobs=4]
 *
 * <workspace> holds the cases in tsc/ and the installed vue-tsc (see
 * bench/run-in-docker.sh: run it inside the container).
 */

const workspace = process.argv[2];
if (!workspace) throw new Error('usage: bun bench/conformance.ts <workspace>');
const jobs = Number(process.argv.find((value) => value.startsWith('--jobs='))?.split('=')[1] ?? '4');
const tool = join(import.meta.dir, '..');
const workspaceRequire = createRequire(join(workspace, 'package.json'));
const vueTsc = workspaceRequire.resolve('vue-tsc/bin/vue-tsc.js');
/* The tsc a TypeScript package ships, from its own `bin` (an aliased package
 * such as @typescript/typescript6 names it tsc6). */
const tscOf = (packageJson: string) => {
	const { bin } = JSON.parse(readFileSync(packageJson, 'utf8')) as { bin?: Record<string, string> | string };
	const entry = typeof bin === 'string' ? bin : (bin?.tsc ?? Object.values(bin ?? {})[0] ?? 'bin/tsc');

	return join(dirname(packageJson), entry);
};
const projectTsc = tscOf(createRequire(vueTsc).resolve('typescript/package.json'));
const tsc7 = join(tool, 'node_modules/@typescript/typescript-linux-x64/lib/tsc');
const casesDir = join(workspace, 'tsc');
const cases = readdirSync(casesDir)
	.filter((name) => existsSync(join(casesDir, name, 'tsconfig.json')))
	.sort();

const errorsOf = (stdout: string) =>
	stdout
		.split('\n')
		.map((line) => line.trim())
		.filter((line) => /\(\d+,\d+\): error TS\d+|^error TS\d+/.test(line))
		.sort();
const minus = (a: string[], b: string[]) => a.filter((line) => !b.includes(line));

const run = async (command: string[], cwd: string, env: Record<string, string> = {}) => {
	const child = Bun.spawn(command, { cwd, env: { ...process.env, ...env }, stderr: 'pipe', stdout: 'pipe' });
	const [stdout, stderr, exitCode] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);

	return { errors: errorsOf(stdout), exitCode, stderr: stderr.trim().split('\n').slice(-3).join('\n') };
};

type CaseResult = {
	bunMatchesTsc7: boolean;
	endToEnd: boolean;
	invalid: string[];
	name: string;
	notes?: Record<string, string[] | string>;
	sameAsVueTsc: boolean;
	vueTscErrors: number;
};

const checkCase = async (name: string): Promise<CaseResult> => {
	const cwd = join(casesDir, name);
	const vueCheck = (tsc?: string) =>
		run([process.execPath, join(tool, 'src/cli.ts'), '-p', 'tsconfig.json'], cwd, tsc ? { VUE_CHECK_TSC: tsc } : {});
	const oracle = await run(['node', vueTsc, '--noEmit', '-p', 'tsconfig.json', '--pretty', 'false'], cwd);
	const withProjectTs = await vueCheck(projectTsc);
	const withTsc7 = await vueCheck(tsc7);
	const ours = await vueCheck();
	const invalid = [
		...(oracle.exitCode > 2 ? [`vue-tsc exited ${oracle.exitCode}`] : []),
		...[
			['vue-check (project TS)', withProjectTs],
			['vue-check (tsc 7)', withTsc7],
			['vue-check', ours]
		].flatMap(([label, result]) => {
			const { exitCode, stderr } = result as typeof ours;

			return exitCode === 0 || exitCode === 2 ? [] : [`${label as string} exited ${exitCode}: ${stderr}`];
		})
	];
	const handling = { onlyVueCheck: minus(withProjectTs.errors, oracle.errors), onlyVueTsc: minus(oracle.errors, withProjectTs.errors) };
	const checker = { onlyBunCheck: minus(ours.errors, withTsc7.errors), onlyTsc7: minus(withTsc7.errors, ours.errors) };
	const added = minus(withTsc7.errors, withProjectTs.errors);
	const removed = minus(withProjectTs.errors, withTsc7.errors);
	const expected = [...minus(oracle.errors, removed), ...added].sort();
	const sameAsVueTsc = invalid.length === 0 && handling.onlyVueCheck.length === 0 && handling.onlyVueTsc.length === 0;
	const bunMatchesTsc7 = invalid.length === 0 && checker.onlyBunCheck.length === 0 && checker.onlyTsc7.length === 0;
	const endToEnd = invalid.length === 0 && minus(expected, ours.errors).length === 0 && minus(ours.errors, expected).length === 0;
	const notes =
		sameAsVueTsc && bunMatchesTsc7 && endToEnd
			? added.length + removed.length > 0
				? { typeScript7Added: added, typeScript7Removed: removed }
				: undefined
			: { ...handling, ...checker, invalid, typeScript7Added: added, typeScript7Removed: removed };

	return { bunMatchesTsc7, endToEnd, invalid, name, notes, sameAsVueTsc, vueTscErrors: oracle.errors.length };
};

const results: CaseResult[] = [];
const queue = [...cases];
await Promise.all(
	Array.from({ length: jobs }, async () => {
		for (let name = queue.shift(); name !== undefined; name = queue.shift()) {
			const result = await checkCase(name);
			results.push(result);
			const ok = result.sameAsVueTsc && result.bunMatchesTsc7 && result.endToEnd;
			console.error(`${ok ? 'ok  ' : 'DIFF'} ${name}${ok ? '' : ` ${JSON.stringify(result.notes).slice(0, 300)}`}`);
		}
	})
);
results.sort((a, b) => a.name.localeCompare(b.name));
const passing = results.filter((result) => result.sameAsVueTsc && result.bunMatchesTsc7 && result.endToEnd);
const report = {
	cases: results.length,
	identical: passing.length,
	vueTscErrors: results.reduce((sum, result) => sum + result.vueTscErrors, 0),
	sameAsVueTsc: results.filter((result) => result.sameAsVueTsc).length,
	bunMatchesTsc7: results.filter((result) => result.bunMatchesTsc7).length,
	typeScript7Changes: results.filter((result) => result.notes && 'typeScript7Added' in result.notes).length,
	failing: results.filter((result) => !passing.includes(result)),
	withTypeScript7Changes: passing.filter((result) => result.notes)
};
writeFileSync(join(tool, 'bench/results/conformance.json'), JSON.stringify(report, null, '\t'));
console.log(JSON.stringify({ ...report, failing: report.failing.map((result) => result.name), withTypeScript7Changes: undefined }, null, 2));
