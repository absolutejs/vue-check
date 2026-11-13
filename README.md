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
| element-plus | 18.9 s · 1220 MB | 5.43 s · 420 MB | 6.65 s · 420 MB | 3.5× | 2.9× |
| gitea | 10.49 s · 1015 MB | 1.97 s · 314 MB | 2.5 s · 271 MB | 5.3× | 3.2× |
| halo-console-components | 2.58 s · 301 MB | 1.39 s · 227 MB | 1.61 s · 230 MB | 1.9× | 1.3× |
| naive-ui-admin | 8.61 s · 914 MB | 2.43 s · 303 MB | 2.88 s · 301 MB | 3.5× | 3.0× |
| slidev | 4.43 s · 741 MB | 1.71 s · 285 MB | 2.04 s · 285 MB | 2.6× | 2.6× |
| soybean-admin | 1.56 s · 577 MB | 1.71 s · 270 MB | 1.76 s · 271 MB | 0.9× | 2.1× |
| vitepress-client | 3.93 s · 366 MB | 2.05 s · 268 MB | 1.78 s · 264 MB | 1.9× | 1.4× |
| vitepress-docs | 1.66 s · 302 MB | 0.73 s · 185 MB | 0.76 s · 182 MB | 2.3× | 1.6× |
| vue-element-plus-admin | 5.49 s · 537 MB | 1.38 s · 235 MB | 1.72 s · 238 MB | 4.0× | 2.3× |
| vue-pure-admin | 19.15 s · 1466 MB | 4.33 s · 410 MB | 5.12 s · 409 MB | 4.4× | 3.6× |
| vue-vben-admin | 23.17 s · 1445 MB | 4.54 s · 447 MB | 5.92 s · 421 MB | 5.1× | 3.2× |

| Project | vue-tsc | Errors (vue-tsc) | Same as vue-tsc | bun check = tsc 7 | TypeScript 7 changes |
|---|---|---|---|---|---|
| element-plus | 3.1.5 | 1 | ✓ | ✓ | 3 |
| gitea | 3.3.14 | 0 | ✓ | ✓ | 0 |
| halo-console-components | 3.3.9 | 0 | ✓ | ✓ | 0 |
| naive-ui-admin | 3.3.13 | 0 | ✓ | ✓ | 0 |
| slidev | 3.3.14 | 1 | ✓ | ✓ | 0 |
| soybean-admin | 3.3.12 | 1 | ✓ | ✓ | 0 |
| vitepress-client | 3.3.14 | 33 | ✓ | ✓ | 0 |
| vitepress-docs | 3.3.14 | 17 | ✓ | ✓ | 0 |
| vue-element-plus-admin | 3.3.9 | 0 | ✓ | ✓ | 0 |
| vue-pure-admin-planted-errors | 3.3.13 | 220 | ✓ | ✓ | 24 |
| vue-pure-admin | 3.3.13 | 0 | ✓ | ✓ | 6 |
| vue-vben-admin | 3.3.11 | 0 | ✓ | ✓ | 0 |

| Project | Vue files | Codegen | bun check | Mapping |
|---|---|---|---|---|
| element-plus | 165 | 2.26 s · 339 MB | 0.92 s · 297 MB | 2.17 s · 420 MB |
| gitea | 26 | 1.21 s · 238 MB | 0.39 s · 275 MB | 0.33 s · 140 MB |
| halo-console-components | 31 | 0.97 s · 193 MB | 0.09 s · 100 MB | 0.23 s · 98 MB |
| naive-ui-admin | 68 | 1.52 s · 269 MB | 0.32 s · 257 MB | 0.55 s · 188 MB |
| slidev | 121 | 1.55 s · 252 MB | 0.15 s · 206 MB | 0 s · 40 MB |
| soybean-admin | 90 | 1.57 s · 236 MB | 0.13 s · 169 MB | 0 s · 39 MB |
| vitepress-client | 66 | 1.47 s · 234 MB | 0.12 s · 109 MB | 0.39 s · 121 MB |
| vitepress-docs | 4 | 0.49 s · 151 MB | 0.06 s · 106 MB | 0.15 s · 87 MB |
| vue-element-plus-admin | 34 | 0.9 s · 201 MB | 0.16 s · 175 MB | 0.31 s · 121 MB |
| vue-pure-admin | 255 | 2.82 s · 369 MB | 0.98 s · 353 MB | 0.45 s · 192 MB |
| vue-vben-admin | 376 | 2.58 s · 390 MB | 0.98 s · 391 MB | 0.92 s · 299 MB |

Vue's conformance suite (vuejs/language-tools test-workspace/tsc): 251 of 252 cases identical on every leg; bun check = tsc 7 on 252 of 252. Different: `_failed_#4569` (see Known differences).

Environment: Intel(R) Core(TM) Ultra 7 155U, 12 GB RAM (container limit 10 GB), Bun 1.4.4+absolute.2, Node v26.3.0, oracle tsc 7.0.2. Medians of 5 runs.

## Known differences

- **TypeScript 7 semantics.** `vue-check` checks your code the way TypeScript 7 does. Where TypeScript 7
  differs from your TypeScript 5 or 6, it reports what `tsc` 7 reports; the benchmark verifies each case
  with real `tsc` 7.0.2.
- **Codegen normalization.** Two patterns Vue's codegen produces, `//` inside a string in a component's
  props copy and an event modifier with an empty handler (`@click.stop=""`), are normalized to valid
  TypeScript of the same length before checking, so every source-map offset stays exact
  (`src/generate.ts`).

- **One conformance case.** In `_failed_#4569`, which Vue's own suite marks as a known failure, a
  declaration-emit error (TS4025) on Vue's internal export is reported on a different line: `tsc`-style
  output carries where an error starts but not its length, and this error's span is the one the
  mapping can't recover.

## License

MIT
