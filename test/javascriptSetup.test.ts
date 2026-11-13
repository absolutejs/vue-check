import { describe, expect, test } from 'bun:test';
import ts from 'typescript';
import { twinLanguage } from '../src/generate';

/* Vue's codegen 3.2 writes typed scaffolding into the JavaScript virtual file
 * of a <script setup> without lang="ts"; 3.3 writes JSDoc instead. A checker
 * that behaves like tsc stops at TypeScript syntax in JavaScript (a syntax
 * error to it), so such a file is checked as TypeScript with its own errors
 * off, as vue-tsc reports nothing in a JavaScript file without checkJs. */

// What @vue/language-core 3.2.6 generates for `<script setup>` (JavaScript).
const typedScaffolding = `const fail = () => {};
// @ts-ignore
declare const { defineProps, defineSlots }: typeof import('vue');
type __VLS_SetupExposed = import('vue').ShallowUnwrapRef<{ fail: typeof fail }>;
const __VLS_ctx = {} as __VLS_SetupExposed;
`;
// What 3.3 generates for the same component.
const jsdocScaffolding = `const fail = () => {};
const __VLS_ctx = /** @type {import('vue').ComponentPublicInstance} */ ({});
`;

describe('twinLanguage', () => {
	test('JavaScript with TypeScript syntax is checked as TypeScript with its errors off', () => {
		expect(twinLanguage(ts, '.js', typedScaffolding, false)).toEqual({ extension: '.ts', prefix: '// @ts-nocheck\n', unsupported: false });
		expect(twinLanguage(ts, '.jsx', typedScaffolding, false).extension).toBe('.tsx');
	});

	test('JavaScript without it stays JavaScript', () => {
		expect(twinLanguage(ts, '.js', jsdocScaffolding, false)).toEqual({ extension: '.js', prefix: '', unsupported: false });
	});

	test('TypeScript is untouched', () => {
		expect(twinLanguage(ts, '.ts', typedScaffolding, false)).toEqual({ extension: '.ts', prefix: '', unsupported: false });
	});

	test('with checkJs it is unsupported, never checked the wrong way', () => {
		expect(twinLanguage(ts, '.js', typedScaffolding, true).unsupported).toBe(true);
		expect(twinLanguage(ts, '.js', jsdocScaffolding, true).unsupported).toBe(false);
	});
});
