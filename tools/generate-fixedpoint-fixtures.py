#!/usr/bin/env python3
"""Execute BBGO's actual default/dnum build selections, using only Go stdlib.

Never edits the source checkout. Arithmetic vectors stay finite/nonzero-divisor;
unsupported infinities/legacy floating-path differences are documented separately.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

source = Path(sys.argv[1] if len(sys.argv) > 1 else "../bbgo").resolve()
root = Path(__file__).resolve().parents[1]
main = r'''
package main
import ("encoding/json"; "os"; "fmt"; "math/rand")
type ParseVector struct { Input string `json:"input"`; Output string `json:"output"` }
type Op struct { A string `json:"a"`; B string `json:"b"`; Add string `json:"add"`; Sub string `json:"sub"`; Mul string `json:"mul"`; Div string `json:"div"` }
func main() {
    parses := []ParseVector{}
    inputs := []string{"0","1","-1","0.1","123.456","0.00000001","-1000"}
    if backend == "dnum" { inputs = append(inputs, "1.00000000000000111", "1.1e-15","1e-100","1e100","12.5%","0.000000000000000000001000111", "0.0009763593380614657") }
    for _, input := range inputs { parses=append(parses, ParseVector{input,MustNewFromString(input).String()}) }
    pairs := [][2]string{{"0.1","0.2"},{"123.5","2"},{"-100","4"},{"1","3"},{"1000","0.001"},{"0.00000001","1"},{"-0.5","-2"}}
    if backend == "dnum" {
        r:=rand.New(rand.NewSource(42))
        for i:=0;i<1000;i++ {
            a:=fmt.Sprintf("%de%d",1000000000000000+r.Int63n(8999999999999999),r.Intn(61)-45)
            b:=fmt.Sprintf("%de%d",1000000000000000+r.Int63n(8999999999999999),r.Intn(61)-45)
            if i%2==0 {a="-"+a}; if i%3==0 {b="-"+b}
            pairs=append(pairs,[2]string{a,b})
        }
    }
    ops:=[]Op{}
    for _, p:=range pairs {
        a,b:=MustNewFromString(p[0]),MustNewFromString(p[1])
        ops=append(ops,Op{p[0],p[1],a.Add(b).String(),a.Sub(b).String(),a.Mul(b).String(),a.Div(b).String()})
    }
    enc:=json.NewEncoder(os.Stdout); enc.SetIndent("","  ")
    if err:=enc.Encode(map[string]interface{}{"backend":backend,"parse":parses,"arithmetic":ops});err!=nil {panic(err)}
}
'''
with tempfile.TemporaryDirectory(prefix="bbgo-fixedpoint-go-", dir="/tmp/opencode") as temp:
    directory = Path(temp)
    (directory / "go.mod").write_text("module bbgo-fixedpoint-vectors\n\ngo 1.22\n")
    for name in ("convert.go", "dec.go", "div128.go"):
        content = (source / "pkg/fixedpoint" / name).read_text()
        (directory / name).write_text(content.replace("package fixedpoint", "package main", 1))
    (directory / "main.go").write_text(main)
    for backend in ("legacy", "dnum"):
        (directory / "selection.go").write_text(f'package main\nconst backend = "{backend}"\n')
        flags = ["-tags", "dnum"] if backend == "dnum" else []
        result = subprocess.check_output(
            ["go", "run", *flags, "."], cwd=directory,
            env={**os.environ, "GOWORK": "off"},
        )
        json.loads(result)
        destination = root / f"crates/bbgo-fixedpoint/tests/fixtures/go-{backend}.json"
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(result)
        print(destination)
