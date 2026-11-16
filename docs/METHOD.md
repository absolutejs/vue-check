# How vue-check works, and how it's proven

## What it does

`vue-check` type-checks a Vue project and reports the errors `vue-tsc --noEmit` reports, using Bun's
native TypeScript checker (`bun check`) instead of `tsc`.

## How it works

`vue-check` runs three steps, each in its own process, and each after the previous one has exited:

| Step | Process | What it holds |
|---|---|---|
| 1. Codegen | `src/codegen.ts` | TypeScript and Vue's compiler, every parsed `.vue` file |
| 2. Check | `bun check` | the program |
| 3. Mapping | the CLI | the errors and the source maps |

Because no step outlives the next one, peak memory is the largest step, not their sum. Codegen's memory
is returned to the system before the check starts.

### 1. The same codegen as vue-tsc

`vue-tsc` doesn't type-check Vue directly. `@vue/language-core` turns each `.vue` file into TypeScript (a
"virtual file"), along with a source map back to the `.vue` file, and `tsc` checks that TypeScript.

`vue-check` runs **the project's own** `@vue/language-core`, resolved through the project's `vue-tsc` the
way `vue-tsc` loads it, and sets it up as `vue-tsc` does: the project's `vueCompilerOptions`, Vue's
global types written where that version expects them, and every file extension Vue is configured for
(`.vue`, plus for example `.md` in a VitePress project). The TypeScript it generates is the code
`vue-tsc` checks, in the language `vue-tsc` gives it (a plain `<script>` is JavaScript).

A JavaScript file stays JavaScript, with one exception. Some versions of Vue's codegen (3.2) write
typed scaffolding (`declare`, type aliases, casts) into the JavaScript of a `<script setup>` without
`lang="ts"`. A checker that behaves like `tsc` treats TypeScript syntax in JavaScript as a syntax error
and checks nothing, where `vue-tsc` hides it. Without `checkJs`, `vue-tsc` reports nothing in such a file
and only uses its types, so vue-check checks it as TypeScript with `// @ts-nocheck`: the same types, and
the same silence. With `checkJs` it stops with exit code 3 rather than check it differently.

It generates every Vue file the program reaches, not only the project's own: `vue-tsc` creates a virtual
file for any Vue file its program resolves, such as a monorepo's workspace packages. Imports are read
with TypeScript's `preProcessFile` (a scan, not a full parse), from the generated code and from source
files outside `node_modules`, and resolved as the project resolves them.

Each generated file is written to `.vue-check/` at its source's absolute path (`/app/src/App.vue`
becomes `.vue-check/app/src/App.vue.ts`).

### 2. The same program, on Bun's checker

A generated tsconfig extends the project's own and makes the program the one `vue-tsc` builds, file
for file:

- **The same root files, in the same order.** They're listed explicitly, in the order `tsc` lists the
  project's, with each Vue file replaced in place by its generated TypeScript. The order files enter a
  program decides the order the checker creates types, and with it how types print in messages.
- **The same imports.** `rootDirs: ["/", ".vue-check"]` makes every folder and its mirror behave as one,
  so `import Foo from './Foo.vue'` resolves to `Foo.vue.ts` wherever the two files are. `rootDirs`
  covers relative imports only, so each path alias (`@/*`) also gets its mirrored counterpart:

  ```json
  {
    "compilerOptions": {
      "rootDirs": ["/", "<project>/.vue-check"],
      "paths": { "@/*": ["<project>/src/*", "<project>/.vue-check/<project>/src/*"] }
    }
  }
  ```

  A generated file resolves its own imports from its folder: bare imports walk up `node_modules`
  folders, and the nearest `package.json` decides a file's module format. So every folder above a
  source that has a `node_modules` or a `package.json` gets a link to it at the same place in the
  mirror, and a generated file resolves each import to exactly the file its source would. Only those
  two are linked: TypeScript follows a link to its real file for what it resolves through
  `node_modules`, so those files enter the program once, but a linked source file would enter it twice.

  A Vue file imported from a package (`import Picker from 'some-package/Picker.vue'`) resolves inside
  `node_modules`, where `rootDirs` doesn't apply and a package's `exports` target is looked up exactly
  as written. So each such import gets an exact path alias to its generated TypeScript.
