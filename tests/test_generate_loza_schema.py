"""Regression tests for scripts/generate_loza_schema.py logical type inference."""
from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "generate_loza_schema.py"
_spec = importlib.util.spec_from_file_location("generate_loza_schema", SCRIPT)
generate_loza_schema = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(generate_loza_schema)

_logical_type = generate_loza_schema._logical_type


class LogicalTypeTests(unittest.TestCase):
    def test_one_of_variant_resolving_to_dynamic_is_allowed(self):
        # A nested oneOf with more than one distinct variant resolves to "dynamic".
        # The outer oneOf must accept that variant instead of rejecting it.
        schema = {
            "oneOf": [
                {"type": "string"},
                {"oneOf": [{"type": "integer"}, {"type": "number"}]},
            ]
        }
        self.assertEqual(_logical_type(schema), "dynamic")

    def test_one_of_all_dynamic_variants_is_allowed(self):
        schema = {
            "oneOf": [
                {"oneOf": [{"type": "integer"}, {"type": "number"}]},
                {"oneOf": [{"type": "boolean"}, {"type": "string"}]},
            ]
        }
        self.assertEqual(_logical_type(schema), "dynamic")

    def test_single_variant_still_resolves_directly(self):
        schema = {"oneOf": [{"type": "string"}, {"type": "string"}]}
        self.assertEqual(_logical_type(schema), "string")

    def test_unsupported_one_of_variant_is_still_rejected(self):
        # A variant with no representable type must remain an error.
        schema = {"oneOf": [{"type": "string"}, {}]}
        with self.assertRaises(ValueError):
            _logical_type(schema)
    def test_duckdb_must_be_a_mapping(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "defaults.yaml"
            for value in ("null", "[]"):
                path.write_text(f"duckdb: {value}\n", encoding="utf-8")
                with self.subTest(value=value), self.assertRaisesRegex(ValueError, "duckdb must be a mapping"):
                    generate_loza_schema._physical_config(path)

if __name__ == "__main__":
    unittest.main()
