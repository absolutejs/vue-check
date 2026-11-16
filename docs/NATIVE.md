# Native codegen

vue-check's time and memory now go mostly to Vue's codegen (`@vue/language-core`, JavaScript), not
to the check: on the median project it's about two thirds of a run, and usually the peak memory. This
is the plan to make it native, and the bar it has to pass.

## The bar

For a version it claims, the native codegen must write **the same output folder** as the JavaScript
codegen (`src/codegen.ts`) on the same project: every generated file byte for byte, and
`codegen.json` equal (every source map, every segment's verification rule, the files that can't be checked).
Identical input to the checker means identical errors by construction. `bench/codegen-diff.ts`
compares the two folders.

It's checked on every Vue file of every benchmarked project, the planted-error copy, and every case
of Vue's conformance suite, then by the existing four-way differential end to end.

## Versions

Native codegen covers exact `@vue/language-core` versions: **3.3.9 to 3.3.14** (source:
vuejs/language-tools tags `v3.3.9` to `v3.3.14`). The port is of the latest, and the binary is told
the project's version. Where an earlier one writes something else, it writes what that one does:

- **3.3.12 and 3.3.13** differ from the latest at a few points: where the macros are declared, the
  template's code in a function of its own, the type of an event handler's arguments, how a custom
  directive is called.
- **3.3.9 to 3.3.11** are the codegen as it was before 3.3.12 restructured it. An expression's
  identifiers are found by a different walk (`binding_references_old.rs`), each is read from the
  context or with `.value` with no second pass, the bindings a template reads are declared as a
  type of the context, and every helper is written as TypeScript whatever the script's language.

Each version is compared with its own install, layer by layer, like the first. A project on any
other version uses the JavaScript codegen, which stays exact. A new Vue release can make a project
slower until it's ported, never wrong. Porting a release means porting its codegen diff and passing
the bar again.

Vue's template parser isn't part of `@vue/language-core`: it comes from the `@vue/compiler-dom` the
project installed. The native parser reproduces `@vue/compiler-dom` **3.5.35 to 3.5.45**, with the 7.0
line of the `entities` package it decodes entities with. The parser's source is the same from 3.5.36
on, and each version installed in the benchmark is checked. 3.5.35 differs in one behaviour, which the
port follows when the project resolves it: a CDATA section outside any element makes that parser
throw, which changes how it parses every later file, so such a file sends the project to the
JavaScript codegen. A project that resolves any other version uses the JavaScript codegen.

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
the generated file, the links and `codegen.json`.

A project is one the port reproduces when:

- its `@vue/language-core`, `@vue/compiler-dom` and `entities` are the versions above;
- every `vueCompilerOptions` it sets is one the port reads, with a value the port reads it as. They
  are passed to the binary, as `target` and `typesRoot` are:
  - strict templates: `strictTemplates`, or `strictVModel` and the `checkUnknown…` options one by one;
  - `fallthroughAttributes`, `checkRequiredFallthroughAttributes`, `fallthroughComponentNames`;
  - the `infer…Dollar…` options and `jsxSlots`;
  - `macros` and `composables` (the names `defineProps`, `useAttrs` and the rest go by);
  - `dataAttributes` and `htmlAttributes`;
  - `resolveStyleClassNames` and `strictCssModules`;
  - `lib` and `optionsWrapper`.

  `vitePressExtensions` is free too: it only adds files the binary isn't asked about. Any other
  option that isn't at its default leaves the project to the JavaScript codegen: `plugins`,
  `extensions`, `petiteVueExtensions`, `vapor`, `skipTemplateCodegen`, `resolveStyleImports` and
  `experimentalModelPropName`;
- no file changes how later files parse (see below). When one does, the whole project is generated
  again by the JavaScript codegen.

`VUE_CHECK_NATIVE=0` turns the native codegen off, `VUE_CHECK_NATIVE=<path>` names the binary, and
`VUE_CHECK_NATIVE_EXPLAIN=1` prints which codegen made each file and why.
`bench/codegen-bar.sh <project> <tsconfig> <out>` runs both and compares the two folders.

## How it ships

The binary is built for six platforms (Linux, macOS and Windows, each on x64 and arm64) and published
as one npm package per platform, `@absolutejs/vue-check-<platform>`, each holding only that binary.
`@absolutejs/vue-check` names all six as optional dependencies at its own exact version, so an
install fetches the one for the machine. `src/native.ts` looks for that package, and uses its binary
only if its version is vue-check's own. With no binary (another platform, or an install without
optional dependencies) vue-check runs the JavaScript codegen: the output is the same either way.

