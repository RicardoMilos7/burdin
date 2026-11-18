"""A small JSON Schema validator for the test scripts, with no dependencies (the scripts run unchanged on every host).

It supports the part of draft 2020-12 that docs/diagnostic.schema.json and tests/diagnostics/sarif-subset.schema.json
use: `$ref` into `#/$defs/...`, `type` (one name or a list), `const`, `enum`, `required`, `properties`,
`additionalProperties: false`, `items`, `minItems`, `minLength`, `minimum`, `pattern`, and `oneOf`. A keyword it does
not know raises an error instead of being ignored, so a schema cannot quietly check less than it says.

Usage:
    problems = validate(value, schema)   # a list of "path: message" strings; empty when the value is valid
"""

import re

ANNOTATIONS = {"$schema", "$id", "title", "description", "$comment", "$defs"}
KEYWORDS = {
    "$ref", "type", "const", "enum", "required", "properties", "additionalProperties", "items", "minItems",
    "minLength", "minimum", "pattern", "oneOf",
} | ANNOTATIONS


def _type_matches(value, name: str) -> bool:
    if name == "object":
        return isinstance(value, dict)
    if name == "array":
        return isinstance(value, list)
    if name == "string":
        return isinstance(value, str)
    if name == "boolean":
        return isinstance(value, bool)
    if name == "null":
        return value is None
    if name == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    raise ValueError(f"unsupported type name {name!r}")


def _resolve(root: dict, reference: str) -> dict:
    if not reference.startswith("#/"):
        raise ValueError(f"unsupported $ref {reference!r}")
    node = root
    for part in reference[2:].split("/"):
        node = node[part]
    return node


def _validate(value, schema: dict, root: dict, path: str, problems: list) -> None:
    unknown = set(schema) - KEYWORDS
    if unknown:
        raise ValueError(f"{path}: unsupported schema keywords {sorted(unknown)}")
    if "$ref" in schema:
        _validate(value, _resolve(root, schema["$ref"]), root, path, problems)
    if "type" in schema:
        names = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        if not any(_type_matches(value, name) for name in names):
            problems.append(f"{path}: expected {' or '.join(names)}, found {type(value).__name__}")
            return
    if "const" in schema and (value != schema["const"] or type(value) is not type(schema["const"])):
        problems.append(f"{path}: expected {schema['const']!r}, found {value!r}")
    if "enum" in schema and value not in schema["enum"]:
        problems.append(f"{path}: {value!r} is not one of {schema['enum']}")
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            problems.append(f"{path}: shorter than {schema['minLength']}")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            problems.append(f"{path}: {value!r} does not match {schema['pattern']}")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            problems.append(f"{path}: {value} is below {schema['minimum']}")
    if isinstance(value, dict):
        for name in schema.get("required", []):
            if name not in value:
                problems.append(f"{path}: missing required property {name!r}")
        properties = schema.get("properties", {})
        for name, item in value.items():
            if name in properties:
                _validate(item, properties[name], root, f"{path}.{name}", problems)
            elif schema.get("additionalProperties") is False:
                problems.append(f"{path}: unexpected property {name!r}")
    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            problems.append(f"{path}: fewer than {schema['minItems']} items")
        if "items" in schema:
            for index, item in enumerate(value):
                _validate(item, schema["items"], root, f"{path}[{index}]", problems)
    if "oneOf" in schema:
        attempts = []
        for option in schema["oneOf"]:
            attempt: list = []
            _validate(value, option, root, path, attempt)
            attempts.append(attempt)
        matches = sum(1 for attempt in attempts if not attempt)
        if matches == 0:
            # The closest alternative explains the failure best.
            problems.append(f"{path}: matches none of the oneOf alternatives; the closest says:")
            problems.extend("    " + line for line in min(attempts, key=len))
        elif matches > 1:
            problems.append(f"{path}: matches {matches} of the oneOf alternatives, expected exactly 1")


def validate(value, schema: dict) -> list:
    problems: list = []
    _validate(value, schema, schema, "$", problems)
    return problems
