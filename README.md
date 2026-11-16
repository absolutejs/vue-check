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

vue-check writes the TypeScript it generates to `.vue-check/` in the project, so add `.vue-check/` to
`.gitignore`, or put it in a folder you already ignore with `--out-dir=<dir>`.

## How it works

`vue-tsc` doesn't type-check Vue itself. Vue's codegen (`@vue/language-core`) turns each `.vue` file
into TypeScript, with a map back to the source, and `tsc` checks that TypeScript. The memory and time go
to `tsc`, running in Node over the whole project.

`vue-check` keeps Vue's codegen and replaces the checker, in three steps that each run after the
previous one has exited, so peak memory is the largest step, not their sum:

1. **The same codegen.** It runs the `@vue/language-core` your project's `vue-tsc` uses, set up the way
   `vue-tsc` sets it up, and writes the generated TypeScript to `.vue-check/`. For a project on
   `@vue/language-core` 3.3.9 to 3.3.14, a native port of that exact codegen
   writes the same files, byte for byte, faster ([docs/NATIVE.md](docs/NATIVE.md)); any other
   project, and any file the port declines, gets the JavaScript codegen as before.
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

The projects on `@vue/language-core` 3.3.9 to 3.3.14 have their code generated natively: all but
element-plus (3.1.5), directus (3.1.8) and naive-ui-admin (whose Vue tooling resolves an older
template compiler), which use their own JavaScript codegen.

<!-- RESULTS -->

| Project | vue-tsc | vue-check | vue-check (1 thread) | Faster | Less memory |
|---|---|---|---|---|---|
| 1panel | 29.21 s · 2747 MB | 3.87 s · 827 MB | 5.86 s · 836 MB | 7.5× | 3.3× |
| directus | 30.94 s · 2436 MB | 6.49 s · 724 MB | 9.34 s · 706 MB | 4.8× | 3.4× |
| dozzle | 7.03 s · 1394 MB | 1.62 s · 344 MB | 2.44 s · 295 MB | 4.3× | 4.1× |
| element-plus | 16.96 s · 1221 MB | 3.76 s · 367 MB | 4.48 s · 365 MB | 4.5× | 3.3× |
| gitea | 11.24 s · 1007 MB | 1.07 s · 309 MB | 1.33 s · 273 MB | 10.5× | 3.3× |
| halo-console-components | 2.8 s · 300 MB | 0.62 s · 219 MB | 0.65 s · 178 MB | 4.5× | 1.4× |
| misskey | 26.47 s · 2198 MB | 2.79 s · 558 MB | 4.34 s · 490 MB | 9.5× | 3.9× |
| naive-ui-admin | 10.12 s · 908 MB | 2.09 s · 302 MB | 2.55 s · 287 MB | 4.8× | 3.0× |
| reka-ui | 3.81 s · 634 MB | 1.1 s · 273 MB | 1.83 s · 231 MB | 3.5× | 2.3× |
| slidev | 5.69 s · 741 MB | 0.88 s · 243 MB | 1.07 s · 209 MB | 6.5× | 3.0× |
| soybean-admin | 1.33 s · 584 MB | 0.61 s · 207 MB | 0.81 s · 189 MB | 2.2× | 2.8× |
| vikunja | 21.11 s · 1423 MB | 2 s · 417 MB | 3.08 s · 355 MB | 10.6× | 3.4× |
| vitepress-client | 4.2 s · 367 MB | 0.87 s · 175 MB | 1.02 s · 176 MB | 4.8× | 2.1× |
| vitepress-docs | 2.13 s · 302 MB | 0.68 s · 165 MB | 0.71 s · 164 MB | 3.1× | 1.8× |
| vue-devtools | 3.08 s · 402 MB | 0.79 s · 180 MB | 0.93 s · 178 MB | 3.9× | 2.2× |
| vue-element-plus-admin | 4.66 s · 539 MB | 0.7 s · 216 MB | 0.9 s · 171 MB | 6.7× | 2.5× |
| vue-pure-admin | 19.42 s · 1467 MB | 2.14 s · 411 MB | 2.68 s · 371 MB | 9.1× | 3.6× |
| vue-vben-admin | 20.14 s · 1457 MB | 2.34 s · 446 MB | 3.27 s · 423 MB | 8.6× | 3.3× |
| vueuse | 3.73 s · 708 MB | 0.84 s · 309 MB | 1.27 s · 232 MB | 4.4× | 2.3× |

| Project | vue-tsc | Errors (vue-tsc) | Same as vue-tsc | bun check = tsc 7 | TypeScript 7 changes |
|---|---|---|---|---|---|
| 1panel | 3.3.9 | 267 | ✓ | ✓ | 10 |
| directus | 3.1.8 | 2298 | ✓ | ✓ | 234 |
| dozzle | 3.3.14 | 0 | ✓ | ✓ | 0 |
| element-plus | 3.1.5 | 1 | ✓ | ✓ | 3 |
| gitea | 3.3.14 | 0 | ✓ | ✓ | 0 |
| halo-console-components | 3.3.9 | 0 | ✓ | ✓ | 0 |
| misskey | 3.3.11 | 687 | ✓ | ✓ | 4 |
| naive-ui-admin | 3.3.13 | 0 | ✓ | ✓ | 0 |
| reka-ui | 3.3.11 | 0 | ✓ | ✓ | 2 |
| slidev | 3.3.14 | 1 | ✓ | ✓ | 0 |
| soybean-admin | 3.3.12 | 1 | ✓ | ✓ | 0 |
| vikunja | 3.3.11 | 0 | ✓ | ✓ | 0 |
| vitepress-client | 3.3.14 | 33 | ✓ | ✓ | 0 |
| vitepress-docs | 3.3.14 | 17 | ✓ | ✓ | 0 |
| vue-devtools | 3.3.14 | 95 | ✓ | ✓ | 4 |
| vue-element-plus-admin | 3.3.9 | 0 | ✓ | ✓ | 0 |
| vue-pure-admin-planted-errors | 3.3.13 | 220 | ✓ | ✓ | 24 |
| vue-pure-admin | 3.3.13 | 0 | ✓ | ✓ | 6 |
| vue-vben-admin | 3.3.11 | 0 | ✓ | ✓ | 0 |
| vueuse | 3.3.14 | 1 | ✓ | ✓ | 0 |

