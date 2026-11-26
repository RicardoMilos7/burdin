# Burdin data formatters for lldb (`spec/debug.md`, DBG5, DBG7).
#
# lldb has no embedded-script section, so this file is loaded explicitly:
#
#     (lldb) command script import debuggers/lldb/burdin_formatters.py
#
# `burdin debug` and `burdin dap` write the embedded copy beside the session and import it for you. The formats
# match `debuggers/gdb/burdin_printers.py`; both read the compiler's debug types by member name, and the layout
# facts the DWARF cannot carry are the ones in `spec/debug.md`, DBG5 (a `Map`'s `(key, value, hash)` slots with a
# zero hash for an empty one, a `Shared` cell of `{ strong, value }`, and the `any` vtable header).

import re

import lldb

MAX_ITEMS = 100
CATEGORY = "burdin"


# ----- reading values ---------------------------------------------------------------------------------------

def _name(valobj):
    return valobj.GetType().GetUnqualifiedType().GetName() or ""


def _member(valobj, name):
    # A value with synthetic children searches them first, so a `List`'s own `len`/`ptr` members are only
    # reachable through the value without them.
    return valobj.GetNonSyntheticValue().GetChildMemberWithName(name)


def _fields(ty):
    """The member names of a struct or union type (`GetFieldsArray` is a property, not a method, in lldb 21)."""
    if ty is None or not ty.IsValid():
        return []
    return [ty.GetFieldAtIndex(i).GetName() for i in range(ty.GetNumberOfFields())]


def _int(valobj, name):
    child = _member(valobj, name)
    if child is None or not child.IsValid():
        return 0
    text = child.GetValue()
    if text is None:
        return 0
    # A `bool` member reads as `true`/`false` rather than a number.
    if text == "true":
        return 1
    if text == "false":
        return 0
    try:
        return int(text, 0)
    except ValueError:
        return 0


def _read(addr, length):
    if addr == 0 or length <= 0:
        return b""
    process = lldb.debugger.GetSelectedTarget().GetProcess()
    error = lldb.SBError()
    data = process.ReadMemory(addr, length, error)
    return data if error.Success() else b""


def _text(valobj):
    return _read(_int(valobj, "ptr"), _int(valobj, "len"))


def _rendered(data):
    return data.decode("utf-8", "backslashreplace")


def _quoted(data):
    text = _rendered(data)
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


def _elements(valobj, pointer, count):
    """The first `count` values behind `pointer`, read by address.

    lldb does not enumerate the elements of a bare pointer through `GetChildAtIndex` (a `List`'s buffer, a
    `Map`'s slot array), so each element is a value made from its address and the pointee type.
    """
    if count <= 0:
        return []
    ty = pointer.GetType().GetPointeeType()
    if not ty.IsValid():
        ty = pointer.Dereference().GetType()
    size = ty.GetByteSize()
    try:
        base = int(pointer.GetValue() or "0", 0)
    except (TypeError, ValueError):
        return []
    if base == 0 or size <= 0:
        return []
    return [pointer.CreateValueFromAddress(f"[{i}]", base + i * size, ty) for i in range(count)]


def _render(valobj):
    if valobj is None or not valobj.IsValid():
        return "?"
    summary = valobj.GetSummary()
    if summary:
        return summary
    value = valobj.GetValue()
    if value is not None:
        return value
    # A plain struct has neither: show its members, as gdb does.
    plain = valobj.GetNonSyntheticValue()
    if plain.GetNumChildren() > 0:
        inner = ", ".join(f"{plain.GetChildAtIndex(i).GetName()} = {_render(plain.GetChildAtIndex(i))}"
                          for i in range(plain.GetNumChildren()))
        return "{" + inner + "}"
    return "?"


# ----- summaries --------------------------------------------------------------------------------------------

def string_summary(valobj, internal_dict):
    try:
        return _quoted(_text(valobj))
    except Exception:
        return None


def list_summary(valobj, internal_dict):
    try:
        length = _int(valobj, "len")
        elements = _elements(valobj, _member(valobj, "ptr"), min(length, MAX_ITEMS))
        items = [_render(element) for element in elements]
        if length > MAX_ITEMS:
            items.append("...")
        return "{" + ", ".join(items) + "}"
    except Exception:
        return None


