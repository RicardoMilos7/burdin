# The Burdin website

The documentation, guides, and examples for Burdin, built with [Astro](https://astro.build) and
[Starlight](https://starlight.astro.build). The site is a separate Bun project in this folder; the compiler is in the
Rust workspace at the repository root and is not touched by it.

## Running it

The site uses [Bun](https://bun.sh) as its package manager and script runner. Install Bun, then:

```console
$ cd website
$ bun install
$ bun run dev        # the site at http://localhost:4321 with live reload
$ bun run build      # the static site in dist/
$ bun run preview    # serve dist/ to look at the production build
```

`bun.lock` pins every dependency. Use `bun install --frozen-lockfile` to install exactly what it records.

## Checking the examples

Every example and every Burdin code block in the docs is compiled with the `burdin` compiler when the site is checked:

```console
$ bun run check:examples
```

The checker uses `burdin` from the `PATH`, or the binary named by the `BURDIN` environment variable:

```console
$ BURDIN=/path/to/burdin bun run check:examples
```

The checker is plain Node-compatible JavaScript that uses only built-in modules, so it runs under Bun as above.

It runs three kinds of check:

1. **Examples** in `examples/`. Each `name.bd` is run, and its standard output is compared with `name.out`. If the file
   `name.status` exists, the exit status must match it, and if `name.in` exists, it is the standard input.
2. **Snippets** in `snippets/`. Each `.bd` file must compile and run.
3. **Doc blocks.** Each ` ```burdin ` block in `src/content/docs` is compiled. A block with `test` items is run with
   `burdin test`; any other block is type-checked with `burdin check`. A block whose info string is
   ` ```burdin fragment ` is not checked. Use that for an illustration that depends on other files or that fails on
   purpose; show the compiler's real output in a ` ```text ` block next to it.

The checker runs every check, prints each failing block with the compiler's messages, and exits with a non-zero status
if any check failed.

## Publishing

CI publishes the site to GitHub Pages (`.github/workflows/website.yml`): a push to `main` builds the compiler, generates
the API reference with it, checks every example in the docs, builds the site, checks its links, and deploys. A pull
request that touches the site runs the same build and checks without deploying.

GitHub Pages serves a project site from `/<repo>/`, so the workflow sets `BURDIN_SITE_BASE=/burdin/` and the last build
step (`bun run build:pages`, which runs `scripts/apply-base.mjs`) adds that prefix to any absolute link the Markdown
processor left alone. A local build keeps the root:

```console
$ bun run build              # served from /
$ BURDIN_SITE_BASE=/burdin/ bun run build:pages   # the shape Pages publishes
$ bun run check:links        # every internal link leads to a file or an id
```

`BURDIN_SITE_URL` (default `https://ricardomilos7.github.io`) sets the site address; with a custom domain, set it and
build without a base.

## The API reference

The API pages are not written by hand: `burdin doc` generates one site per official package in `../packages/`, and
the site's `/api/` page lists them. Generate them before a build:

```console
$ bun run api-docs
$ bun run build
```

The script uses the same `burdin` as the checker (`PATH` or `BURDIN`), writes to `public/api/` (ignored by git, and
replaced on every run), and exits with a non-zero status if a package could not be documented. Use a compiler built
from this repository: an older one may not read the packages' manifests. A package without generated pages is still
listed on `/api/`, marked as missing.

The standard library is built into the compiler, so it is documented with `burdin doc --std` rather than as a package.
A compiler without that option is skipped with a note, and `/api/` links the standard library to the guide's map page
(`src/content/docs/reference/standard-library.md`) instead.

## Layout

```
astro.config.mjs        Starlight configuration: the sidebar, the search, the Burdin highlighting, the fonts
src/content/docs/       the pages: getting-started, guide, coming-from, examples, reference
src/pages/index.astro   the landing page
src/pages/api/          the API reference index (the generated sites are in public/api/)
src/components/landing/ the parts of the landing page
src/styles/theme.css    the theme (colour tokens for light and dark, headings, links, sidebar, asides)
src/styles/code-themes.mjs  the two syntax-highlighting themes, matched to the palette
examples/               complete programs, with expected output
snippets/               the program shown on the landing page
scripts/check-examples.mjs  the checker described above
scripts/build-api-docs.mjs  the API reference generator described above
public/                 static files, including the logo
```

The Burdin syntax highlighting reuses the TextMate grammar in `editors/vscode/syntaxes/`, so the site and the editor
extension always agree.

## The logo

The logo is a placeholder: `public/logo-placeholder.svg`. Replace that file with the real logo and keep the name, or
change the `logo` and `favicon` entries in `astro.config.mjs`.

## Colours and type

"Burdin" is Basque for iron, so the palette is cold metal: graphite ink on a steel-gray paper, with iron-oxide orange
(`--burdin-oxide`) kept for a few marks only: link underlines, focus rings, the current page, keywords in code, and the
rail beside a program's output. The tokens are at the top of `src/styles/theme.css`, with one set for the dark theme and
one for the light theme; Starlight's theme switch chooses between them. The code colours live in
`src/styles/code-themes.mjs` and use the same values.

Text is Schibsted Grotesk (400, 500, 700). Titles and code are Fragment Mono, which has one weight, so titles get
their weight from size and tight letter-spacing rather than bold. Both come from Fontsource, Latin subset
only, and are listed in `customCss` in `astro.config.mjs`.

After changing the code themes, delete `.astro/` and `node_modules/.astro/` before building: Astro caches rendered
Markdown, and a stale cache keeps the old colours on the docs pages.

## Error and lint pages

`scripts/build-reference-pages.mjs` splits `docs/error-codes.md` and `docs/lints.md` into one page per code at
`/errors/<code>` and `/lints/<code>`, the addresses every diagnostic links to (`URL_BASE` in
`crates/burdin_diag/src/lib.rs`). `dev`, `build`, and `build:pages` run it first; the generated pages in
`src/content/docs/errors/` and `src/content/docs/lints/` are not committed. Edit the `docs/` files, never the pages.
