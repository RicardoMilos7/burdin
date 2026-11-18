//! `missing_docs` (`spec/lint.md`, LN33): a public item with no `///` comment. The items are the ones `burdin doc
//! --coverage` counts (`burdin_syntax::docs::public_items`), so the lint and the report always agree: the public
//! functions, types, interfaces, aliases, constants, derives, C functions, and extension methods of a module, the
//! methods of its inherent `impl` blocks, and the methods of its public interfaces. Generated code and `main` are
//! left out, and so are `priv` items and the members of `priv` types.

use crate::{Ctx, Finding, Lint};
use burdin_syntax::docs::public_items;

pub fn check(cx: &Ctx, out: &mut Vec<Finding>) {
    for source in &cx.input.files {
        for item in public_items(source.ast, source.file, cx.text(source.file)) {
            if item.docs.is_none() {
                let message = format!("public {} `{}` has no documentation", item.kind, item.name);
                out.push(Finding::new(Lint::MissingDocs, item.span, message).note(
                    "write a `///` comment on the lines directly above it; `burdin doc --coverage` shows the rest",
                ));
            }
        }
    }
}
