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
  mirror, and a generated file resolves each import to exactly the file its source would.
- **The same options.** The generated config extends the project's. Two options are carried over the
  way TypeScript 7 needs them: `baseUrl`, which TypeScript 7 has removed, becomes its documented
  replacement (a catch-all path alias); relative `types` entries stay resolved from the project's
  config.

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
  syntax errors when it finds any, and skips type checks. So `vue-check` parses every generated file
  with TypeScript's parser. A syntax error that maps back to the user's code (`{{ a + }}`) is theirs:
  the check runs and reports it, as `vue-tsc` does. Two codegen patterns that `vue-tsc`'s
  error-tolerant path accepts are normalized to valid TypeScript **without changing the text's
  length**, so every source-map offset stays exact:
  - `//` inside a string in a component's props copy;
  - an event modifier with an empty handler (`@click.stop=""`).

  Any other syntax error that maps nowhere would stop the checker on codegen's text, so `vue-check`
  stops with exit code 3 and names the file.
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
