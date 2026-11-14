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

Vue's template parser isn't part of `@vue/language-core`: it comes from the `@vue/compiler-dom` the
project installed. The native parser reproduces `@vue/compiler-dom` **3.5.36 to 3.5.45** (the parser's
source is the same across them, and each one installed in the benchmark is checked), with the 7.0
line of the `entities` package it decodes entities with. A project that resolves any other version
uses the JavaScript codegen.

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

## How it is called

`src/generate.ts` walks the project as before: the Vue files of the tsconfig, then every one reached
by import. For a project the port reproduces (`src/native.ts` decides), each queue of files goes to
the binary in one run, as lines of JSON on its standard input: the file's text, the Vue version the
code is for (`target`), and the folder of language-core's type declarations relative to the file.
Each line that comes back is the file's generated code and mappings, or the reason the port leaves
the file alone. Everything after that is the same code for both: the two repairs, the language of
the generated file, its syntax check, the links and `codegen.json`.

A project is one the port reproduces when:

- its `@vue/language-core`, `@vue/compiler-dom` and `entities` are the versions above;
- its `vueCompilerOptions` are the defaults. `target` and `typesRoot` are free (they are passed to
  the binary), and so is `vitePressExtensions`, which only adds files the binary isn't asked about;
- no file changes how later files parse (see below). When one does, the whole project is generated
  again by the JavaScript codegen.

`VUE_CHECK_NATIVE=0` turns the native codegen off, `VUE_CHECK_NATIVE=<path>` names the binary, and
`VUE_CHECK_NATIVE_EXPLAIN=1` prints which codegen made each file and why.
`bench/codegen-bar.sh <project> <tsconfig> <out>` runs both and compares the two folders.

## What native declines

The port stands behind a result or it doesn't return one. Two cases go back to the JavaScript codegen:

- **A file whose result it can't guarantee.** Where the original's behavior comes from JavaScript
  itself (an offset before the start of the text, half of a surrogate pair), the native side marks
  the file unsupported instead of guessing. The same goes for a file that sets a compiler option in
  a comment (`<!-- @strictTemplates true -->`), a template in a language other than HTML, and a
  directive expression in which a decoded entity sits next to characters outside ASCII (the
  original then counts offsets in the decoded text).
- **A project with a file that changes how later files parse.** Vue's parser keeps its `v-pre` and
  `<pre>` state between files and never clears it: a file that ends inside `v-pre`, or that makes the
  parser throw halfway, changes how every file after it in the same process is parsed. Only the
  original, run in the original order, reproduces that.

## Proving each layer

The folder diff is the bar, and it only says *that* two outputs differ. So each layer is also compared
on its own, against the project's own packages, value for value:

```sh
native/oracle/run.sh <project> <results-dir>     # every .vue file of a project
bun native/oracle/stress.ts <dir> --count=3000   # generated files real projects don't contain
MODULES=<a project's node_modules> native/oracle/run.sh <dir> <results-dir>
```

`native/oracle/dump.ts` runs in a container and writes what the project's `@vue/compiler-dom` and
`@vue/language-core` make of each file: the syntax tree with every location, the errors, and the
blocks of the single-file component. `vue-check-codegen parse` writes the same from the native side,
and `native/oracle/compare.ts` compares them.

Real projects contain almost no malformed templates, so `stress.ts` generates them, from a seeded
generator: malformed tags, every kind of attribute and directive, entities, comments and CDATA,
raw-text blocks, characters outside ASCII, and files cut off at any point, so that every state of the
tokenizer is also an end state.

The last layer is the whole file. The oracle records the code Volar hands TypeScript for each file
and its mappings (source offsets, generated offsets, lengths, and which errors each range reports);
`vue-check-codegen generate` writes the same from the file's text, and
`native/oracle/compare-generated.ts` compares them. The one input the native side is given besides
the text is the path of language-core's type declarations relative to the file, which the `///
<reference>` lines at the top carry.

## Writing the Rust

- **No `unsafe`.** It is to Rust what a type cast is to TypeScript: a claim the compiler can't check.
  The crate forbids it (`unsafe_code = "forbid"`). If a block is ever unavoidable, it carries a
  `// SAFETY:` comment stating the invariant it relies on and why it holds, which clippy enforces.
- **The other escapes are as rare.** No lint is switched off without a reason, numbers change type
  through `From`/`TryFrom` and never a silent `as`, and nothing that can fail on real input is assumed
  not to (`unwrap`, `expect` and `panic!` are denied). `cargo clippy` runs with `all` and `pedantic`
  denied.
- **Offsets are bytes inside, UTF-16 at the edge.** Rust strings are UTF-8 and everything that reads
  the output (Volar's source maps, TypeScript) counts UTF-16 code units. Offsets are converted once,
  where results leave the program (`utf16.rs`).
- **JavaScript's definitions, not Rust's.** Where the two differ (what `trim` removes, what `\s`
  matches), `js.rs` has JavaScript's.