- **The same options.** The generated config extends the project's. Two options are carried over the
  way TypeScript 7 needs them: `baseUrl`, which TypeScript 7 has removed, becomes its documented
  replacement (a catch-all path alias); relative `types` entries stay resolved from the project's
  config. The project's incremental build file (`incremental`, `tsBuildInfoFile`) describes the program
  `vue-tsc` builds, so `vue-check` never reads or replaces it: the check runs without one, and a
  composite project keeps its build file in `.vue-check/`.

`.ts` files therefore see every component's real type, the same as with `vue-tsc`: props, emits, slots
and generics. Bun's checker implements `rootDirs` and `paths` as `tsc` does, so Bun itself needs no
change. On vue-pure-admin, `tsc --listFiles` prints the same 3,174 files in the same order for both
programs.

Then a single `bun check` runs over the whole program, on every core, with `--no-script` (so it always
type-checks, never a project's own `check` script in its place).

### 3. Errors where vue-tsc reports them

Each error in a generated file is translated back to the `.vue` file with **Volar's own `SourceMap`**,
loaded from the project's install, and the same call `vue-tsc` makes: `toSourceRange` with
`fallbackToAnyMatch`, filtered by `shouldReportDiagnostics`. Vue's source maps carry, for each segment,
a `verification` feature that says which errors `vue-tsc` reports there. Internal scaffolding is never
reported, for example, and certain error codes are suppressed on props.

Most of what a checker reports in generated code is in that scaffolding: hundreds of unused names in
a project with no error of its own. `toSourceRange` maps a range by where it starts, and with no
mapping there that reports the error's code it yields nothing whatever the range's length. So that
is asked first, with Volar's own call, and the error's length (which needs TypeScript's parser) is
only worked out for an error whose start is mapped. `VUE_CHECK_VERIFY_SPANS=1` does both for every
error and fails if they ever disagree; the differential is also run with it.

The maps travel from step 1 to step 3 as data, so the mapping never loads the codegen again:

- a segment's `verification` that is `true` or `false` is kept as is;
- one of Vue's fixed rules (its `codeFeatures`, which are pure functions of the error code) is matched
  by identity and evaluated in step 1 for every TypeScript diagnostic code;
- any other rule, such as the stateful ones under `@vue-expect-error` (which count the errors they see),
  marks the file. The CLI regenerates just that file and asks Volar itself, in the order `vue-tsc` does.

Errors in `.ts` files pass through unchanged. The output format is `vue-tsc`'s:
`file(line,column): error TSxxxx: message`.

### 4. Guarantees

- **Syntax errors are reported as vue-tsc reports them.** A checker that behaves like `tsc` reports only
  syntax errors when it finds any, and skips type checks. A syntax error that maps back to the user's
  code (`{{ a + }}`) is theirs: the check reports it, as `vue-tsc` does. Two codegen patterns that `vue-tsc`'s
  error-tolerant path accepts are normalized to valid TypeScript **without changing the text's
  length**, so every source-map offset stays exact:
  - `//` inside a string in a component's props copy;
  - an event with an empty handler (`@click=""`).

  The first changes one result: `vue-tsc` 3.3.14 doesn't check the props after a URL in a component's
  props (`<Child href="https://example.com" :count="'text'" />`), because its codegen turns the URL's
  `//` into a comment, and vue-check does. Both are tracked, with their removal steps, in
  `UPSTREAM_ISSUES.md` (vuejs/language-tools#6360 and #6361).

  Any other syntax error that maps nowhere is codegen's own, which `vue-tsc` hides and then
  type-checks past; the checker stops on it instead. The checker parses every file, so its output
  says when that happened: when the syntax errors it stopped on were all hidden ones, `vue-check`
  exits with code 3 and names the file, never a clean result.
- **Only what the project installed.** Vue's codegen, Volar and TypeScript are loaded from the
  project's `node_modules`, never from Bun's global install cache (which Bun's `require` falls back to
  for a package the project doesn't have).
- **A check that didn't run is never clean.** If the checker fails without printing a single error
  (a crash, an unreadable config), `vue-check` exits 3. Errors with no position (an invalid option, a
  missing file) are reported as `tsc` prints them.
- **Exit codes:** 0 for no errors, 2 for errors (as `vue-tsc`), 3 when the project couldn't be checked.

## How it's proven

The approach is the one Bun used to prove `bun check` against TypeScript: a **differential**. Reference
tools are the oracles, no expected output is stored, and every claim is an exact equality.

### Four-way differential

Every project is checked four ways. The checker is the only thing `vue-check` lets you swap
(`VUE_CHECK_TSC`), so between two `vue-check` runs nothing else differs:

| | Codegen, program and mapping | Checker |
|---|---|---|
| `vue-tsc` | Vue's | the project's TypeScript (5 or 6) |
| `vue-check` on the project's TypeScript | vue-check's | the project's TypeScript |
| `vue-check` on `tsc` 7 | vue-check's | real `tsc` 7.0.2 |
| `vue-check` | vue-check's | `bun check` |

That answers each question separately:

1. **Is the Vue handling right?** `vue-check` on the project's TypeScript must report exactly what
   `vue-tsc` reports. Same TypeScript, so no difference is allowed.
2. **Is Bun's checker right?** `vue-check` on `bun check` and on `tsc` 7 check the same generated
   program, so they must agree exactly. TypeScript 7.0.2 is the version Bun's own differential tests use
   as their oracle.
3. **What does TypeScript 7 change?** The same program on the project's TypeScript and on `tsc` 7. These
   changes are measured, not chosen.
4. **End to end.** `vue-check` must equal `vue-tsc` with exactly those TypeScript 7 changes applied.

### Planted errors (mutation testing)

A clean project only proves that both tools print nothing. So `bench/mutate.ts` plants hundreds of known
errors across a copy of each project, spread deterministically over its files, in 12 kinds that each
exercise a different part of the mapping:

| Kind | What it exercises |
|---|---|
| a wrong type in `<script setup>` | script mapping |
| an unknown name in a template binding | template context |
| a misspelled method in an interpolation | template expressions |
| an unknown name in `v-if` | directives |
| a wrong member on a `v-for` item | scoped template variables |
| an unknown event handler | event codegen |
| a wrong prop type on a built-in component | component props (Vue's fixed suppression rules) |
| `@vue-expect-error` with no error under it | Vue's stateful rule: reported |
| `@vue-expect-error` with an error under it | Vue's stateful rule: suppressed |
| a wrong member in a ternary | expression spans |
| a type error in a plain `.ts` file | pass-through |
| an unknown prop, typed from a `.ts` file through the component's import | the joined program |

Template edits alternate between the start and the end of the template. Each planted error must come
out identically from every leg of the differential.

### Vue's own conformance suite

`vuejs/language-tools` keeps a test project per behavior and per fixed bug in `test-workspace/tsc`:
the cases `vue-tsc`'s own type-check test runs. Most assert their errors in place with
`@ts-expect-error` and `@vue-expect-error`, so a clean result proves both that nothing is reported
wrongly and that every expected error is there; the rest leave known errors that must come out
identically. `bench/conformance.ts` runs every case through the same four-way differential.

### What a valid run is

- Every run must complete a real check. A run that exits with anything other than 0, 1 or 2, or (for
  `vue-check`) that never reaches the end of its check, is recorded as invalid and never counted.
- Every run is isolated in a container with no network, a 10 GB memory cap, and the tool mounted
  read-only.
- **Memory** is the peak RSS of the whole process tree, sampled from `/proc` every 20 ms, so every
  `vue-check` process is counted. Each step's own peak is recorded too.
- **Time** is wall clock, with each step timed. Results are medians of 5 runs.
- `bun check` is also measured on one thread, so its parallelism can't flatter the comparison.
- Each result file records its environment: CPU, memory, container limit, and the versions of Bun, Node,
  the project's TypeScript, `vue-tsc` and `tsc` 7.

### Reproduce

```sh
git clone https://github.com/absolutejs/vue-check && cd vue-check && bun install
bench/run-in-docker.sh <vue-project> <name> --runs=5 --vue-tsc-args=--skipLibCheck
cat bench/results/<name>.json
```

For a package in a monorepo, set `WORKSPACE=<monorepo root>`.

## Results

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

