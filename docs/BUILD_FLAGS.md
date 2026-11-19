# Build-flag migration coverage

Audited source: sibling `~/src/bbgo` at
`a135236d38d10b56f5f1bacbd0306de9e8059040`. The complete inspected constraint
inventory is [`go-build-tags.json`](go-build-tags.json): **58 Go files**.
This inventory covers explicit `//go:build` / `// +build` constraints, not every
Go compiler flag, filename platform suffix, or all GOOS/GOARCH combinations.

| Go tag | Rust handling | Coverage / remaining scope |
| --- | --- | --- |
| `dnum`, `!dnum` | `Decimal` trait, concrete `Dnum` / `Legacy`, additive `dnum` feature changes `Value` | Both selected backends tested across workspace; original Go-tag-generated numeric fixtures; native SQL matrix |
| `release`, `!release` | Cargo dev / release profiles | Both optimization profiles tested; **not** parity with Go's version/CLI package, which is not migrated |
| `debug` | No matching feature yet | Go tri-strategy debug hooks belong to an unmigrated strategy; Rust `debug_assertions` is not the same behavior |
| `web` | Not migrated | Embedded backtest/server UI assets; no fake no-op feature |
| `pprof` | Not migrated | Go runtime HTTP profiler has no direct Rust runtime equivalent; profiling adapter/server not implemented |
| `race`, `!race` | Go toolchain-specific | Rust concurrency tests exist, but no claim of Go race-detector or Rust sanitizer parity |
| `go1.18`, `!go1.18` | One Rust implementation + declared MSRV | Go compatibility shims do not require two Rust implementations; minimum Rust versions have not separately been verified |
| `unix` | Not migrated collector executable | Reusable libraries/test harnesses do not port Go collector signal handlers or establish Windows compatibility |
| `ignore` | Excluded upstream programs | Research/generators are not installed runtime components; Rust fixture generators are separate tools |

The unimplemented rows are **migration gaps**, not successful builds of those
upstream capabilities. There are intentionally no `web`, `pprof`, `race`, etc.
Cargo features that misleadingly compile without providing the feature.

## Reproduce and detect constraint drift

```sh
python3 tools/audit-go-build-tags.py ../bbgo --check
# After reviewing a source update and classifying any new tags:
python3 tools/audit-go-build-tags.py ../bbgo
python3 scripts/test_build_tags.py
```

The audit fails on any unknown tag or (with `--check`) changed files/constraints.
Offline CI validates the checked-in inventory and unknown-tag rejection without
downloading another repository. To detect upstream changes, run the audit against
that updated source checkout; offline CI cannot discover an unseen upstream commit.

## Rust build matrix

CI tests and strict Clippy run all four combinations:

```sh
cargo test --workspace --all-targets --locked
cargo test --workspace --all-targets --locked --features dnum
cargo test --workspace --all-targets --locked --release
cargo test --workspace --all-targets --locked --release --features dnum
```

The SQL service job additionally runs MySQL 8 / PostgreSQL 16 collector and
checkpoint contracts for both numeric selections:

```sh
cargo test-sql-swarm
cargo test-sql-swarm --features dnum
cargo test-sql-local --tools-root /path/to/packages/root --cargo-test-args --features dnum
```

For downstream applications, declare `features = ["dnum"]` on the shared package
you depend on (types/core/replay/backtest/binance/persistence/model), or on
`bbgo-fixedpoint` directly. Forward it from your application's feature if desired.
Features are additive and unified, not runtime numeric switches.
