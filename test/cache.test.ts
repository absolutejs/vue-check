import { expect, test } from 'bun:test';
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fingerprintOf, keyOf, readsOf, remember, remembered } from '../src/cache';

/* A project on disk: a source folder the tsconfig includes, a file of the
 * program in a sibling package, and one in node_modules. */
const project = () => {
	const base = mkdtempSync(join(tmpdir(), 'vue-check-cache-'));
	const root = join(base, 'app');
	const outDir = join(root, '.vue-check');
	for (const folder of [join(root, 'src'), outDir, join(base, 'shared'), join(root, 'node_modules', 'pkg')]) mkdirSync(folder, { recursive: true });
	const files = {
		config: join(root, 'tsconfig.json'),
		declaration: join(root, 'node_modules', 'pkg', 'index.d.ts'),
		shared: join(base, 'shared', 'index.ts'),
		source: join(root, 'src', 'main.ts'),
		vue: join(root, 'src', 'App.vue')
	};
	for (const file of Object.values(files)) writeFileSync(file, 'one');
	const reads = () =>
		readsOf({
			configs: [files.config],
			outDir,
			programFiles: [files.source, files.shared, files.declaration, join(outDir, 'app', 'src', 'App.vue.ts')],
			root,
			skipped: [],
			sources: [files.vue],
			trees: [join(root, 'src')]
		});

	return { base, files, outDir, reads, root };
};
/* A change a second later: a file's time is what is compared. */
const later = (file: string, seconds = 1) => {
	const time = new Date(Date.now() + seconds * 1000);
	utimesSync(file, time, time);
};

test('nothing changed: the same fingerprint, and the result is remembered', () => {
	const { base, outDir, reads, root } = project();
	const key = keyOf(root, join(root, 'tsconfig.json'), outDir, []);
	remember(outDir, key, reads(), 2, 'src/App.vue(1,1): error TS1: x\n');

	expect(fingerprintOf(reads())).toBe(fingerprintOf(reads()));
	expect(remembered(outDir, key)).toEqual({ exitCode: 2, output: 'src/App.vue(1,1): error TS1: x\n' });
	rmSync(base, { force: true, recursive: true });
});

test('every kind of change forgets the result', () => {
	const changes: [string, (made: ReturnType<typeof project>) => void][] = [
		['a source file', ({ files }) => later(files.source)],
		['a Vue file', ({ files }) => later(files.vue)],
		['a file of another package', ({ files }) => later(files.shared)],
		['a declaration file in node_modules', ({ files }) => later(files.declaration)],
		['the tsconfig', ({ files }) => later(files.config)],
		['a file with a new size and the same time', ({ files }) => writeFileSync(files.source, 'longer')],
		['a new file in an included folder', ({ root }) => writeFileSync(join(root, 'src', 'new.ts'), '')],
		['a file that isn\'t code in an included folder', ({ root }) => writeFileSync(join(root, 'src', 'template.html'), '')],
		['a removed file', ({ files }) => rmSync(files.source)],
		['a new file beside one of another package', ({ base }) => writeFileSync(join(base, 'shared', 'index.d.ts'), '')],
		['a newly installed package', ({ root }) => mkdirSync(join(root, 'node_modules', 'other'))],
		['a lockfile', ({ root }) => writeFileSync(join(root, 'bun.lock'), '')]
	];
	for (const [name, change] of changes) {
		const made = project();
		const key = keyOf(made.root, made.files.config, made.outDir, []);
		remember(made.outDir, key, made.reads(), 0, '');
		expect(remembered(made.outDir, key), `before ${name}`).toEqual({ exitCode: 0, output: '' });
		change(made);
		expect(remembered(made.outDir, key), name).toBeNull();
		rmSync(made.base, { force: true, recursive: true });
	}
});

test('another command is another run', () => {
	const { base, files, outDir, reads, root } = project();
	const key = keyOf(root, files.config, outDir, []);
	remember(outDir, key, reads(), 0, '');

	expect(remembered(outDir, keyOf(root, files.config, outDir, ['--skipLibCheck']))).toBeNull();
	expect(remembered(outDir, keyOf(root, join(root, 'tsconfig.app.json'), outDir, []))).toBeNull();
	rmSync(base, { force: true, recursive: true });
});

test('what a run writes is not what it read', () => {
	const { base, files, outDir, reads, root } = project();
	const key = keyOf(root, files.config, outDir, []);
	remember(outDir, key, reads(), 0, '');
	writeFileSync(join(outDir, 'tsconfig.json'), '{}');

	expect(remembered(outDir, key)).toEqual({ exitCode: 0, output: '' });
	rmSync(base, { force: true, recursive: true });
});
