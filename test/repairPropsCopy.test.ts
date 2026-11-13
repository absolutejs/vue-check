import { describe, expect, test } from 'bun:test';
import { repairPropsCopy } from '../src/generate';

const copy = (props: string) =>
	`const __VLS_8 = __VLS_asFunctionalComponent1(__VLS_7, new __VLS_7({\n// @ts-ignore\n${props}}));\n`;

describe("undoing language-core's comment rewrite in a props copy", () => {
	test('a URL in an attribute comes back intact, at the same length', () => {
		const broken = copy('href: "https:/*lindell.me/JsBarcode/", */ target: "_blank", ...{ style: {} }, ');
		const repaired = repairPropsCopy(broken);
		expect(repaired).toContain('href: "https://lindell.me/JsBarcode/",\n   target: "_blank"');
		expect(repaired.length).toBe(broken.length);
	});

	test('a real comment stays a comment', () => {
		const broken = copy('a: 1, /* note */ b: 2, ');
		expect(repairPropsCopy(broken)).toContain('a: 1, // note\n   b: 2, ');
	});

	test('code without the rewrite is untouched', () => {
		const clean = copy('a: 1, b: "x", ');
		expect(repairPropsCopy(clean)).toBe(clean);
	});
});

describe('an empty event handler with a modifier', () => {
	test('becomes an expression that parses, at the same length', async () => {
		const { repairEmptyHandlers } = await import('../src/generate');
		const broken = '...{ onClick: (__VLS_ctx.)},';
		const repaired = repairEmptyHandlers(broken);
		expect(repaired).toBe('...{ onClick: (null!     )},');
		expect(repaired.length).toBe(broken.length);
	});

	test('real member access is untouched', async () => {
		const { repairEmptyHandlers } = await import('../src/generate');
		expect(repairEmptyHandlers('(__VLS_ctx.onClose)')).toBe('(__VLS_ctx.onClose)');
	});
});
