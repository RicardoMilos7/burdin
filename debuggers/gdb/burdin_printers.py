# Burdin pretty-printers for gdb (`spec/debug.md`, DBG5-DBG6).
#
# `burdin build -g` embeds this file in an inlined Python `.debug_gdb_scripts` entry, so gdb loads it with the
# program and no installation is needed. It can also be loaded by hand:
#
#     (gdb) command script import debuggers/gdb/burdin_printers.py
#
# The printers read the debug types the compiler emits (`emit/debug.rs`) *by member name*, never by a hard-coded
# offset, so a layout change moves them only if the names move. The layout facts that the DWARF cannot carry are
# written once in `spec/debug.md`, DBG5, and mirrored here and in `debuggers/lldb/burdin_formatters.py`:
#
#   * a `Map` buffer holds `cap` (a power of two) slots of `(key, value, u64 hash)`; a zero hash is an empty slot,
#     and there are no tombstones;
#   * a `Shared` cell is `{ strong: usize, value }` and the `Shared<T>` value points at it;
#   * an `any I` box is `{ data: ptr, vtable: ptr }`, and the vtable is
#     `{ i64 type_id, ptr drop, ptr name, usize name_len, ptr method_0, ... }`.

import re

import gdb
import gdb.printing

# The most elements a printer renders inline; the rest is left to `set print elements` and expansion.
MAX_ITEMS = 100


def _strip(val):
    return val.type.strip_typedefs()


def _type_name(val):
    try:
        return _strip(val).name or ""
    except gdb.error:
        return ""


def _text(val, ptr_member="ptr", len_member="len"):
    """The bytes of a `str`, `String`, or buffer: the `ptr`/`len` members read through the inferior's memory."""
    length = int(val[len_member])
    if length == 0:
        return b""
    address = int(val[ptr_member])
    if address == 0:
        return b""
    return gdb.selected_inferior().read_memory(address, length).tobytes()


def _quoted(data):
    text = data.decode("utf-8", "backslashreplace")
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n") + '"'


def _render(val):
    """One value as the user sees it: through its own pretty-printer when it has one."""
    try:
        return str(val)
    except gdb.error:
        return "<?>"


def _items(values):
    shown = []
    for index, value in enumerate(values):
        if index >= MAX_ITEMS:
            shown.append("...")
            break
        shown.append(_render(value))
    return "{" + ", ".join(shown) + "}"


def _pointer_cast(val, gdb_type):
    return gdb.Value(int(val)).cast(gdb_type)


class StringPrinter:
    def __init__(self, val):
        self.val = val

    def to_string(self):
        # The `string` display hint makes gdb quote and escape the text itself.
        return _text(self.val).decode("utf-8", "backslashreplace")

    def display_hint(self):
        return "string"


class ListPrinter:
    def __init__(self, val):
        self.val = val

    def _length(self):
        return int(self.val["len"])

    def to_string(self):
        # The `array` display hint renders the children as `{a, b, ...}`; there is no summary of its own.
        return None

    def children(self):
        length = self._length()
        for i in range(min(length, MAX_ITEMS)):
            yield str(i), self.val["ptr"][i]

    def display_hint(self):
        return "array"


class MapPrinter:
    def __init__(self, val):
        self.val = val

    def _slots(self):
        """The live `(key, value)` pairs: every slot whose hash is not zero (`spec/debug.md`, DBG5)."""
        cap = int(self.val["cap"])
        slots = self.val["ptr"]
        for i in range(cap):
            slot = slots[i]
            if int(slot["hash"]) != 0:
                yield slot["key"], slot["value"]

    def to_string(self):
        pairs = []
        for index, (key, value) in enumerate(self._slots()):
            if index >= MAX_ITEMS:
                pairs.append("...")
                break
            pairs.append(f"{_render(key)}: {_render(value)}")
        return "{" + ", ".join(pairs) + "}"

    def children(self):
        for index, (key, value) in enumerate(self._slots()):
            yield f"[{index}].key", key
            yield f"[{index}].value", value

    def display_hint(self):
        return "map"


class _LangEnumPrinter:
    """`Option` and `Result` are enums of `std` (`{ tag, payload }`), shown as `Some(5)` and `Ok(7)`: the active
    variant's name and its one field, `0`."""

    def __init__(self, val):
        self.val = val

    def _active(self):
        variants = _variants(self.val)
        tag = int(self.val["tag"])
        if tag < 0 or tag >= len(variants):
            return f"<variant {tag}>", None
        member = self.val["payload"][variants[tag]]
        fields = [f.name for f in _strip(member).fields()]
        return variants[tag], (member[fields[0]] if fields else None)

    def to_string(self):
        name, value = self._active()
        if value is None:
            return name
        return f"{name}({_render(value)})"

    def children(self):
        name, value = self._active()
        if value is not None:
            yield "value", value

    def display_hint(self):
        return "array"


OptionPrinter = ResultPrinter = _LangEnumPrinter


class EnumPrinter:
    """The active variant from the tag, and its payload from the union member with that name (DBG5)."""

    def __init__(self, val, variants):
        self.val = val
        self.variants = variants

    def _active(self):
        tag = int(self.val["tag"])
        if tag < 0 or tag >= len(self.variants):
            return f"<variant {tag}>", None
        name = self.variants[tag]
        member = self.val["payload"][name]
        return name, member

    def to_string(self):
        name, member = self._active()
        if member is None:
            return f"<variant {name}>"
        fields = [f.name for f in _strip(member).fields()]
        if not fields:
            return name
        inner = ", ".join(f"{f} = {_render(member[f])}" for f in fields)
        return f"{name} {{ {inner} }}"

    def children(self):
        name, member = self._active()
        if member is None:
            return
        for field in _strip(member).fields():
            yield field.name, member[field.name]


