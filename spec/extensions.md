# Extension methods (milestone 18)

An extension method is a function with a receiver that is called with dot syntax on a value whose type it names,
without an impl block and without ownership of the type:

```burdin
fn String.to_user(self): User { ... }
fn String.shout(mut self) { ... }
fn List<T>.sum(self): T where T: Copy + Ord { ... }

"alice".to_user()
```

Extension methods are ordinary functions whose first parameter is `self`. The compiler resolves a call to one only
after the receiver type's own methods, and code generation emits it like any other function. The rules below are
normative; `tests/m18_extension/` is their executable form.

## EX1: Declaration

```
[ "priv" ] [ "unsafe" ] "fn" ( TypePath | SliceType ) "." IDENT [ GenericParams ] "(" ParamList ")" [ ":" Type ] [ WhereClause ] Block
```

An extension is declared at module scope. The text before the last `.` is the receiver type, a type path resolved in
the declaring module: a local struct or enum, an imported type, or a built-in type (`String`, `str`, an integer,
`List<T>`, `Option<T>`, ...). A slice receiver is written as the slice type: `fn [T].first(self)` declares an
extension on every slice, and `fn [[T]].concat(self)` one on slices of slices. The text after the last `.` is the
method name.

- `self` must be the first parameter. `self`, `mut self`, and `own self` mean what they mean on a method: a read-only
  receiver is borrowed, `mut self` borrows the caller's place exclusively, and `own self` takes the value. A
  declaration without `self` first is E0216.
- Type arguments written on the receiver are the extension's type parameters when they are ordinary names that no
  type in scope binds: `fn Pair<T>.first(self)` has one parameter `T`, and `fn Option<i32>.or_zero(self)` has none
  (its receiver is exactly `Option<i32>`). Writing a parameter twice is E0007.
- A `<...>` list after the method name declares method-level type parameters, as on a generic method:
  `fn Holder.pair<U>(self, other: U)`. Their inline bounds follow the ordinary rules.
- Bounds on any parameter come from the method's inline `<...>` bounds and from `where` predicates
  (`fn List<T>.largest(self): T where T: Copy + Ord`). A `where` predicate that names something other than one of the
  extension's type parameters is E0216.
- An extension may be `unsafe fn`; calling it requires an `unsafe` block or an `unsafe fn`. `priv` makes the
  extension's name package-private, like every other declaration.
- Type arguments on a path segment other than the receiver's last one are E0216 (a module path cannot be generic).

Extension methods are not values: they are not in the module's value namespace, so `shout(x)` is E0002. An extension
has an associated form, `Type.shout(x)`, which calls it with its receiver as the first argument (EX9).

## EX2: Visibility and imports

An extension is visible in the module that declares it. Another module gets it in one of three ways:

```burdin
use text                  // every extension text declares, or re-exports with `pub use`
use text.shout            // one extension, by its method name
use text.{shout, loud}    // several
use text.shout as yell    // one extension, called `yell` in this module
```

- **`use text`** (also `use text as t`) brings every extension that `text` declares, and every one it re-exports with
  `pub use`. It is not an error for `text` to have none. An extension `text` only imports from elsewhere is not
  brought: imports are not re-exported, as for every other item. A `priv` extension of another package is not brought,
  and that is not an error (its method simply is not visible); a `priv` extension of the same package is.
- **`use text.shout`** brings every declaration with that name, one per receiver type, under its own method name.
  `pub use` re-exports it.
- **`use text.shout as yell`** brings the same declarations under the method name `yell`. Calls in this module use
  `yell`; `shout` is not visible through this import, so `x.shout()` is E0002. `pub use text.shout as yell` re-exports
  the alias. A `priv` extension of another package is E0150 for an item import, as for any item.
- Importing a module does not import its other items, and `pub use text` re-exports the module name only: the
  extensions `text` brings are re-exported only by a `pub use text.shout`.
- Within one module a method name stands for one extension declaration per receiver type. Two different declarations
  under one name in one module, whether declared there or brought by an import (`use text`, `use text.shout`, or
  `use text.shout as shout`), are E0007 at the `use` that brings the second one. Bringing the same declaration twice
  (`use text` and `use text.shout`) is not an error.
