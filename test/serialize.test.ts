import { describe, expect, test } from 'bun:test';
import { codeFeatures } from '@vue/language-core';
import { shouldReportDiagnostics } from '@volar/language-core';
import type { SourceMap } from '../src/generate';
import { deserialize, serialize } from '../src/serialize';

const codes = [2304, 2339, 2353, 2561, 2578, 6133];
const segment = (data: unknown) => ({ data, generated: [0], generatedLengths: [1], lengths: [1], source: [0] });
const mapOf = (...data: unknown[]): Record<string, SourceMap> => ({
	'a.vue.ts': {
		mappings: data.map(segment),
		shift: 0,
		shouldReport: (value, code) => shouldReportDiagnostics(value as Parameters<typeof shouldReportDiagnostics>[0], 'undefined', code),
		source: 'a.vue'
	}
});

describe('serialize', () => {
	test("decides every code as Volar does for Vue's fixed rules and booleans", () => {
		const data = [
			codeFeatures.full,
			{ verification: false },
			{ ...codeFeatures.doNotReportTs6133 },
			{ ...codeFeatures.doNotReportTs2353AndTs2561 },
			{ verification: {} }
		];
		const live = mapOf(...data);
		const serial = serialize(live, codeFeatures as never, codes);
		const map = serial.maps['a.vue.ts'];
		expect(map?.regenerate).toBe(false);
		const decided = deserialize(map!, serial.rules);
		for (const [index, value] of data.entries())
			for (const code of codes)
				expect([index, code, decided.shouldReport(decided.mappings[index]?.data, String(code))]).toEqual([
					index,
					code,
					live['a.vue.ts']!.shouldReport(value, String(code))
				]);
	});

	test('marks a file with any other rule (e.g. stateful @vue-expect-error) for Volar', () => {
		let seen = 0;
		const stateful = { verification: { shouldReport: () => (seen++, false) } };
		const serial = serialize(mapOf(codeFeatures.full, stateful), codeFeatures as never, codes);
		expect(serial.maps['a.vue.ts']?.regenerate).toBe(true);
		expect(seen).toBe(0);
	});
});
