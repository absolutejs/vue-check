import type { SourceMap } from './generate';

/*
 * The maps as data, so the error mapping doesn't need Vue's codegen loaded
 * again. A segment's `verification` (which errors vue-tsc reports there) is
 * kept as Volar evaluates it:
 *
 *   - true / false (or an object with no rule): reported or not, for every code;
 *   - one of Vue's fixed rules (`codeFeatures`, matched by identity: the
 *     codegen spreads those objects into its mappings): a pure function of the
 *     error code, evaluated here for every TypeScript diagnostic code and kept
 *     as the codes it rejects;
 *   - any other rule (for example the stateful ones under @vue-expect-error,
 *     which count the errors they see): not data. That file is marked, and the
 *     CLI regenerates it to ask Volar itself, as before.
 */

export type Verify = boolean | number;
export type SerialMap = Omit<SourceMap, 'mappings' | 'shouldReport'> & {
	mappings: (Omit<SourceMap['mappings'][number], 'data'> & { verify: Verify })[];
	/** A segment has a rule that isn't data: ask Volar (regenerate the file). */
	regenerate: boolean;
};
export type Serialized = {
	/** Every diagnostic code the fixed rules were evaluated for. */
	knownCodes: number[];
	maps: Record<string, SerialMap>;
	/** Per fixed rule: the codes it rejects. `verify: n` refers to rules[n]. */
	rules: number[][];
};

type Rule = { shouldReport?: (source: string | undefined, code: string | number) => boolean };

export const serialize = (
	maps: Record<string, SourceMap>,
	codeFeatures: Record<string, { verification?: unknown }>,
	diagnosticCodes: number[]
): Serialized => {
	const fixed = new Map<unknown, number>();
	const rules: number[][] = [];
	for (const feature of Object.values(codeFeatures)) {
		const verification = feature?.verification as Rule | undefined;
		if (typeof verification !== 'object' || !verification?.shouldReport || fixed.has(verification)) continue;
		fixed.set(verification, rules.length);
		const { shouldReport } = verification;
		rules.push(diagnosticCodes.filter((code) => !shouldReport('undefined', String(code))));
	}
	const verifyOf = (data: unknown): Verify | null => {
		const verification = (data as { verification?: unknown } | null)?.verification;
		if (typeof verification !== 'object' || verification === null) return Boolean(verification);
		if (!(verification as Rule).shouldReport) return true;

		return fixed.get(verification) ?? null;
	};

	return {
		knownCodes: diagnosticCodes,
		maps: Object.fromEntries(
			Object.entries(maps).map(([file, map]) => {
				let regenerate = false;
				const mappings = map.mappings.map(({ data, ...mapping }) => {
					const verify = verifyOf(data);
					if (verify === null) regenerate = true;

					return { ...mapping, verify: verify ?? false };
				});

				return [file, { mappings, regenerate, shift: map.shift, source: map.source }];
			})
		),
		rules
	};
};

/** A SourceMap that decides from the data, as Volar would have. */
export const deserialize = (map: SerialMap, rules: number[][]): SourceMap => ({
	mappings: map.mappings.map(({ verify, ...mapping }) => ({ ...mapping, data: verify })),
	shift: map.shift,
	shouldReport: (data, code) => (typeof data === 'number' ? !(rules[data] ?? []).includes(Number(code)) : Boolean(data)),
	source: map.source
});