- The standard library's extensions are visible in every module (`spec/stdlib.md`). A method name the module declares
  or brings itself hides the standard library's extension of that name, as before.

A dependency's extension is reachable from another package only through a `use` that names it, or through a `use` of
the dependency's root module that the root re-exports (`pub use`).

## EX3: Resolution order

For `x.f(args)` the checker tries, in order:

1. the methods defined with the type (its impl blocks, including the methods an interface implementation provides);
2. the methods the compiler provides for the type (`String.len`, `List.push`, `clone`, `hash`, the iterator
   protocol's methods, `to_string` through `Display`, ...);
3. the extension methods visible in the calling module whose receiver type applies to `x`;
4. otherwise, E0002: no method `f` on type `x`.

The receiver is auto-dereferenced before the lookup, exactly as for a method call. An extension's receiver applies
when the receiver's type matches the declared pattern and every type parameter that occurs in the pattern is bound;
for `fn List<T>.largest`, `List<i32>` applies and `Map<i32, i32>` does not. Two built-in receiver fallbacks apply
after the receiver's own methods and extensions: an array `[T; N]` borrows as a slice `[T]` for slice methods and
slice extensions, and a `String` borrows as a `str` for read-only `str` extensions (issue #1019), so `std.text`
declares its read-only text helpers once on `str` and `String`'s own methods and extensions still win first. A call
to an extension that is not visible in the module therefore reports the ordinary "no method" E0002.

The lookup stays local to the calling module: it only sees the extensions that are in that module's extension
namespace. No whole-program search is done.

## EX4: Shadowing (E0215)

When an extension's receiver type already has a method of the same name — declared by an impl block, provided by an
interface implementation, or provided by the compiler for a built-in type — the method wins at every call, and the
extension can never be reached through dot syntax. The compiler reports a warning, E0215, **at the extension's
declaration** (at its method name). The program still compiles and runs, and the warning does not fail a build.

The check is done once the receiver type's methods are known, so it also catches a method the type gained after the
extension was written. A generic receiver is checked against the receivers it resolves to; its methods are the ones
the type's impl blocks declare.

## EX5: Conflicts and invalid declarations

- Two extensions in one module may share a name when their receiver types differ (`fn String.label` and
  `fn Gadget.label`). A second declaration for the same receiver type — after parameter renaming, so
  `fn Pair<T>.m` and `fn Pair<U>.m` collide — is E0007 at the second declaration.
- Two different extension declarations under one method name in one module (from two modules, or a declaration and
  an import, a module import included) are E0007 at the `use` that brings the second one (EX2).
- A declaration without `self` first, with type arguments on an intermediate receiver segment, or with a `where`
  predicate naming a non-parameter is E0216.
- An extension whose receiver type does not resolve reports the ordinary unknown-type diagnostic (E0002/E0003).

## EX6: Calls and conventions

The receiver follows the method conventions exactly:

- A read-only `self` is borrowed for the call, so the receiver stays usable and a returned view may derive from it
  (RT1-RT4). Storing a view derived from a temporary is E0123 (RT5).
- `mut self` borrows the receiver exclusively; the receiver must be a mutable place (E0113), and the ordinary
  exclusivity and borrow rules apply to the call as a whole (EX1).
- `own self` moves the receiver; using it afterwards is E0100.
- The other arguments follow the ordinary function rules: `mut`/`own` conventions with their call-site markers,
  defaults, named arguments (AG1-AG7), and exclusivity between the receiver and the arguments.
- An unsafe extension needs an `unsafe` context (E0161).
- Generic arguments are inferred first from the receiver's type, then from the other arguments in parameter order,
  and the bounds are checked at the call (E0141). Inside a generic body whose receiver is a type parameter, the call
  is checked against the extension's bounds and needs no instance.

## EX7: Code generation

An extension method becomes an ordinary Burdin function whose first parameter is the receiver:

- `self` is passed by address, `mut self` by address and exclusively, `own self` by value, exactly like a method's
  receiver.
- The function's internal name is module-qualified and marked as an extension
  (`<module>.ext.<Receiver>.<name>`, and the instance name adds the type arguments), so it can never collide with a
  method of the same type and name.
- A generic extension is monomorphized per receiver/argument instantiation, like a generic function. A non-generic
  one is emitted once.
- Reachability, drop flags, drop glue, and clone glue are the ordinary ones for a function with these parameters: the
  receiver is dropped where its convention says, and `own self` drops the value inside the extension.

## EX8: Not a function value

An extension is not callable by its name alone. There is no value of a "method" type, no way to take a reference to
an extension, and no way to pass one where a closure is expected. `shout(s)` is E0002; the associated form of EX9 is a
call with a type in front, not a value.

## EX9: Associated form

An extension is also called with its receiver type in front, as an associated function:

```burdin
String.shout(s)          // the same call as s.shout()
Point.manhattan(p)
List<i32>.total(xs)
i32.doubled(21)
```

- `Type.name(recv, args...)` calls the extension `name` whose receiver applies to `Type` (EX3), passing the first
  argument as the receiver and the others as the remaining arguments. It is checked as `recv.name(args...)`: the
  receiver's argument is checked against the extension's receiver, so `Point.manhattan(s)` with a `String` `s` is a
  type mismatch (E0003).
- The receiver follows the receiver conventions of a method call: it takes no call-site marker (`Counter.bump(c)`; a
  `mut` marker on it is E0115), and a `mut self` extension needs a mutable place (E0113).
- `Type` is written in full: a type name, a built-in type, or a generic type with its arguments (`List<i32>`). The
  form is not a template: a generic type written without its arguments is not a receiver here.
- Resolution: the type's own functions come first. An associated function or variant of `Type` with the name (an
  impl function, a compiler-provided associated function such as `String.from`, or a variant) is called, and the
  extension is tried only when `Type` has none. Without a receiver argument (`String.shout()`), or when no visible
  extension applies to `Type`, the call is E0002: no function or variant `name` on `Type`.
- The extension must be visible in the calling module (EX2). The rules of EX6 apply to the call as they do to
  `recv.name(...)`.

## Tests

| Rule | Tests |
| --- | --- |
| EX1 declaration, receiver types, generics | `m18_extension/string_extension.bd`, `user_type_extension.bd`, `enum_extension.bd`, `concrete_receiver_args.bd`, `generic_receiver.bd`, `generic_user_type.bd`, `method_type_params.bd`, `missing_self.bd`, `where_not_parameter.bd`, `duplicate_receiver_arg.bd` |
| EX2 visibility and imports | `m18_extension/imported_extension/`, `module_import/`, `module_import_same_declaration/`, `module_import_collision/`, `module_import_private/`, `module_reexport/`, `not_imported/`, `aliased_import/`, `alias_hides_declared_name/`, `alias_collision/`, `alias_shadowed/`, `duplicate_import/`, `private_extension/`, `imported_type_extension/`, `reexported_extension/` |
| EX3 resolution order | `m18_extension/shadowed_method.bd`, `shadowed_builtin_method.bd`, `shadowed_interface_method.bd`, `receiver_mismatch.bd`, `receiver_concrete_arg_mismatch.bd`, `receiver_allocator_mismatch.bd`, `not_callable_by_name.bd` |
| EX4 shadowing warning | `m18_extension/shadowed_method.bd`, `shadowed_builtin_method.bd`, `shadowed_interface_method.bd` |
| EX5 conflicts | `m18_extension/same_name_different_receivers.bd`, `duplicate_extension.bd`, `duplicate_receiver_arg.bd`, `duplicate_import/`, `module_import_collision/`, `alias_collision/`, `missing_self.bd`, `where_not_parameter.bd` |
| EX6 calls and conventions | `m18_extension/mut_receiver.bd`, `own_receiver_drop.bd`, `mut_requires_mutable.bd`, `view_of_self.bd`, `view_of_temporary.bd`, `generic_body_calls_extension.bd`, `unsafe_extension.bd`, `unsafe_extension_outside.bd` |
| EX7 code generation | `m18_extension/own_receiver_drop.bd`, `generic_receiver.bd`, `generic_user_type.bd` |
| EX8 not a function value | `m18_extension/not_callable_by_name.bd` |
| EX9 associated form | `m18_extension/assoc_call.bd`, `assoc_call_shadowed.bd`, `assoc_call_no_receiver.bd`, `assoc_call_wrong_receiver.bd`, `assoc_call_marker.bd`, `assoc_call_not_mutable.bd`, `assoc_call_not_visible/` |
