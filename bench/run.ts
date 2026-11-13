#!/usr/bin/env bun
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { availableParallelism, cpus, tmpdir, totalmem } from 'node:os';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';

/*
 * vue-check against vue-tsc on a real project: the same errors, and what each
 * costs. Run inside an isolated container (bench/run-in-docker.sh), never
 * against a project's own scripts.
 *
 *   bun bench/run.ts <project-dir> [--runs=5] [--tsconfig=tsconfig.json] [--vue-tsc-args="--skipLibCheck"]
 *
 * Memory is the peak RSS of the whole process tree (sampled from /proc every
 * 20 ms), so all of vue-check's processes are counted together; each step's
 * own peak is recorded too (by which process it is). Time is wall-clock.
 * bun check uses every core by default; it's also measured on one thread so
 * parallelism can't flatter the comparison.
 */

const option = (name: string, fallback: string) =>
	process.argv.find((value) => value.startsWith(`--${name}=`))?.split('=').slice(1).join('=') ?? fallback;

const project = process.argv[2];
if (!project) throw new Error('usage: bun bench/run.ts <project-dir>');
const runs = Number(option('runs', '5'));
const tsconfig = option('tsconfig', 'tsconfig.json');
const vueTscArgs = option('vue-tsc-args', '').split(' ').filter(Boolean);
const tool = join(import.meta.dir, '..');

const rssOf = (pid: number) => {
	try {
		const status = readFileSync(`/proc/${pid}/status`, 'utf8');

		return Number(/VmRSS:\s+(\d+)/.exec(status)?.[1] ?? 0) * 1024;
	} catch {
		return 0;
	}
};
const childrenOf = (pid: number): number[] => {
	try {
		return readdirSync(`/proc/${pid}/task`).flatMap((task) =>
			readFileSync(`/proc/${pid}/task/${task}/children`, 'utf8').split(' ').filter(Boolean).map(Number)
		);
	} catch {
		return [];
	}
};
const treePids = (pid: number): number[] => [pid, ...childrenOf(pid).flatMap(treePids)];
/* Which vue-check step a process is: its own codegen process, the checker
 * (bun check or tsc), or the CLI itself (mapping errors back). */
const phaseOf = (pid: number): 'check' | 'codegen' | 'map' => {
	try {
		const argv = readFileSync(`/proc/${pid}/cmdline`, 'utf8').split('\0');
		if (argv.some((value) => value.endsWith('/codegen.ts'))) return 'codegen';
		if (argv[1] === 'check' || argv.some((value) => value.endsWith('/tsc'))) return 'check';
	} catch {
		// Exited between samples.
	}

	return 'map';
};

type Phases = {
	checkMs: number;
	codegenMs: number;
	mapMs: number;
	regeneratedFiles: number;
	vueFiles: number;
};
type PhaseRss = Record<'check' | 'codegen' | 'map', number>;
type Measurement = { exitCode: number; peakRssBytes: number; phaseRss: PhaseRss; phases?: Phases; stdout: string; wallMs: number };

const phasesFile = join(tmpdir(), `vue-check-phases-${process.pid}.json`);
const measure = async (command: string[]): Promise<Measurement> => {
	rmSync(phasesFile, { force: true });
	const started = performance.now();
	const child = Bun.spawn(command, {
		cwd: project,
		env: { ...process.env, VUE_CHECK_PHASES: phasesFile },
		stderr: 'pipe',
		stdout: 'pipe'
	});
	let peak = 0;
	const phaseRss: PhaseRss = { check: 0, codegen: 0, map: 0 };
	const sampler = setInterval(() => {
		const now: PhaseRss = { check: 0, codegen: 0, map: 0 };
		for (const pid of treePids(child.pid)) now[phaseOf(pid)] += rssOf(pid);
		peak = Math.max(peak, now.check + now.codegen + now.map);
		for (const phase of ['check', 'codegen', 'map'] as const) phaseRss[phase] = Math.max(phaseRss[phase], now[phase]);
	}, 20);
	const [stdout, exitCode] = await Promise.all([new Response(child.stdout).text(), child.exited]);
	clearInterval(sampler);
	const wallMs = performance.now() - started;
	const phases = existsSync(phasesFile) ? (JSON.parse(readFileSync(phasesFile, 'utf8')) as Phases) : undefined;

	return { exitCode, peakRssBytes: peak, phaseRss, phases, stdout, wallMs };
};

