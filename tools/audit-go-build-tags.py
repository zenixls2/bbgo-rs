#!/usr/bin/env python3
"""Inventory Go build constraints; fail if a new tag has no migration decision."""
import argparse
import json
from pathlib import Path
import re

DECISIONS = {
    "dnum": "implemented: Decimal trait + Legacy/Dnum; Cargo feature dnum; numeric CI matrix",
    "release": "partial: Cargo dev/release profiles tested; Go version/CLI package not migrated",
    "debug": "not migrated: tri strategy debug hooks (not a numeric backend flag)",
    "web": "not migrated: embedded backtest/server web assets and UI",
    "pprof": "not migrated: Go runtime profiling HTTP endpoint; no no-op Rust feature",
    "race": "Go toolchain race detector selection; Rust concurrency tests are not detector parity",
    "go1.18": "Go language-version compatibility shims; one Rust implementation, MSRV declared separately",
    "unix": "not migrated: collector process signal handlers; reusable Rust libraries have no collector binary",
    "ignore": "excluded upstream: code generators and research files; not shipped runtime functionality",
}
ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "docs/go-build-tags.json"

def inventory(source):
    records = []
    for path in sorted(source.rglob("*.go")):
        if any(part in (".git", "vendor", "node_modules") for part in path.relative_to(source).parts):
            continue
        lines = path.read_text(errors="strict").splitlines()
        constraints = [
            line for line in lines[:40]
            if line.startswith("//go:build ") or line.startswith("// +build ")
        ]
        if not constraints:
            continue
        tags = set()
        for line in constraints:
            expression = line.split("build ", 1)[1]
            tags.update(re.findall(r"[A-Za-z_][A-Za-z_0-9.]*", expression))
        unknown = tags - DECISIONS.keys()
        if unknown:
            raise ValueError(f"{path}: unclassified build tags {sorted(unknown)}")
        records.append({
            "path": str(path.relative_to(source)),
            "constraints": constraints,
            "tags": sorted(tags),
        })
    return {"decisions": DECISIONS, "files": records}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, nargs="?", default=ROOT.parent / "bbgo")
    parser.add_argument("--check", action="store_true", help="reject drift from checked-in inventory")
    args = parser.parse_args()
    if not (args.source / "pkg/fixedpoint/dec.go").is_file():
        parser.error("expected a BBGO source checkout")
    result = inventory(args.source.resolve())
    if args.check:
        if result != json.loads(OUTPUT.read_text()):
            raise SystemExit("Go build constraints changed: review and regenerate docs/go-build-tags.json")
    else:
        OUTPUT.write_text(json.dumps(result, indent=2) + "\n")
    print(f"{len(result['files'])} build-constrained Go files; all tags classified")

if __name__ == "__main__":
    main()
