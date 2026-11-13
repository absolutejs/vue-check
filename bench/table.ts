#!/usr/bin/env bun
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

/*
 * Renders bench/results/*.json as Markdown tables and writes them between the
 * <!-- RESULTS --> markers in README.md and docs/METHOD.md. Numbers only ever
 * come from result files.
 *
 *   bun bench/table.ts
 */

type Phases = { checkMB: number; checkS: number; codegenMB: number; codegenS: number; mapMB: number; mapS: number; vueFiles: number };
type Summary = Record<string, { errors: number; medianPeakRssMB: number; medianWallS: number; phases?: Phases }>;
type Result = {
	bunCheckMatchesTsc7: boolean;
	environment?: Record<string, string | number>;
	identical: boolean;
	runs: number;
	summary: Summary;
	typeScript7Changes: { added: string[]; removed: string[] };
};

const root = join(import.meta.dir, '..');
const directory = join(root, 'bench/results');
const results = readdirSync(directory)
	.filter((file) => file.endsWith('.json') && !file.endsWith('.raw.json'))
	.sort()
	.map((file) => ({ name: file.replace(/\.json$/, ''), result: JSON.parse(readFileSync(join(directory, file), 'utf8')) as Result }));

const times = (a: number, b: number) => `${(a / b).toFixed(1)}×`;
const yes = (value: boolean) => (value ? '✓' : '✗');

const performance = results.filter(({ result }) => result.runs > 1);
const proof = results;
const lines: string[] = [];
lines.push('| Project | vue-tsc | vue-check | vue-check (1 thread) | Faster | Less memory |');
lines.push('|---|---|---|---|---|---|');
for (const { name, result } of performance) {
	const tsc = result.summary['vue-tsc'];
	const ours = result.summary['vue-check'];
	const single = result.summary['vue-check (1 thread)'];
	if (!tsc || !ours || !single) continue;
	lines.push(
		`| ${name} | ${tsc.medianWallS} s · ${tsc.medianPeakRssMB} MB | ${ours.medianWallS} s · ${ours.medianPeakRssMB} MB | ${single.medianWallS} s · ${single.medianPeakRssMB} MB | ${times(tsc.medianWallS, ours.medianWallS)} | ${times(tsc.medianPeakRssMB, ours.medianPeakRssMB)} |`
	);
}
lines.push('');
lines.push('| Project | vue-tsc | Errors (vue-tsc) | Same as vue-tsc | bun check = tsc 7 | TypeScript 7 changes |');
lines.push('|---|---|---|---|---|---|');
for (const { name, result } of proof)
	lines.push(
		`| ${name} | ${result.environment?.vueTsc ?? '?'} | ${result.summary['vue-tsc']?.errors ?? '?'} | ${yes(result.identical)} | ${yes(result.bunCheckMatchesTsc7)} | ${result.typeScript7Changes.added.length + result.typeScript7Changes.removed.length} |`
	);
lines.push('');
lines.push('| Project | Vue files | Codegen | bun check | Mapping |');
lines.push('|---|---|---|---|---|');
for (const { name, result } of performance) {
	const phases = result.summary['vue-check']?.phases;
	if (phases)
		lines.push(
			`| ${name} | ${phases.vueFiles} | ${phases.codegenS} s · ${phases.codegenMB} MB | ${phases.checkS} s · ${phases.checkMB} MB | ${phases.mapS} s · ${phases.mapMB} MB |`
		);
}
const environment = results.find(({ result }) => result.environment)?.result.environment;
if (environment)
	lines.push(
		'',
		`Environment: ${environment.cpu}, ${environment.totalMemoryGB} GB RAM (container limit ${environment.containerMemoryLimit}), Bun ${environment.bun}, Node ${environment.node}, oracle tsc ${environment.tsc7}. Medians of ${performance[0]?.result.runs ?? '?'} runs.`
	);
const table = lines.join('\n');

for (const file of ['README.md', 'docs/METHOD.md']) {
	const path = join(root, file);
	const text = readFileSync(path, 'utf8');
	const updated = text.replace(/<!-- RESULTS[^>]*-->[\s\S]*?(?=\n## |\n?$)/, `<!-- RESULTS -->\n\n${table}\n`);
	writeFileSync(path, updated);
}
console.log(table);
