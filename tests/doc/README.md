# `burdin doc` fixtures

`tests/check_doc.py` documents these inputs and compares the result with the files under `expect/`.

| Path | What it covers |
| --- | --- |
| `geometry/` | A library package: a root module with `pub use`, shapes with operators, derives, an interface, an error enum, a generic type with instance `impl` blocks, extension methods, private items, and every kind of intra-doc link. Its site, its JSON, and its coverage report are the goldens in `expect/`. |
| `links/broken.bd` | Intra-doc links: every form that resolves, and the ones that do not (`//~ D0001`). |
| `links/private.bd` | A link to a `priv` item (`//~ D0002`), which `--private` makes work. |
| `safety/escape.bd` | Markup and `javascript:` addresses in a doc comment stay text. |
| `errors/type_error.bd` | A package that does not compile is not documented. |

After a deliberate change to the output, run `python3 tests/check_doc.py --update` and review the diff of `expect/`.