The Linux binaries are static (musl), so one per architecture serves glibc distributions and Alpine.

`.github/workflows/release.yml` builds the six binaries for a `v*` tag into a draft release, and
runs each one its runner can. `scripts/release.sh`, on the release machine, checks every binary is
the format, architecture and version its name claims, then publishes the packages.

## What native declines

The port stands behind a result or it doesn't return one. Two cases go back to the JavaScript codegen:

- **A file whose result it can't guarantee.** Where the original's behavior comes from JavaScript
  itself (an offset before the start of the text, half of a surrogate pair), the native side marks
  the file unsupported instead of guessing. The same goes for a file that sets a compiler option in
  a comment (`<!-- @strictTemplates true -->`), a template in a language other than HTML, and a
  `v-for` expression in which a decoded entity sits next to characters outside ASCII (the original
  then cuts the source at offsets counted in the decoded text). And for a template expression that
  contains `<!--` or `-->`: JavaScript reads those as the start of a comment in a script, and
  TypeScript,
  which parses the expression for the original, never does (`a<!--b` is `a < !--b`).
- **A project with a file that changes how later files parse.** Vue's parser keeps its `v-pre` and
  `<pre>` state between files and never clears it: a file that ends inside `v-pre`, or that makes the
  parser throw halfway, changes how every file after it in the same process is parsed. Only the
  original, run in the original order, reproduces that.

One case looks like those and is reproduced. The original locates the parts of a directive's
expression by adding offsets in the expression's text to where it starts in the source. When an
entity in it was decoded (`v-html="a + '&nbsp;é'"`), that text is shorter than the source, and the
offsets are counted in UTF-16 units of the decoded text from there on. The port counts in bytes of
the source and converts once, where the result leaves it (`utf16.rs`); for such a value that one
conversion counts as the original does. The parser records the value; an offset the codegen counted
in the written text instead (the expression is read a second way, as written, when it is checked
again after its element) is marked so.

## The import reader

vue-tsc checks every Vue file its program reaches by import, listed in the tsconfig or not. To find
them, vue-check reads the imports of every source file and of every generated file and follows the
ones that lead outside the project's own list. Reading the imports is TypeScript's
`preProcessFile`: a walk over the scanner's tokens, with no parser.

`native/src/imports` is a port of `preProcessFile` and of the scanner under it, as far as
`preProcessFile` drives it. The binary reads the files across the machine's cores
(`vue-check-codegen imports`), and reports the imports of the code it generates along with the code.
Which file an import means is still decided by the project's own TypeScript (`resolveModuleName`),
as is the tsconfig and its list of files: those define the program, and stay the project's.

It is used for a TypeScript in which everything ported is, text for text, what was ported:
`src/native.ts` hashes the source of `preProcessFile`, `createScanner` and the functions and tables
those two read, out of the project's `typescript.js`, and compares it with the hashes of the versions
the oracle below has passed on. Those are TypeScript 5.7.3 to 6.0.3 (two hashes: the source is the
same within 5.x and within 6.0) and the 6.0.3 bridge build that ships TypeScript 7's compiler behind
the JavaScript API. With any other TypeScript, and for any file the port declines, vue-check asks the
project's `preProcessFile`. The port declines a file that isn't UTF-8 and an import whose name holds
half a surrogate pair (`'\uD800'`), which a JavaScript string can hold and a Rust one can't.

TypeScript 7 itself has no `preProcessFile`: its package is the native compiler, with no JavaScript
API. Vue's codegen needs that API, so a project checked by vue-tsc has a TypeScript 5 or 6 (or the
bridge) for it, and that is the one read here.

`preProcessFile` is wrong in consistent ways, because it has no parser (the text of a regular
expression is scanned as code, for one), and the port is wrong in the same ways: what matters is
that both list the same names in the same order.

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

The import reader has its own oracle: `native/oracle/run-imports.sh <dir> <results-dir>` compares it
with the project's `preProcessFile` on every script file under a folder, `node_modules` included
(packages hold syntax a project's own code doesn't), and `stress-imports.ts` generates files real
projects don't contain: real files cut off at any point, with the scanner's awkward tokens (escapes,
unterminated strings and templates, numeric literals, merge-conflict markers, shebangs) put in at
random places.