def map_summary(valobj, internal_dict):
    try:
        slots = _elements(valobj, _member(valobj, "ptr"), _int(valobj, "cap"))
        pairs = []
        for slot in slots:
            if _int(slot, "hash") != 0:
                pairs.append(f"{_render(_member(slot, 'key'))}: {_render(_member(slot, 'value'))}")
                if len(pairs) >= MAX_ITEMS:
                    pairs.append("...")
                    break
        return "{" + ", ".join(pairs) + "}"
    except Exception:
        return None


def _lang_enum_summary(valobj):
    """`Option` and `Result` are enums of `std` (`{ tag, payload }`), shown as `Some(5)` and `Ok(7)`."""
    try:
        payload = _member(valobj, "payload")
        variants = _fields(payload.GetType())
        tag = _int(valobj, "tag")
        if tag < 0 or tag >= len(variants):
            return f"<variant {tag}>"
        member = payload.GetChildMemberWithName(variants[tag])
        fields = _fields(member.GetType())
        if not fields:
            return variants[tag]
        return f"{variants[tag]}({_render(member.GetChildMemberWithName(fields[0]))})"
    except Exception:
        return None


def option_summary(valobj, internal_dict):
    return _lang_enum_summary(valobj)


def result_summary(valobj, internal_dict):
    return _lang_enum_summary(valobj)


def enum_summary(valobj, internal_dict):
    # Every Burdin enum is `{ tag: i32, payload: union }`. lldb has no way for a summary to decline a type —
    # returning None prints the literal `None` — but an empty summary falls back to the value's fields and
    # children, which is how the catch-all registration below leaves structs and the other printers' types alone.
    try:
        payload = _member(valobj, "payload")
        tag_member = _member(valobj, "tag")
        if payload is None or not payload.IsValid() or tag_member is None or not tag_member.IsValid():
            return ""
        variants = _fields(payload.GetType())
        if not variants:
            return ""
        tag = _int(valobj, "tag")
        if tag < 0 or tag >= len(variants):
            return f"<variant {tag}>"
        variant = variants[tag]
        member = payload.GetChildMemberWithName(variant)
        if member is None or not member.IsValid():
            return variant
        fields = _fields(member.GetType())
        if not fields:
            return variant
        inner = ", ".join(f"{name} = {_render(member.GetChildMemberWithName(name))}" for name in fields)
        return f"{variant} {{ {inner} }}"
    except Exception:
        return ""


def heap_summary(valobj, internal_dict):
    try:
        pointer = _member(valobj, "value")
        if not pointer.IsValid() or int(pointer.GetValue() or "0", 0) == 0:
            return "Heap(null)"
        return f"Heap({_render(pointer.Dereference())})"
    except Exception:
        return None


def shared_summary(valobj, internal_dict):
    try:
        cell = _member(valobj, "cell")
        if not cell.IsValid():
            return "Shared(null)"
        cell = cell.Dereference()
        return f"Shared({_render(_member(cell, 'value'))}) (strong {_int(cell, 'strong')})"
    except Exception:
        return None


def _vtable_name(valobj):
    """The concrete type's name from the vtable at `{ i64 type_id, ptr drop, ptr name, usize name_len }`."""
    vtable = _int(valobj, "vtable")
    if vtable == 0:
        return None
    header = _read(vtable + 16, 8)
    if len(header) != 8:
        return None
    name_ptr = int.from_bytes(header, "little")
    length_bytes = _read(vtable + 24, 8)
    if len(length_bytes) != 8:
        return None
    length = int.from_bytes(length_bytes, "little")
    data = _read(name_ptr, length)
    return data.decode("utf-8", "replace") if data else None


def any_summary(valobj, internal_dict):
    try:
        name = _vtable_name(valobj)
        if not name:
            return "any <unknown>"
        concrete = _concrete(valobj, name)
        if concrete is None:
            return f"any {name}"
        return f"any {name} = {_render(concrete)}"
    except Exception:
        return None


def _concrete(valobj, name):
    """The boxed value, read through a typed pointer at `data`, when the concrete type is in the debug info."""
    target = lldb.debugger.GetSelectedTarget()
    ty = target.FindFirstType(name)
    if not ty.IsValid():
        return None
    data = _member(valobj, "data")
    if data is None or not data.IsValid():
        return None
    cast = data.Cast(ty.GetPointerType())
    return cast.Dereference() if cast.IsValid() else None


def closure_summary(valobj, internal_dict):
    return "closure { code, env, drop }"


