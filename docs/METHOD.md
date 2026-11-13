# How vue-check works, and how it's proven

## What it does

`vue-check` type-checks a Vue project and reports the errors `vue-tsc --noEmit` reports, using Bun's
native TypeScript checker (`bun check`) instead of `tsc`.

## How it works

### 1. The same codegen as vue-tsc

`vue-tsc` doesn't type-check Vue directly. `@vue/language-core` turns each `.vue` file into TypeScript (a
"virtual file"), along with a source map back to the `.vue` file, and `tsc` checks that TypeScript.

`vue-check` runs **the project's own** `@vue/language-core`, resolved through the project's `vue-tsc` the
way `vue-tsc` loads it, with the project's `vueCompilerOptions`. The TypeScript it generates is the same
code `vue-tsc` checks. It's written to `.vue-check/`, mirroring the project's layout
(`src/App.vue` becomes `.vue-check/src/App.vue.ts`, or `.vue.tsx` for `lang="tsx"`).

The `.vue` files are found the way `tsc` and `vue-tsc` find them: from the tsconfig's `include` and
`exclude`, with `.vue` registered as an extra file extension.

### 2. One program for `.ts` and `.vue`

A generated tsconfig extends the project's own, and adds:

```json
{ "compilerOptions": { "rootDirs": ["<project>", "<project>/.vue-check"], "allowArbitraryExtensions": true } }
```

`rootDirs` makes the project and `.vue-check/` behave as one directory, so a relative import such as
`import Foo from './Foo.vue'` resolves to `Foo.vue.ts`. `.ts` files therefore see every component's real
type, the same as with `vue-tsc`: props, emits, slots and generics. Bun's checker implements `rootDirs` as
`tsc` does, so Bun itself needs no change.

Then a single `bun check` runs over the whole program, on every core.

### 3. Errors where vue-tsc reports them

Each error in a generated file is translated back to its line and column in the `.vue` file, through the
codegen's source map. Vue's source maps also carry, for each segment, a `verification` feature that says
which errors `vue-tsc` reports there. For example, internal scaffolding is never reported, and certain
error codes are suppressed on props. `vue-check` applies these rules with Volar's own
`shouldReportDiagnostics`, the function `vue-tsc` uses. As in Volar, an error counts only if both ends of
its span fall in segments that allow its code.

Errors in `.ts` files pass through unchanged. The output format is `vue-tsc`'s:
`file(line,column): error TSxxxx: message`.

### 4. Guarantees

- **Every generated file parses.** A checker that behaves like `tsc` reports only syntax errors when it
  finds any, and skips type checks. So before checking, `vue-check` parses every generated file with
  TypeScript's parser. Two codegen patterns that `vue-tsc`'s error-tolerant path accepts are normalized
  to valid TypeScript **without changing the text's length**, so every source-map offset stays exact:
  - `//` inside a string in a component's props copy;
  - an event modifier with an empty handler (`@click.stop=""`).

  If anything else doesn't parse, `vue-check` stops with exit code 3 and names the file. It never prints
  a clean result for a check that didn't run.
- **Exit codes as vue-tsc:** 0 for no errors, 2 for errors.

## How it's proven

The approach is the one Bun used to prove `bun check` against TypeScript: a **differential**. A reference
tool is the oracle, no expected output is stored, and any difference is a bug unless it's explained by a
named, verified cause.

### Three-way differential

Every project is checked three ways:

| | Codegen and mapping | Checker |
|---|---|---|
| `vue-tsc` | Vue's | `tsc` (the project's TypeScript, 5 or 6) |
| `vue-check` on `tsc` 7 | vue-check's | real `tsc` 7.0.2 |
| `vue-check` | vue-check's | `bun check` |

That answers two questions separately:

1. **Is Bun's checker right?** `vue-check` on `bun check` and `vue-check` on `tsc` 7 check exactly the
   same generated program, so their errors must be identical: every code, message, file, line and
   column. TypeScript 7.0.2 is the version Bun's own differential tests use as their oracle.
2. **Is the Vue handling right?** `vue-check` on `tsc` 7 must report what `vue-tsc` reports. Because
   `vue-tsc` runs the project's TypeScript 5 or 6, the only accepted difference is an error that
   TypeScript 7 itself reports differently. Such errors are listed separately, as TypeScript 7 changes.

### Planted errors (mutation testing)

A clean project only proves that both tools print nothing. So `bench/mutate.ts` plants known errors
across a copy of each project. Each error goes in a different file, chosen deterministically:

- a wrong type in `<script setup>`;
- an unknown name in a template binding;
- a misspelled method in a template expression;
- a type error in a plain `.ts` file.

Each planted error must be reported identically by every column of the three-way differential.

### What a valid run is

- Both tools must complete a real check. A run that exits with anything other than 0, 1 or 2 is
  recorded as invalid and never counted as a result.
- Every run is isolated in a container with no network, a 10 GB memory cap, and the tool mounted
  read-only.
- **Memory** is the peak RSS of the whole process tree, sampled from `/proc` every 20 ms, so `vue-check`'s
  codegen and its `bun check` child are counted together.
- **Time** is wall clock. Results are medians of 5 runs.
- `bun check` is also measured on one thread, so its parallelism can't flatter the comparison.
- Each result file records its environment: CPU, memory, container limit, and the versions of Bun, Node,
  the project's TypeScript, `vue-tsc` and `tsc` 7.

### Reproduce

```sh
git clone https://github.com/absolutejs/vue-check && cd vue-check && bun install
bench/run-in-docker.sh <vue-project> <name> --runs=5 --vue-tsc-args=--skipLibCheck
cat bench/results/<name>.json
```

## Results

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

