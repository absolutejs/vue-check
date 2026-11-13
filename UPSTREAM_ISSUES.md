# Upstream Issues

Bugs in vue-check's **dependencies** (Vue's `@vue/language-core`, Bun's `bun check`) that vue-check
works around. Each entry has the symptom, the root cause, the upstream issue to watch, the
workaround's full footprint, and what to change or remove once upstream fixes it.

vue-check runs **the project's own** `@vue/language-core`, not a pinned copy. So an upstream fix
doesn't retire a workaround on its own: the workaround stays as long as vue-check supports a
`vue-tsc` version without the fix, and it must stay harmless on versions that have it. **Only a
passing repro retires an entry**, never an issue's state: upstream sometimes closes an issue as a
duplicate or as fixed for a narrower case.

## Current status

Last checked: **2026-11-13**, on `vue-tsc` / `@vue/language-core` **3.3.14**, vue 3.5.25, TypeScript
5.9.3, Bun 1.4.4.

| #   | Problem                                                    | Upstream                       | Fixed? |
| --- | ---------------------------------------------------------- | ------------------------------ | ------ |
| 1   | A prop value containing `//` hides the props after it      | vuejs/language-tools#6360 open | No     |
| 2   | An empty event handler generates code that doesn't parse   | vuejs/language-tools#6361 open | No     |
| 3   | `bun check` output carries no error length                 | not filed                      | No     |

---

## 1. A prop value containing `//` (such as a URL) disables type-checking of the props after it

- **Dependency:** `@vue/language-core` (vuejs/language-tools),
  `packages/language-core/lib/codegen/template/element.ts`
- **Status:** open, filed by us 2026-11-13
    - **vuejs/language-tools#6360**: https://github.com/vuejs/language-tools/issues/6360

**Symptom.** On a component, a wrong prop type after a prop whose value contains `//` is not
reported by `vue-tsc`:

```vue
<Child href="https://example.com" :count="'text'" />   <!-- vue-tsc: no error -->
<Child :count="'text'" href="https://example.com" />   <!-- vue-tsc: TS2322, correctly -->
```

For vue-check the generated file doesn't parse at all, and a checker that behaves like `tsc` stops at
the syntax error and checks nothing.

**Root cause.** The component's props copy rewrites `//` comments as `/* … */` with a line regex,
`.replace(/\/\/(.*)$/gm, …)`, that also matches `//` inside string literals. `href="https://example.com"`
becomes `href: "https:/*example.com", */`, an unterminated string. `vue-tsc` hides those syntax errors
(the line is unmapped and under `// @ts-ignore`), but TypeScript's error recovery consumes the code
after it, so the props that follow are never checked.

**Workaround in vue-check.** The rewrite is undone exactly before checking: each `/*REST */ ` on a props
copy line becomes `//REST`, the line break it replaced and two spaces, the same length, so every
source-map offset still holds. Because the props after the URL are then checked, vue-check reports the
error `vue-tsc` misses (the one result this changes; see README "Known differences"). Footprint:

1. `src/generate.ts`: `PROPS_COPY`, `repairPropsCopy`, and its call in `repairGenerated`.
2. `test/repairPropsCopy.test.ts`: the `repairPropsCopy` tests (URL restored at the same length, a real
   comment stays a comment, other code untouched).
3. `README.md` "Known differences" and `docs/METHOD.md` "Guarantees": the `//` normalization bullet and
   the `<Child href="https://example.com" …>` example.
4. The blog post (absolutejs-docs, `src/frontend/components/blog/vueCheck/VueCheckContent.tsx`): the
   note that vue-check reports the error `vue-tsc` misses.

**On fixed versions.** `repairPropsCopy` only rewrites the comment pattern on props copy lines, so once
the codegen leaves strings alone it has nothing to undo (test: "code without the rewrite is untouched").
vue-check and `vue-tsc` then agree on the example above, and footprint 3 and 4 become wrong.

**Change when the fix ships:** in footprint 3 and 4, say which `vue-tsc` versions have the difference.
**Remove when:** vue-check's oldest supported `vue-tsc` includes the fix. Delete footprint 1 and 2, the
two doc mentions and the post note. Verify with the example above on that oldest version: `vue-tsc`
reports both lines, and vue-check (without the repair) reports the same.

---

## 2. An empty event handler (`@click=""`) generates TypeScript that doesn't parse

- **Dependency:** `@vue/language-core` (vuejs/language-tools),
  `packages/language-core/lib/codegen/template/elementEvents.ts`, `generateEventExpression`
- **Status:** open, filed by us 2026-11-13
    - **vuejs/language-tools#6361**: https://github.com/vuejs/language-tools/issues/6361

**Symptom.** `<button @click="">`, with or without modifiers (`@click.stop=""`, `@keyup.enter=""`),
generates `{ onClick: (__VLS_ctx.)}`: `TS1003: Identifier expected`. `vue-tsc` users see nothing, and
every other error in the file is still reported (TypeScript's parser recovers). For vue-check, a
syntax error that maps nowhere stops the run with exit 3.

**Root cause.** `generateEventExpression` wraps the expression in `(…)` and generates it as an
interpolation even when `prop.exp.content` is empty, so the context access has no property name.

**Workaround in vue-check.** `__VLS_ctx.` before `)` becomes `null!` padded to the same length: an
expression of type `never`, which reports nothing, as `vue-tsc` shows nothing for an empty handler.
Footprint:

1. `src/generate.ts`: `EMPTY_HANDLER`, `repairEmptyHandlers`, and its call in `repairGenerated`.
2. `test/repairPropsCopy.test.ts`: the `repairEmptyHandlers` tests (parses at the same length; real
   member access untouched).
3. `README.md` "Known differences" and `docs/METHOD.md` "Guarantees": the empty-handler bullet.

**On fixed versions.** The pattern `__VLS_ctx.)` no longer occurs, so the repair does nothing.

**Remove when:** vue-check's oldest supported `vue-tsc` includes the fix. Delete footprint 1 to 3.
Verify on that version that `<button @click="">` and `<button @click.stop="">` generate code that parses
(no exit 3 without the repair), and that the conformance suite (`bench/conformance.ts`) is unchanged.

---

## 3. `bun check` output carries no error length

- **Dependency:** Bun (`bun check`)
- **Status:** **not filed.** A feature gap rather than a bug: `bun check --no-pretty` prints `tsc`'s
  one-line format, `file(line,column): error TSxxxx: message`, which has no length either. Filing it
  means asking for a machine-readable output (for example `--format=json` with each diagnostic's start
  and length), which would help any tool that maps diagnostics.

**Symptom.** Which source-map segments an error touches depends on its span, and Volar decides whether
`vue-tsc` reports an error from its start **and** end. With only a start, vue-check has to reconstruct
the length.

**Workaround in vue-check.** `spanAt` in `src/cli.ts` takes the length from TypeScript's parser on the
generated file: the parser's own diagnostic for a syntax error, the innermost syntax node starting at
the position otherwise, and the comment for an unused `@ts-expect-error`. It matches on every project
benchmarked and on 251 of 252 conformance cases. The exception is `_failed_#4569` (a declaration-emit
error, TS4025, on Vue's internal export), whose span isn't the node at its start (README "Known
differences").

**Remove when:** `bun check` can print each diagnostic's length. Then read the length from the output,
delete `spanAt`, `SPAN` and the lazy TypeScript parser in `src/cli.ts`, and rerun the conformance suite:
`_failed_#4569` should become identical.