/** Error lines, normalized: the same problems must be reported by both. */
const errorsOf = (stdout: string) =>
	stdout
		.split('\n')
		.filter((line) => /\(\d+,\d+\): error TS\d+/.test(line))
		.map((line) => line.trim())
		.sort();

const tools = {
	'vue-tsc': ['node', join(project, 'node_modules/vue-tsc/bin/vue-tsc.js'), '--noEmit', '-p', tsconfig, ...vueTscArgs],
	'vue-check': [process.execPath, join(tool, 'src/cli.ts'), '-p', tsconfig, ...vueTscArgs],
	'vue-check (1 thread)': [process.execPath, join(tool, 'src/cli.ts'), '-p', tsconfig, '--threads=1', ...vueTscArgs]
} as const;

const results: Record<string, Measurement[]> = {};
for (const [name, command] of Object.entries(tools)) {
	results[name] = [];
	for (let run = 0; run < runs; run++) {
		const result = await measure([...command]);
		results[name].push(result);
		console.error(`${name} run ${run + 1}/${runs}: ${(result.wallMs / 1000).toFixed(2)} s, ${(result.peakRssBytes / 2 ** 20).toFixed(0)} MB`);
	}
}

const median = (values: number[]) => {
	const sorted = [...values].sort((a, b) => a - b);

	return sorted[Math.floor(sorted.length / 2)] ?? 0;
};
const summary = Object.fromEntries(
	Object.entries(results).map(([name, list]) => [
		name,
		{
			errors: errorsOf(list[0]?.stdout ?? '').length,
			medianPeakRssMB: Math.round(median(list.map((item) => item.peakRssBytes)) / 2 ** 20),
			medianWallS: Number((median(list.map((item) => item.wallMs)) / 1000).toFixed(2)),
			...(list[0]?.phases
				? {
						phases: {
							checkMB: Math.round(median(list.map((item) => item.phaseRss.check)) / 2 ** 20),
							checkS: Number((median(list.map((item) => item.phases?.checkMs ?? 0)) / 1000).toFixed(2)),
							codegenMB: Math.round(median(list.map((item) => item.phaseRss.codegen)) / 2 ** 20),
							codegenS: Number((median(list.map((item) => item.phases?.codegenMs ?? 0)) / 1000).toFixed(2)),
							mapMB: Math.round(median(list.map((item) => item.phaseRss.map)) / 2 ** 20),
							regeneratedFiles: list[0].phases.regeneratedFiles,
							mapS: Number((median(list.map((item) => item.phases?.mapMs ?? 0)) / 1000).toFixed(2)),
							vueFiles: list[0].phases.vueFiles
						}
					}
				: {})
		}
	])
);
const oracle = errorsOf(results['vue-tsc']?.[0]?.stdout ?? '');
const ours = errorsOf(results['vue-check']?.[0]?.stdout ?? '');
const minus = (a: string[], b: string[]) => a.filter((line) => !b.includes(line));

/*
 * Four ways, so each question is answered on its own, and every answer is an
 * exact equality:
 *
 *   vue-check on the project's own TypeScript vs vue-tsc: is the Vue
 *     handling right? Same TypeScript, so they must agree exactly.
 *   vue-check on bun check vs vue-check on tsc 7.0.2 (the oracle Bun tests
 *     against), on exactly the same generated program: is Bun's checker
 *     right? They must agree exactly.
 *   vue-check on the project's TypeScript vs on tsc 7, again on the same
 *     program: what TypeScript 7 itself changes. By definition, not chosen.
 *   And end to end: vue-check (on bun check) must equal vue-tsc with exactly
 *     those TypeScript 7 changes applied.
 */