The last layer is the whole file. The oracle records the code Volar hands TypeScript for each file
and its mappings (source offsets, generated offsets, lengths, and which errors each range reports);
`vue-check-codegen generate` writes the same from the file's text, and
`native/oracle/compare-generated.ts` compares them. The one input the native side is given besides
the text is the path of language-core's type declarations relative to the file, which the `///
<reference>` lines at the top carry.

## Results

The folder diff passes (every generated file and `codegen.json` equal) on each project below, on a
planted-error copy, and on all 252 conformance cases; the four-way differential (`bench/sweep.sh`)
is unchanged with native codegen on. Native is on by default where the versions and options match,
so `bench/codegen-bar.sh <project> <tsconfig> <out>` reruns the comparison for one project.

The whole codegen step (`src/codegen.ts`: reading the project, generating every file, following
imports and writing the folder) with each codegen, from one run of `bench/codegen-bar.sh` per
project on the same machine as the README's tables. The folder each wrote is identical.

| project | Vue files | generated natively | JavaScript codegen | native codegen | faster |
|---|---|---|---|---|---|
| 1panel | 623 | 623 | 7.29 s | 1.62 s | 4.5× |
| dozzle | 219 | 219 | 3.33 s | 0.70 s | 4.7× |
| gitea | 26 | 26 | 1.65 s | 0.53 s | 3.1× |
| halo-console-components | 31 | 31 | 1.31 s | 0.49 s | 2.6× |
| misskey | 592 | 592 | 5.11 s | 1.07 s | 4.8× |
| reka-ui | 470 | 470 | 2.85 s | 0.76 s | 3.8× |
| slidev | 121 | 120 | 2.12 s | 0.69 s | 3.1× |
| soybean-admin | 90 | 90 | 1.78 s | 0.58 s | 3.1× |
| vikunja | 249 | 248 | 3.05 s | 0.88 s | 3.5× |
| vitepress-client | 66 | 66 | 1.57 s | 0.54 s | 2.9× |
| vitepress-docs | 4 | 4 | 0.96 s | 0.47 s | 2.0× |
| vue-devtools | 31 | 31 | 1.66 s | 0.61 s | 2.7× |
| vue-element-plus-admin | 34 | 34 | 1.36 s | 0.52 s | 2.6× |
| vue-pure-admin | 255 | 255 | 3.13 s | 0.85 s | 3.7× |
| vue-vben-admin | 376 | 376 | 3.13 s | 0.94 s | 3.3× |
| vueuse | 214 | 214 | 2.93 s | 0.73 s | 4.0× |

The two files left to the JavaScript codegen are one with a comment that sets a compiler option
(slidev) and one whose block layout the port declines (vikunja).

The import reader lists the same imports as `preProcessFile` on every script file of six projects and
Vue's conformance suite, `node_modules` included (about 326,000 files, 1.8 GB, on TypeScript 5.9.3,
6.0.3 and the bridge), and on generated files (20,000 to 60,000 per version) on TypeScript 5.7.3,
5.8.2, 5.8.3, 5.9.2, 5.9.3, 6.0.2 and 6.0.3. It declined under 1% of the generated files (half a
surrogate pair), and no real one.

Each option the port reads has its own branches in the codegen. The whole-file comparison is run
with them set (`VUE_OPTIONS='{"strictTemplates":true}' native/oracle/run.sh …`), in thirteen
combinations over the conformance suite's files and generated sets, with no differences; and the
folder diff passes on every conformance case with the options each sets.

A file with `<!-- @vue-expect-error -->` has a rule in its source map that counts the errors it
sees, which is a function and not data. The code and the map are generated natively all the same;
the map marks the rule, and when the checker reports an error in that file vue-check asks Volar
itself about it, as it does for such a file from the JavaScript codegen.

For 3.3.9 to 3.3.13, the same comparisons run against an install of each: every layer on the
conformance suite's files and the generated sets, and the whole file again with the options the
port reads set, with no differences. The folder diff passes on the benchmarked projects that run
them: halo, vue-element-plus-admin and 1panel (3.3.9), vue-vben-admin, misskey, reka-ui and vikunja
(3.3.11), soybean-admin (3.3.12), vue-pure-admin and its planted-error copy (3.3.13).

For 3.3.9 to 3.3.11 three comparisons below the whole file are looser, each because those versions
don't keep what is compared: the script analysis lists its bindings as ranges, not by name and
kind; the port writes some runs of plain text in other pieces; and the names an expression reads
aren't logged in order. The whole-file comparison, which is exact, covers what each of them feeds.

The script codegen differs by Vue version (3.3, 3.4, 3.5 and 3.6 each change the code written). The
whole-file comparison is run at each (`TARGET=3.4 native/oracle/run.sh …`), on the conformance
suite's files and on generated ones, with no differences.

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

