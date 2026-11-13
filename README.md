# vue-check

Type-check Vue projects on **Bun's native TypeScript checker**. `vue-check` reports what `vue-tsc`
reports, and uses a fraction of the memory and time.

```sh
bunx @absolutejs/vue-check -p tsconfig.json
```

It accepts the same compiler flags as `vue-tsc --noEmit` (for example `--skipLibCheck`) and prints errors
in the same format, so it can replace `vue-tsc --noEmit` in a `typecheck` script. It needs
[Bun](https://bun.com) 1.4.4 or newer (Node projects can add Bun as a dev dependency) and a project on
`vue-tsc` 3.

## How it works

`vue-tsc` doesn't type-check Vue itself. Vue's codegen (`@vue/language-core`) turns each `.vue` file
into TypeScript, with a map back to the source, and `tsc` checks that TypeScript. The memory and time go
to `tsc`, running in Node over the whole project.

`vue-check` keeps Vue's codegen and replaces the checker, in three steps that each run after the
previous one has exited, so peak memory is the largest step, not their sum:

1. **The same codegen.** It runs the `@vue/language-core` your project's `vue-tsc` uses, set up the way
   `vue-tsc` sets it up, and writes the generated TypeScript to `.vue-check/`.
2. **The same program, on Bun's checker.** `bun check` checks your project and the generated files as
   one program, file for file the one `vue-tsc` builds: the same root files in the same order, with
   `rootDirs` and your path aliases joining the two trees, so `import Foo from './Foo.vue'` (or
   `'@/components/Foo.vue'`) resolves to the generated code and `.ts` files see your components' real
   types.
3. **The same errors.** Each error is mapped back to its line and column in the `.vue` file by Volar's
   own source map and diagnostic filter, as `vue-tsc` maps it, so you see what `vue-tsc` would show and
   nothing from Vue's internal scaffolding.

It never hides a failure. If generated code doesn't parse, a checker stops at the syntax error and checks
nothing, so `vue-check` exits with an error that names the file instead of reporting a clean result.

## How it's tested

The same way Bun proved its checker against TypeScript: a differential, with the reference tools as
oracles and no expected output stored. Every project is checked four ways, and each claim is an exact
equality (every code, message, file, line and column):

- **The Vue handling.** `vue-check` on your project's own TypeScript reports exactly what `vue-tsc`
  reports.
- **Bun's checker.** `vue-check` on `bun check` reports exactly what it reports on real `tsc` 7.0.2,
  the version Bun tests against, for the same generated program.
- **TypeScript 7.** `bun check` follows TypeScript 7. Its changes are whatever the same program gives
  on your TypeScript and on `tsc` 7, by definition, and the end-to-end result must be `vue-tsc`'s with
  exactly those changes.
- **Planted errors.** `bench/mutate.ts` plants hundreds of known errors of 12 kinds across a copy of a
  large app: script setup, template bindings and interpolations, `v-if` and `v-for`, event handlers,
  props on components, `@vue-expect-error` used and unused, plain `.ts` files, and component props typed
  from a `.ts` file. A clean project alone would only prove that both print nothing.
- **Unit tests** cover the codegen normalization and the source-map serialization (`bun test`).

## Results

Measured in an isolated container (no network, memory-capped). Memory is the peak RSS of the whole
process tree, so every `vue-check` process is counted. Times are wall clock; the figures are medians.

<!-- RESULTS -->

| Project | vue-tsc | vue-check | vue-check (1 thread) | Faster | Less memory |
|---|---|---|---|---|---|
| element-plus | 15.91 s · 1225 MB | 2.64 s · 564 MB | 3.5 s · 511 MB | 6.0× | 2.2× |
| vue-pure-admin | 16.14 s · 1466 MB | 3.01 s · 711 MB | 3.74 s · 670 MB | 5.4× | 2.1× |

| Project | vue-tsc | Errors (vue-tsc) | Same as vue-tsc | bun check = tsc 7 | TypeScript 7 changes |
|---|---|---|---|---|---|
| element-plus | 3.1.5 | 1 | ✓ | ✓ | 3 |
| vue-pure-admin-planted-errors | 3.3.13 | 40 | ✓ | ✓ | 6 |
| vue-pure-admin | 3.3.13 | 0 | ✓ | ✓ | 6 |

Environment: Intel(R) Core(TM) Ultra 7 155U, 12 GB RAM (container limit 10 GB), Bun 1.4.4+absolute.2, Node v26.3.0, oracle tsc 7.0.2. Medians of 5 runs.

## Known differences

- **TypeScript 7 semantics.** `vue-check` checks your code the way TypeScript 7 does. Where TypeScript 7
  differs from your TypeScript 5 or 6, it reports what `tsc` 7 reports; the benchmark verifies each case
  with real `tsc` 7.0.2.
- **Codegen normalization.** Two patterns Vue's codegen produces, `//` inside a string in a component's
  props copy and an event modifier with an empty handler (`@click.stop=""`), are normalized to valid
  TypeScript of the same length before checking, so every source-map offset stays exact
  (`src/generate.ts`).

## License

MIT
