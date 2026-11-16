#!/usr/bin/env python3
"""Execute the original Go model formulas with minimal type-only scaffolding."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

source = Path(sys.argv[1] if len(sys.argv) > 1 else "../gamma-capture").resolve()
main = r'''
package main
import ("encoding/json"; "os"; "time")
type Direction int
const (DirectionUp Direction = 1; DirectionDown Direction = -1)
type CrossingEvent struct {
    ExchangeTime time.Time
    GapAffected bool
    Direction Direction
    BarrierWidth float64
}
type IntensityConfig struct {
    Window, VolatilityWindow time.Duration
    PriorAlphaUp, PriorAlphaDown, PriorBetaUp, PriorBetaDown float64
    MinEvents int
}
type Passage struct {
    Up, Down, Seconds float64
    TP, SL, Current int
    Result FirstPassage
}
type Mass struct { K int; Up, Down, Result float64 }
func main() {
    passages := []Passage{
        {Up:1, Down:1, Seconds:1, TP:1, SL:1},
        {Up:2, Down:1, Seconds:3, TP:3, SL:2, Current:1},
        {Up:0, Down:0, Seconds:1, TP:2, SL:2},
        {Up:1, Down:0, Seconds:2, TP:2, SL:2},
        {Up:0.1, Down:0.2, Seconds:0.5, TP:3, SL:3},
    }
    for i := range passages {
        p := &passages[i]
        p.Result = TPBeforeSL(p.Up, p.Down, time.Duration(p.Seconds*float64(time.Second)), p.TP, p.SL, p.Current)
    }
    masses := []Mass{}
    for k := -3; k <= 3; k++ {
        for _, pair := range [][2]float64{{0,0}, {1,2}, {10,10}} {
            masses = append(masses, Mass{K:k, Up:pair[0], Down:pair[1], Result:SkellamPMF(k,pair[0],pair[1])})
        }
    }
    model := NewIntensityModel(IntensityConfig{Window:60*time.Second, VolatilityWindow:30*time.Second,
        PriorAlphaUp:1, PriorAlphaDown:2, PriorBetaUp:1, PriorBetaDown:1, MinEvents:1})
    model.Observe(time.UnixMilli(1000),false)
    for _, item := range []struct{at int64; direction Direction}{{2000,DirectionUp},{3000,DirectionDown},{4000,DirectionUp}} {
        model.Update(CrossingEvent{ExchangeTime:time.UnixMilli(item.at), Direction:item.direction, BarrierWidth:0.01})
    }
    snapshot := model.Snapshot(time.UnixMilli(11000))
    encoder := json.NewEncoder(os.Stdout); encoder.SetIndent("", "  ")
    if err := encoder.Encode(map[string]any{"passages":passages,"masses":masses,"intensity":snapshot}); err != nil { panic(err) }
}
'''
with tempfile.TemporaryDirectory(prefix="bbgo-gamma-vectors-", dir="/tmp/opencode") as temp:
    model = Path(temp) / "model.go"
    model.write_text((source / "pkg/strategy/gammacapture/model.go").read_text().replace("package gammacapture", "package main", 1))
    path = Path(temp) / "main.go"
    path.write_text(main)
    result = subprocess.check_output(["go", "run", str(model), str(path)], env={**os.environ, "GOWORK": "off"})
    json.loads(result)
    destination = source / "rust/tests/fixtures/go-model.json"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(result)
    print(destination)