| Project | Vue files | Codegen | bun check | Mapping |
|---|---|---|---|---|
| 1panel | 623 | 1.38 s · 585 MB | 0.89 s · 494 MB | 1.36 s · 827 MB |
| directus | 597 | 3.64 s · 667 MB | 1.29 s · 638 MB | 1.48 s · 588 MB |
| dozzle | 219 | 0.72 s · 191 MB | 0.43 s · 286 MB | 0.42 s · 181 MB |
| element-plus | 165 | 2.22 s · 333 MB | 1.09 s · 303 MB | 0.33 s · 138 MB |
| gitea | 26 | 0.58 s · 149 MB | 0.41 s · 270 MB | 0.04 s · 58 MB |
| halo-console-components | 31 | 0.52 s · 185 MB | 0.08 s · 100 MB | 0.01 s · 39 MB |
| misskey | 592 | 0.86 s · 326 MB | 0.7 s · 460 MB | 1.09 s · 444 MB |
| naive-ui-admin | 68 | 1.61 s · 244 MB | 0.37 s · 259 MB | 0.09 s · 78 MB |
| reka-ui | 470 | 0.58 s · 176 MB | 0.3 s · 226 MB | 0.19 s · 121 MB |
| slidev | 121 | 0.72 s · 158 MB | 0.15 s · 203 MB | 0 s · 45 MB |
| soybean-admin | 90 | 0.5 s · 161 MB | 0.1 s · 168 MB | 0 s · 45 MB |
| vikunja | 249 | 0.87 s · 226 MB | 0.72 s · 360 MB | 0.35 s · 169 MB |
| vitepress-client | 66 | 0.5 s · 141 MB | 0.09 s · 111 MB | 0.26 s · 103 MB |
| vitepress-docs | 4 | 0.43 s · 131 MB | 0.06 s · 108 MB | 0.17 s · 82 MB |
| vue-devtools | 31 | 0.42 s · 146 MB | 0.08 s · 124 MB | 0.27 s · 133 MB |
| vue-element-plus-admin | 34 | 0.51 s · 133 MB | 0.15 s · 176 MB | 0.02 s · 53 MB |
| vue-pure-admin | 255 | 0.75 s · 208 MB | 1.09 s · 356 MB | 0.24 s · 130 MB |
| vue-vben-admin | 376 | 0.87 s · 225 MB | 1.1 s · 390 MB | 0.32 s · 166 MB |
| vueuse | 214 | 0.61 s · 164 MB | 0.22 s · 268 MB | 0 s · 47 MB |

Vue's conformance suite (vuejs/language-tools test-workspace/tsc): 251 of 252 cases identical on every leg; bun check = tsc 7 on 252 of 252. Different: `_failed_#4569` (see Known differences).

Environment: Intel(R) Core(TM) Ultra 7 155U, 12 GB RAM (container limit 6 GB), Bun 1.4.4+absolute.2, Node v26.3.0, oracle tsc 7.0.2. Medians of 5 runs.

## Known differences

- **TypeScript 7 semantics.** `vue-check` checks your code the way TypeScript 7 does. Where TypeScript 7
  differs from your TypeScript 5 or 6, it reports what `tsc` 7 reports; the benchmark verifies each case
  with real `tsc` 7.0.2.
- **Codegen normalization.** Two patterns Vue's codegen produces, `//` inside a string in a component's
  props copy and an event modifier with an empty handler (`@click.stop=""`), are normalized to valid
  TypeScript of the same length before checking, so every source-map offset stays exact
  (`src/generate.ts`). The first changes one result. In `<Child href="https://example.com"
  :count="'text'" />`, `vue-tsc` 3.3.14 reports no error for `count`, because its codegen turns the `//`
  in the URL into the start of a comment that hides the props after it. `vue-check` reports the error,
  as `vue-tsc` does when `count` comes before the URL.

- **`checkJs` with a JavaScript `<script setup>` on vue-tsc 3.2.** That version of Vue's codegen writes
  TypeScript into a JavaScript component. Without `checkJs`, vue-check checks it the way vue-tsc does
  (it reports nothing in that file and uses its types). With `checkJs` (or `// @ts-check`), vue-tsc
  checks it as JavaScript, which vue-check can't reproduce, so it stops with exit code 3 and says so.
  vue-tsc 3.3 writes JSDoc instead and isn't affected.
- **Vue files in a linked workspace package.** When a project reaches a Vue file of another package in
  its workspace through the link in `node_modules`, `vue-check` checks it at its real path and
  `vue-tsc` at the path under `node_modules`. An error in that file is reported under the other path,
  and an import in it that resolves from only one of the two places is an error in one tool and not
  the other.
- **One conformance case.** In `_failed_#4569`, which Vue's own suite marks as a known failure, a
  declaration-emit error (TS4025) on Vue's internal export is reported on a different line: `tsc`-style
  output carries where an error starts but not its length, and this error's span is the one the
  mapping can't recover.

Workarounds for upstream bugs, and when to remove them, are tracked in
[UPSTREAM_ISSUES.md](UPSTREAM_ISSUES.md).

## License

MIT