def range_summary(valobj, internal_dict):
    try:
        name = _name(valobj)
        op = "..=" if "RangeInclusive" in name else ".."
        return f"{_render(_member(valobj, 'lo'))}{op}{_render(_member(valobj, 'hi'))}"
    except Exception:
        return None


# ----- children ---------------------------------------------------------------------------------------------

class ListProvider:
    def __init__(self, valobj, internal_dict):
        self.valobj = valobj
        self.elements = []

    def _load(self):
        length = min(_int(self.valobj, "len"), MAX_ITEMS)
        self.elements = _elements(self.valobj, _member(self.valobj, "ptr"), length)

    def num_children(self):
        self._load()
        return len(self.elements)

    def get_child_index(self, name):
        try:
            return int(name.lstrip("[").rstrip("]"))
        except ValueError:
            return -1

    def get_child_at_index(self, index):
        if not self.elements:
            self._load()
        if index < 0 or index >= len(self.elements):
            return None
        return self.elements[index]


class MapProvider:
    def __init__(self, valobj, internal_dict):
        self.valobj = valobj
        self.live = []

    def _collect(self):
        slots = _elements(self.valobj, _member(self.valobj, "ptr"), _int(self.valobj, "cap"))
        self.live = []
        for slot in slots:
            if _int(slot, "hash") != 0:
                self.live.append(slot)
                if len(self.live) >= MAX_ITEMS:
                    break

    def num_children(self):
        self._collect()
        return len(self.live)

    def get_child_index(self, name):
        return -1

    def get_child_at_index(self, index):
        if not self.live:
            self._collect()
        if index < 0 or index >= len(self.live):
            return None
        return self.live[index]


class EnumProvider:
    def __init__(self, valobj, internal_dict):
        self.valobj = valobj
        self.fields = []

    def _active(self):
        payload = _member(self.valobj, "payload")
        if payload is None or not payload.IsValid():
            return None
        variants = _fields(payload.GetType())
        tag = _int(self.valobj, "tag")
        if tag < 0 or tag >= len(variants):
            return None
        member = payload.GetChildMemberWithName(variants[tag])
        if member is None or not member.IsValid():
            return None
        self.fields = _fields(member.GetType())
        return member

    def num_children(self):
        member = self._active()
        return len(self.fields) if member is not None else 0

    def get_child_index(self, name):
        try:
            return self.fields.index(name)
        except ValueError:
            return -1

    def get_child_at_index(self, index):
        member = self._active()
        if member is None or index < 0 or index >= len(self.fields):
            return None
        return member.GetChildMemberWithName(self.fields[index])


# ----- registration -----------------------------------------------------------------------------------------

# lldb applies the last matching summary, so the enum catch-all — which declines any type without the
# `{ tag, payload }` shape — is registered first and the specific summaries override it.
_SUMMARIES = [
    (r".*", "burdin_formatters.enum_summary"),
    (r"^String$", "burdin_formatters.string_summary"),
    (r"^str$", "burdin_formatters.string_summary"),
    (r"^List<.*>$", "burdin_formatters.list_summary"),
    (r"^Map<.*>$", "burdin_formatters.map_summary"),
    (r"^(std\.)?Option<.*>$", "burdin_formatters.option_summary"),
    (r"^(std\.)?Result<.*>$", "burdin_formatters.result_summary"),
    (r"^Heap<.*>$", "burdin_formatters.heap_summary"),
    (r"^Shared<.*>$", "burdin_formatters.shared_summary"),
    (r"^any .*$", "burdin_formatters.any_summary"),
    (r"^Range(Inclusive)?<.*>$", "burdin_formatters.range_summary"),
    (r"^Fn\(.*$", "burdin_formatters.closure_summary"),
]


def __lldb_init_module(debugger, internal_dict):
    # lldb 19+ has no Python API for type categories, so the formatters are installed through the same `type`
    # commands a user would run, into a category of their own (`type category disable burdin` turns them off).
    debugger.HandleCommand(f"type category define --enabled {CATEGORY}")
    for pattern, function in _SUMMARIES:
        debugger.HandleCommand(
            f'type summary add --category {CATEGORY} --python-function {function} --regex "{pattern}"'
        )
    for pattern, provider in (
        (r"^List<.*>$", "burdin_formatters.ListProvider"),
        (r"^Map<.*>$", "burdin_formatters.MapProvider"),
    ):
        debugger.HandleCommand(
            f'type synthetic add --category {CATEGORY} --python-class {provider} --regex "{pattern}"'
        )
    debugger.HandleCommand(f"type category enable {CATEGORY}")
