# vue-check

Type-check Vue projects on **Bun's native TypeScript checker**. `vue-check` reports what `vue-tsc`
reports, and uses a fraction of the memory and time.

```sh
bunx @absolutejs/vue-check -p tsconfig.json
```

It accepts the same compiler flags as `vue-tsc --noEmit` (for example `--skipLibCheck`) and prints errors
in the same format, so it can replace `vue-tsc --noEmit` in a `typecheck` script. It needs
[Bun](https://bun.com) 1.4.3 or newer; Node projects can add Bun as a dev dependency.

## How it works

`vue-tsc` doesn't type-check Vue itself. Vue's codegen (`@vue/language-core`) turns each `.vue` file
into TypeScript, with a map back to the source, and `tsc` checks that TypeScript. The memory and time go
to `tsc`, running in Node over the whole project.

`vue-check` keeps Vue's codegen and replaces the checker:

1. **The same codegen.** It runs the `@vue/language-core` your project's `vue-tsc` uses, resolved
   through your `vue-tsc`, and writes the generated TypeScript to `.vue-check/`.
2. **Bun's checker.** `bun check` checks your project and the generated files together. `rootDirs` joins
   the two trees, so `import Foo from './Foo.vue'` resolves to the generated code and `.ts` files see your
   components' real types.
3. **The same errors.** Each error is mapped back to its line and column in the `.vue` file. Vue's
   source maps say, per position, which errors `vue-tsc` reports there. `vue-check` applies the same
   rules with Volar's own `shouldReportDiagnostics`, so you see what `vue-tsc` would show and nothing
   from Vue's internal scaffolding.

It never hides a failure. If generated code doesn't parse, a checker stops at the syntax error and checks
nothing, so `vue-check` exits with an error that names the file instead of reporting a clean result.

## How it's tested

The same way Bun proved its checker against TypeScript: a differential, with the reference tool as the
oracle and no expected output stored.

- **Real projects, with planted errors.** `bench/mutate.ts` plants known errors across a copy of a large
  Vue app: wrong types in `<script setup>`, unknown names and misspelled methods in templates, and type
  errors in plain `.ts` files. Both tools check the copy, and every error must match: code, message,
  file, line and column. A clean project alone would only prove that both print nothing.
- **TypeScript 7.** `bun check` follows TypeScript 7, and `vue-tsc` runs your project's TypeScript (5 or
  6). A difference counts as TypeScript 7's own only when real `tsc` 7.0.2, the version Bun tests
  against, reports it on your plain TypeScript and your TypeScript doesn't. The benchmark checks this
  automatically. Any other difference is a bug.
- **Unit tests** cover the codegen repair below (`bun test`).

## Results

Measured in an isolated container (no network, memory-capped). Memory is the peak RSS of the whole
process tree, so `vue-check`'s codegen and its `bun check` child are counted together. Times are wall
clock; the figures are medians.

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