const withChecker = async (tsc: string) => {
	rmSync(phasesFile, { force: true });
	const child = Bun.spawn([process.execPath, join(tool, 'src/cli.ts'), '-p', tsconfig, ...vueTscArgs], {
		cwd: project,
		env: { ...process.env, VUE_CHECK_PHASES: phasesFile, VUE_CHECK_TSC: tsc },
		stderr: 'pipe',
		stdout: 'pipe'
	});
	const [stdout, exitCode] = await Promise.all([new Response(child.stdout).text(), child.exited]);

	return { errors: errorsOf(stdout), exitCode, finished: existsSync(phasesFile) && [0, 1, 2].includes(exitCode) };
};
/* The TypeScript vue-tsc runs: resolved from the project's vue-tsc. */
const projectTsc = (() => {
	const fromVueTsc = createRequire(createRequire(join(project, 'package.json')).resolve('vue-tsc/package.json'));

	return join(dirname(fromVueTsc.resolve('typescript/package.json')), 'bin/tsc');
})();
const tsc7 = join(tool, 'node_modules/@typescript/typescript-linux-x64/lib/tsc');
const withProjectTs = await withChecker(projectTsc);
const withTsc7 = await withChecker(tsc7);
const vueHandling = { onlyVueCheck: minus(withProjectTs.errors, oracle), onlyVueTsc: minus(oracle, withProjectTs.errors) };
const bunVsTsc7 = { onlyBunCheck: minus(ours, withTsc7.errors), onlyTsc7: minus(withTsc7.errors, ours) };
const typeScript7Changes = {
	added: minus(withTsc7.errors, withProjectTs.errors),
	removed: minus(withProjectTs.errors, withTsc7.errors)
};
const expected = [...minus(oracle, typeScript7Changes.removed), ...typeScript7Changes.added].sort();
const endToEnd = { onlyExpected: minus(expected, ours), onlyVueCheck: minus(ours, expected) };

/* A run only counts if it actually type-checked: vue-check exits 3 when the
 * generated code didn't parse (nothing was checked), and either tool can
 * crash. Such a run is invalid, never "0 errors". */
const invalid = Object.fromEntries(
	Object.entries(results).map(([name, list]) => [
		name,
		list
			.filter(
				(item) =>
					(item.exitCode !== 0 && item.exitCode !== 1 && item.exitCode !== 2) ||
					// vue-check writes its phase report only after a finished check, so a
					// run without one crashed, whatever its exit code says.
					(name.startsWith('vue-check') && !item.phases)
			)
			.map((item) => item.exitCode)
	])
);
const anyInvalid = Object.values(invalid).some((codes) => codes.length > 0) || !withProjectTs.finished || !withTsc7.finished;

const versionOf = (command: string[]) => {
	const result = Bun.spawnSync(command, { cwd: project, stderr: 'pipe', stdout: 'pipe' });

	return result.stdout.toString().trim().split('\n')[0] ?? '';
};
/* Everything a reader needs to judge or repeat the numbers. */
const environment = {
	bun: Bun.version,
	containerMemoryLimit: (() => {
		try {
			const limit = readFileSync('/sys/fs/cgroup/memory.max', 'utf8').trim();

			return limit === 'max' ? 'none' : `${Math.round(Number(limit) / 2 ** 30)} GB`;
		} catch {
			return 'unknown';
		}
	})(),
	cpu: cpus()[0]?.model ?? 'unknown',
	node: process.version,
	projectTypeScript: versionOf(['node', join(project, 'node_modules/typescript/bin/tsc'), '--version']),
	totalMemoryGB: Math.round(totalmem() / 2 ** 30),
	tsc7: versionOf([join(tool, 'node_modules/@typescript/typescript-linux-x64/lib/tsc'), '--version']),
	// vue-tsc --version prints TypeScript's version; vue-tsc's own is in its package.
	vueTsc: (() => {
		try {
			return (JSON.parse(readFileSync(join(project, 'node_modules/vue-tsc/package.json'), 'utf8')) as { version: string }).version;
		} catch {
			return 'unknown';
		}
	})()
};

const report = {
	environment,
	invalidRuns: invalid,
	oraclesFinished: withProjectTs.finished && withTsc7.finished,
	cores: availableParallelism(),
	/** Same errors as vue-tsc on the same TypeScript, and end to end apart from exactly TypeScript 7's own changes. */
	identical:
		!anyInvalid &&
		vueHandling.onlyVueCheck.length === 0 &&
		vueHandling.onlyVueTsc.length === 0 &&
		endToEnd.onlyExpected.length === 0 &&
		endToEnd.onlyVueCheck.length === 0,
	/** bun check against real tsc 7.0.2, on exactly the same generated code: must be empty both ways. */
	bunCheckMatchesTsc7: bunVsTsc7.onlyBunCheck.length === 0 && bunVsTsc7.onlyTsc7.length === 0,
	bunVsTsc7,
	endToEnd,
	typeScript7Changes,
	vueHandling,
	project,
	runs,
	summary
};
mkdirSync(join(tool, 'bench/results'), { recursive: true });
const name = option('name', 'result');
writeFileSync(join(tool, `bench/results/${name}.json`), JSON.stringify(report, null, '\t'));
writeFileSync(join(tool, `bench/results/${name}.raw.json`), JSON.stringify(results));
console.log(JSON.stringify(report, null, 2));
