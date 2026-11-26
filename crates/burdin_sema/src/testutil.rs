//! Helpers for the unit tests that need `Option` and `Result`, which are `std`'s enums, without compiling `std`.

use crate::generics::Request;
use crate::globals::Globals;
use burdin_diag::{FileId, Span};
use burdin_tir::{AdtRef, Derives, EnumDef, FieldDef, LangItem, Ty, VariantDef, VariantShape};

/// A `Globals` where `Option` (template 0) and `Result` (template 1) are the language's enums. Their instances
/// come from [`settled`] or [`Globals::test_option`] and [`Globals::test_result`].
pub(crate) fn lang_globals() -> Globals {
    Globals { lang_option: Some(0), lang_result: Some(1), ..Globals::default() }
}

/// Makes an instance for every request `g` holds, as `Sema::allocate_adt` would have.
pub(crate) fn settled(mut g: Globals) -> Globals {
    let requests: Vec<Request> = g.requests.borrow_mut().drain(..).collect();
    for request in requests {
        if let Request::Adt(template, args) = request {
            g.test_instance(template, args);
        }
    }
    g
}

impl Globals {
    /// The `Option` or `Result` instance for `args`, made if it does not exist (template 0 is `Option`).
    fn test_instance(&mut self, template: usize, args: Vec<Ty>) -> Ty {
        if let Some(&(_, id)) = self.adt_instances.get(&(template, args.clone())) {
            return Ty::Enum(self.enum_refs[id].clone());
        }
        let field = |ty: Ty| FieldDef { name: "0".into(), ty, span: Span::new(FileId(0), 0, 0), is_priv: false };
        let variant = |name: &str, fields: Vec<Ty>| VariantDef {
            name: name.into(),
            shape: if fields.is_empty() { VariantShape::Unit } else { VariantShape::Tuple },
            fields: fields.into_iter().map(field).collect(),
        };
        let (lang, variants) = if template == 0 {
            (LangItem::Option, vec![variant("None", vec![]), variant("Some", vec![args[0].clone()])])
        } else {
            (LangItem::Result, vec![variant("Ok", vec![args[0].clone()]), variant("Err", vec![args[1].clone()])])
        };
        let id = self.enums.len();
        let name = Globals::instance_name(if template == 0 { "Option" } else { "Result" }, &args);
        self.enums.push(EnumDef {
            lang: Some(lang),
            name: name.clone(),
            variants,
            derives: Derives {
                copy: true,
                clone: true,
                eq: true,
                ord: true,
                hash: true,
                debug: true,
                ..Derives::default()
            },
            drop_method: None,
        });
        let adt = AdtRef { id, name: name.as_str().into(), second_class: args.iter().any(Ty::is_second_class) };
        self.enum_refs.push(adt.clone());
        self.adt_instances.insert((template, args.clone()), (true, id));
        self.adt_origin.insert((true, id), (template, args));
        Ty::Enum(adt)
    }

    /// `Option<t>` for a test, made if it does not exist.
    pub(crate) fn test_option(&mut self, t: Ty) -> Ty {
        self.test_instance(0, vec![t])
    }

    /// `Result<ok, err>` for a test, made if it does not exist.
    pub(crate) fn test_result(&mut self, ok: Ty, err: Ty) -> Ty {
        self.test_instance(1, vec![ok, err])
    }
}
