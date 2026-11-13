# Native codegen

vue-check's time and memory now go mostly to Vue's codegen (`@vue/language-core`, JavaScript), not
to the check: on the median project it's about two thirds of a run, and usually the peak memory. This
is the plan to make it native, and the bar it has to pass.

## The bar

For a version it claims, the native codegen must write **the same output folder** as the JavaScript
codegen (`src/codegen.ts`) on the same project: every generated file byte for byte, and
`codegen.json` equal (every source map, every segment's verification rule, the unparsed list).
Identical input to the checker means identical errors by construction. `bench/codegen-diff.ts`
compares the two folders.

It's checked on every Vue file of every benchmarked project, the planted-error copy, and every case
of Vue's conformance suite, then by the existing four-way differential end to end.

## Versions

Native codegen covers exact `@vue/language-core` versions, starting with **3.3.14** (source:
vuejs/language-tools tag `v3.3.14`). A project on any other version uses the JavaScript codegen,
which stays exact. A new Vue release can make a project slower until it's ported, never wrong.
Porting a release means porting its codegen diff and passing the bar again.

## Stages

**Stage 1: native Vue codegen.** A Rust binary takes a batch of Vue files with their resolved
`vueCompilerOptions` and returns each file's generated code and source maps (verification rules as
data). `src/codegen.ts` calls it instead of `@vue/language-core` for a supported version.

- SFC blocks (`utils/parseSfc.ts`), the template parser (`@vue/compiler-dom`'s `parse`) and
  language-core's transforms (`template/compile.ts`: v-bind shorthand, v-if, v-for, element, text).
- The codegen: `codegen/script`, `codegen/template`, `codegen/style`, and the `vue-tsx` plugin's
  assembly into embedded codes, with Volar-compatible mappings.
- Script and template-expression analysis (`parsers/`, `interpolation.ts`, `bindingReferences.ts`) on
  [oxc](https://oxc.rs)'s parser, reproducing what language-core reads from TypeScript's AST.

**Stage 2: native orchestration.** tsconfig reading, Vue file discovery and import scanning move into
the binary, so codegen no longer loads TypeScript.

Each stage ships only after it passes the bar on everything above.
