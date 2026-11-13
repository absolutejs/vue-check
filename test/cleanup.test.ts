import { expect, test } from 'bun:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/* .vue-check/ links to the project's node_modules and package.json files
 * (codegen.ts) and is deleted at the start of every run (generate.ts). The
 * delete must remove the links, never what they point to. */
test('clearing .vue-check removes its links, not their targets', () => {
	const base = mkdtempSync(join(tmpdir(), 'vue-check-cleanup-'));
	const modules = join(base, 'project', 'node_modules', 'pkg');
	mkdirSync(modules, { recursive: true });
	writeFileSync(join(modules, 'index.js'), 'kept');
	writeFileSync(join(base, 'project', 'package.json'), '{}');
	const mirror = join(base, 'project', '.vue-check', 'project');
	mkdirSync(mirror, { recursive: true });
	symlinkSync(join(base, 'project', 'node_modules'), join(mirror, 'node_modules'));
	symlinkSync(join(base, 'project', 'package.json'), join(mirror, 'package.json'));

	rmSync(join(base, 'project', '.vue-check'), { force: true, recursive: true });

	expect(existsSync(join(base, 'project', '.vue-check'))).toBe(false);
	expect(readFileSync(join(modules, 'index.js'), 'utf8')).toBe('kept');
	expect(readFileSync(join(base, 'project', 'package.json'), 'utf8')).toBe('{}');
	rmSync(base, { force: true, recursive: true });
});
