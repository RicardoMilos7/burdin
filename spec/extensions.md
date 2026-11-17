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

Extension methods are not values: they are not in the module's value namespace, so `shout(x)` is E0002, and there is no
associated-function form (`Type.name()` with no receiver).

## EX2: Visibility and imports

An extension is visible in the module that declares it and in every module that imports it **by name**:

```burdin
use text.shout          // one extension name
use text.{shout, loud}  // several
```

The imported name is the method name; every declaration in the target module with that name comes along (one per
receiver type). This follows the ordinary item rules: an imported extension is not re-exported, `pub use` re-exports
it, and a `priv` extension cannot be imported from another package (E0150). An extension may be imported under a name
its target module does not declare only if that is a duplicate, which is E0007; importing an extension `as another`
name is E0216, because a call always uses the declared method name.

Importing a module (`use text`) or another item of it (`use text.other`) does **not** make its extensions visible. A
dependency's extension is reachable only through a `use` that names it (or through a `pub use` in the module that
imports it).

## EX3: Resolution order

For `x.f(args)` the checker tries, in order:

1. the methods defined with the type (its impl blocks, including the methods an interface implementation provides);
2. the methods the compiler provides for the type (`String.len`, `List.push`, `clone`, `hash`, the iterator
   protocol's methods, `to_string` through `Display`, ...);
3. the extension methods visible in the calling module whose receiver type applies to `x`;
4. otherwise, E0002: no method `f` on type `x`.

The receiver is auto-dereferenced before the lookup, exactly as for a method call. An extension's receiver applies
when the receiver's type matches the declared pattern and every type parameter that occurs in the pattern is bound;
for `fn List<T>.largest`, `List<i32>` applies and `Map<i32, i32>` does not. A call to an extension that is not visible
in the module therefore reports the ordinary "no method" E0002.

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
- Importing two different extension declarations under one name into a module (from two modules, or a declaration
  and an import) is E0007, like any duplicate import.
- A declaration without `self` first, with type arguments on an intermediate receiver segment, with a `where`
  predicate naming a non-parameter, or imported under another name is E0216.
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

An extension is not callable by its name alone and has no associated form. There is no value of a "method" type, no
way to take a reference to an extension, and no way to pass one where a closure is expected.

## Tests

| Rule | Tests |
| --- | --- |
| EX1 declaration, receiver types, generics | `m18_extension/string_extension.bd`, `user_type_extension.bd`, `enum_extension.bd`, `concrete_receiver_args.bd`, `generic_receiver.bd`, `generic_user_type.bd`, `method_type_params.bd`, `missing_self.bd`, `where_not_parameter.bd`, `duplicate_receiver_arg.bd` |
| EX2 visibility and imports | `m18_extension/imported_extension/`, `not_imported/`, `aliased_import/`, `duplicate_import/`, `private_extension/`, `imported_type_extension/`, `reexported_extension/` |
| EX3 resolution order | `m18_extension/shadowed_method.bd`, `shadowed_builtin_method.bd`, `shadowed_interface_method.bd`, `receiver_mismatch.bd`, `not_callable_by_name.bd` |
| EX4 shadowing warning | `m18_extension/shadowed_method.bd`, `shadowed_builtin_method.bd`, `shadowed_interface_method.bd` |
| EX5 conflicts | `m18_extension/same_name_different_receivers.bd`, `duplicate_extension.bd`, `duplicate_receiver_arg.bd`, `duplicate_import/`, `missing_self.bd`, `where_not_parameter.bd`, `aliased_import/` |
| EX6 calls and conventions | `m18_extension/mut_receiver.bd`, `own_receiver_drop.bd`, `mut_requires_mutable.bd`, `view_of_self.bd`, `view_of_temporary.bd`, `generic_body_calls_extension.bd`, `unsafe_extension.bd`, `unsafe_extension_outside.bd` |
| EX7 code generation | `m18_extension/own_receiver_drop.bd`, `generic_receiver.bd`, `generic_user_type.bd` |
| EX8 not a function value | `m18_extension/not_callable_by_name.bd` |
