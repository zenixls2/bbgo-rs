#!/usr/bin/env python3
"""Run the original Go functions without importing BBGO's unrelated dependencies."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

source = Path(sys.argv[1] if len(sys.argv) > 1 else "../gamma-capture").resolve()
root = Path(__file__).resolve().parents[1]
main = r'''
package main
import ("encoding/json"; "os")
type Vector struct {
    Input string `json:"input"`
    FNV32 uint32 `json:"fnv32"`
    FNV64 uint64 `json:"fnv64"`
    Mask *string `json:"mask,omitempty"`
    Budget int `json:"budget"`
    Chunks []string `json:"chunks"`
}
func main() {
    out := []Vector{}
    for _, s := range []string{"", "a", "ab", "abc", "abcdef", "hello", "0123456789abcdefghijkl", "BTCJPY", "秘密鍵", "a秘密b", "🙂abc"} {
        v := Vector{Input:s, FNV32:FNV32(s), FNV64:FNV64(s), Budget:4, Chunks:StringSplitByLength(s,4)}
        ascii := true
        for _, b := range []byte(s) { if b >= 128 { ascii = false } }
        if ascii { masked := MaskKey(s); v.Mask = &masked }
        out = append(out, v)
    }
    encoder := json.NewEncoder(os.Stdout); encoder.SetIndent("", "  ")
    if err := encoder.Encode(out); err != nil { panic(err) }
}
'''
with tempfile.TemporaryDirectory(prefix="bbgo-go-vectors-", dir="/tmp/opencode") as temp:
    files = []
    for name in ("fnv.go", "string.go"):
        content = (source / "pkg/util" / name).read_text()
        path = Path(temp) / name
        path.write_text(content.replace("package util", "package main", 1))
        files.append(str(path))
    path = Path(temp) / "main.go"
    path.write_text(main)
    result = subprocess.check_output(["go", "run", *files, str(path)], env={**os.environ, "GOWORK": "off"})
    json.loads(result)
    destination = root / "crates/bbgo-util/tests/fixtures/go-util.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(result)
    print(destination)