class HeapPrinter:
    def __init__(self, val):
        self.val = val

    def _pointee(self):
        pointer = self.val["value"]
        if int(pointer) == 0:
            return None
        return pointer.dereference()

    def to_string(self):
        pointee = self._pointee()
        if pointee is None:
            return "Heap(null)"
        return f"Heap({_render(pointee)})"

    def children(self):
        pointee = self._pointee()
        if pointee is not None:
            yield "value", pointee


class SharedPrinter:
    def __init__(self, val):
        self.val = val

    def _cell(self):
        cell = self.val["cell"]
        if int(cell) == 0:
            return None
        return cell.dereference()

    def to_string(self):
        cell = self._cell()
        if cell is None:
            return "Shared(null)"
        return f"Shared({_render(cell['value'])}) (strong {int(cell['strong'])})"

    def children(self):
        cell = self._cell()
        if cell is None:
            return
        yield "strong_count", cell["strong"]
        yield "value", cell["value"]


def _lookup_type(name: str):
    """A Burdin type by its name. The DWARF says `DW_LANG_C`, so gdb wants a struct's tag spelling."""
    name = name.strip()
    for spelling in (name, f"struct {name}", f"union {name}", f"enum {name}"):
        try:
            return gdb.lookup_type(spelling)
        except gdb.error:
            continue
    return None


class AnyPrinter:
    """`any I` (DBG5): the concrete type's name from the vtable, then the boxed value when its type is known."""

    def __init__(self, val):
        self.val = val

    def _vtable_name(self):
        vtable = int(self.val["vtable"])
        if vtable == 0:
            return None
        usize = gdb.lookup_type("usize")
        char_ptr = gdb.lookup_type("char").pointer()
        name_ptr = gdb.Value(vtable + 16).cast(char_ptr.pointer().pointer())  # ptr name at byte 16
        name_len = gdb.Value(vtable + 24).cast(usize.pointer()).dereference()
        address = int(name_ptr.dereference())
        length = int(name_len)
        if address == 0 or length == 0:
            return None
        return gdb.selected_inferior().read_memory(address, length).tobytes().decode("utf-8", "replace")

    def _concrete(self):
        try:
            name = self._vtable_name()
            if not name:
                return None, None
            concrete = _lookup_type(name)
            if concrete is None:
                return name, None
            data = int(self.val["data"])
            return name, gdb.Value(data).cast(concrete.pointer()).dereference()
        except (gdb.error, RuntimeError):
            return None, None

    def to_string(self):
        name, concrete = self._concrete()
        if name is None:
            return "any <unknown>"
        if concrete is None:
            return f"any {name}"
        return f"any {name} = {_render(concrete)}"

    def children(self):
        _, concrete = self._concrete()
        if concrete is not None:
            for field in _strip(concrete).fields():
                yield field.name, concrete[field.name]


class ClosurePrinter:
    def __init__(self, val):
        self.val = val

    def to_string(self):
        return "closure { code, env, drop }"

    def children(self):
        for member in ("code", "env", "drop"):
            yield member, self.val[member]


class RangePrinter:
    def __init__(self, val, inclusive):
        self.val = val
        self.inclusive = inclusive

    def to_string(self):
        op = "..=" if self.inclusive else ".."
        return f"{_render(self.val['lo'])}{op}{_render(self.val['hi'])}"


_ENUM_FIELDS = ["tag", "payload"]
_CLOSURE_FIELDS = ["code", "env", "drop"]
_LANG_ENUM = re.compile(r"^(std\.)?(Option|Result)<.*>$")
_RANGE = re.compile(r"^Range(Inclusive)?<.*>$")


def _variants(val):
    try:
        payload = _strip(val)["payload"].type
        return [f.name for f in payload.fields()]
    except (gdb.error, KeyError):
        return []


def burdin_lookup(val):
    name = _type_name(val)
    if not name:
        return None
    try:
        if name in ("String", "str"):
            return StringPrinter(val)
        if name.startswith("List<"):
            return ListPrinter(val)
        if name.startswith("Map<"):
            return MapPrinter(val)
        if _LANG_ENUM.match(name) and _strip(val).code == gdb.TYPE_CODE_STRUCT:
            return OptionPrinter(val)
        if name.startswith("Heap<"):
            return HeapPrinter(val)
        if name.startswith("Shared<"):
            return SharedPrinter(val)
        if name.startswith("any "):
            return AnyPrinter(val)
        if _RANGE.match(name):
            return RangePrinter(val, "Inclusive" in name)
        tag = _strip(val)
        if tag.code == gdb.TYPE_CODE_STRUCT:
            fields = [f.name for f in tag.fields()]
            if fields == _ENUM_FIELDS:
                return EnumPrinter(val, _variants(val))
            if fields == _CLOSURE_FIELDS:
                return ClosurePrinter(val)
    except gdb.error:
        return None
    return None


def register():
    gdb.printing.register_pretty_printer(gdb.current_objfile(), burdin_lookup, replace=True)


register()
