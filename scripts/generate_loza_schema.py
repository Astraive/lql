#!/usr/bin/env python3
"""Generate the checked-in LQL schema from the canonical LOZA event contract."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

import yaml


TYPE_MAP = {
    "boolean": "bool",
    "integer": "int",
    "number": "float",
    "string": "string",
    "object": "object",
    "array": "array<dynamic>",
}


def _logical_type(schema: dict[str, Any]) -> str:
    types = schema.get("type")
    if isinstance(types, list):
        non_null = [item for item in types if item != "null"]
        types = non_null[0] if len(non_null) == 1 else None
    if isinstance(types, str):
        if types not in TYPE_MAP:
            raise ValueError(f"canonical field uses unsupported JSON type {types!r}")
        return TYPE_MAP[types]
    one_of = schema.get("oneOf")
    if isinstance(one_of, list):
        variants = {_logical_type(item) for item in one_of if isinstance(item, dict)}
        if len(variants) == 1:
            return variants.pop()
        if variants and variants <= set(TYPE_MAP.values()):
            return "dynamic"
        raise ValueError("canonical field has an unsupported oneOf type")
    if schema.get("properties") is not None or schema.get("additionalProperties") is not None:
        return "object"
    raise ValueError("canonical field has no representable JSON type")


def _flatten(properties: dict[str, Any], required: set[str], prefix: str = "") -> list[dict[str, Any]]:
    fields: list[dict[str, Any]] = []
    for name in sorted(properties):
        spec = properties[name]
        if not isinstance(spec, dict):
            continue
        path = f"{prefix}.{name}" if prefix else name
        field_type = _logical_type(spec)
        fields.append(
            {
                "name": path,
                "field_type": field_type,
                "nullable": name not in required,
                "sensitivity": spec.get("x-loza-sensitivity", "public"),
                "description": spec.get("description", ""),
                "structured": field_type == "object",
            }
        )
        nested = spec.get("properties")
        if isinstance(nested, dict):
            nested_required = set(spec.get("required", []))
            fields.extend(_flatten(nested, nested_required, path))
    return fields


def _physical_config(defaults_path: Path) -> dict[str, Any]:
    config = yaml.safe_load(defaults_path.read_text(encoding="utf-8")) or {}
    duckdb = config.get("duckdb", {})
    schema = duckdb.get("schema", {})
    column_types = duckdb.get("column_types", {})
    if not isinstance(schema, dict) or not isinstance(column_types, dict):
        raise ValueError("duckdb.schema and duckdb.column_types must be mappings")
    return {"table": duckdb.get("table", "events"), "raw": duckdb.get("raw_column", "raw"), "projections": schema, "column_types": column_types}


def generate(loza_spec: Path, defaults_path: Path) -> dict[str, Any]:
    event_path = loza_spec / "schemas" / "json" / "event.schema.json"
    if not event_path.is_file():
        raise FileNotFoundError(f"missing canonical event schema: {event_path}")
    event_text = event_path.read_text(encoding="utf-8")
    event = json.loads(event_text)
    if not isinstance(event.get("properties"), dict):
        raise ValueError("canonical event schema must define object properties")

    physical = _physical_config(defaults_path)
    fields = _flatten(event["properties"], set(event.get("required", [])))
    field_names = {field["name"] for field in fields}
    projections: dict[str, dict[str, Any]] = {}
    for physical_name, logical_path in physical["projections"].items():
        if not isinstance(logical_path, str) or logical_path not in field_names:
            raise ValueError(f"Collector projection {physical_name!r} does not map to a canonical event field")
        projections[logical_path] = {
            "column": physical_name,
            "type": physical["column_types"].get(physical_name, "VARCHAR"),
        }

    for field in fields:
        mapping = projections.get(field["name"])
        field["physical"] = {
            "source": physical["table"],
            "column": mapping["column"] if mapping else physical["raw"],
            "storage": "projection" if mapping else "raw",
        }

    return {
        "schema_version": "v1",
        "source_revision": event.get("$id", "v1"),
        "source_content_sha256": hashlib.sha256(event_text.encode("utf-8")).hexdigest(),
        "sources": {
            "events": {
                "physical": physical["table"],
                "row_identity": "event_id",
                "fields": fields,
            }
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--loza-spec", type=Path, required=True)
    parser.add_argument("--defaults", type=Path)
    parser.add_argument("--output", type=Path, default=Path("schemas/loza-v1.json"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    loza_spec = args.loza_spec.resolve()
    defaults = (args.defaults or loza_spec.parent / "collector" / "loza-collector.defaults.yaml").resolve()
    content = json.dumps(generate(loza_spec, defaults), indent=2, sort_keys=True) + "\n"
    output = args.output.resolve()
    if args.check:
        if not output.is_file() or output.read_text(encoding="utf-8") != content:
            print(f"stale generated schema: {output}")
            return 1
        print(f"✓ {output} is current")
        return 0
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(content, encoding="utf-8")
    print(f"generated {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
