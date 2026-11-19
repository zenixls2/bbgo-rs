#!/usr/bin/env python3
"""Offline tests; do not require an external Go checkout."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("audit", ROOT / "tools/audit-go-build-tags.py")
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)

class BuildTagTests(unittest.TestCase):
    def test_checked_inventory_classifies_every_constraint(self):
        data = json.loads((ROOT / "docs/go-build-tags.json").read_text())
        self.assertEqual(data["decisions"], audit.DECISIONS)
        self.assertGreater(len(data["files"]), 20)
        self.assertTrue(any("dnum" in item["tags"] for item in data["files"]))
        for item in data["files"]:
            self.assertTrue(set(item["tags"]) <= audit.DECISIONS.keys())

    def test_unknown_tag_cannot_be_silently_ignored(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="bbgo-tag-test-") as temp:
            root = Path(temp)
            source = root / "flags.go"
            source.write_text("//go:build dnum && future_numeric\npackage flags\n")
            with self.assertRaisesRegex(ValueError, "future_numeric"):
                audit.inventory(root)
            source.write_text("//go:build !dnum\n// +build !dnum\npackage flags\n")
            result = audit.inventory(root)
            self.assertEqual(result["files"][0]["tags"], ["dnum"])
            self.assertEqual(len(result["files"][0]["constraints"]), 2)

if __name__ == "__main__":
    unittest.main()
