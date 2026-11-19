# bbgo-fixedpoint

Two concrete **finite** decimal backends implement `Decimal`:

| | `Legacy` | `Dnum` |
| --- | --- | --- |
| Representation | signed i64 / 100,000,000 | sign × 16-digit u64 coefficient × 10^(exponent − 16) |
| Default `Value` alias | yes | `--features dnum` |
| Range | i64 raw units | checked normalized exponent −128…127 |
| Ordinary mul/div | i128 intermediates, truncate toward zero at eight decimals | Go dnum split-coefficient multiplication, u128 division, digit-normalization rounding |
| Parsing | strict decimal, at most eight nonzero fractional places | scientific/percent syntax and leading decimals; `FromStr` truncates after 16 significant digits as Go does |
| Wire/serde parsing | exact, strings only | **exact**, strings only; excess significant digits rejected |

`pub type Value` is compile-time selected with `cfg(feature = "dnum")`. Domain,
execution, persistence and model integration crates forward that feature; enabling
it on any dependency unifies the selection throughout that Cargo dependency graph.
Do not expect `--no-default-features` elsewhere to undo another dependency's `dnum`.
There is no runtime arithmetic dispatch or boxed numeric trait.

```rust
use bbgo_fixedpoint::{Decimal, Dnum, Error, Legacy};

fn notional<N: Decimal>(price: N, quantity: N) -> Result<N, Error> {
    price.checked_mul(quantity)
}

let eight = notional::<Legacy>("100".parse()?, "0.01".parse()?)?;
let sixteen = notional::<Dnum>("100".parse()?, "0.00000000001".parse()?)?;
# Ok::<(), Error>(())
```

Both types remain available together even when the alias changes. Backend-specific
raw units are deliberately **not** part of `Decimal`: `Legacy::raw/from_raw`
retain their original API, while `Dnum::coefficient/exponent` expose its different
representation. `TryFrom` conversions between the backends are exact or fail;
they never silently round. `Decimal::BACKEND` / `Value::BACKEND` identifies the
semantics for saved state/configuration.

## Arithmetic and safety differences

- Both reject division by zero, nonfinite inputs and overflow. Go's infinity
  sentinels, silent underflow, wrapping and all legacy float-conversion paths
  are **not** reproduced. Legacy scientific/percent parsing remains unsupported.
- Dnum add/sub align/truncate the smaller coefficient before addition.
  Multiplication deliberately preserves Go's omitted low×low term, not an
  idealized arbitrary-precision decimal result. Normalization rounds discarded
  digits half up, one digit at a time; division truncates its coefficient first.
  Arithmetic is not associative; switching backends changes numerical results.
- Dnum's checked exponent range is deliberately bounded to its parser's finite
  supported range, even though Go arithmetic can construct larger exponents.
  Very small results fail instead of silently becoming zero. Arithmetic-produced
  supported values serialize and deserialize exactly.
- `Dnum::from_integer` follows coefficient normalization and can round integers
  longer than 16 digits; use `parse_exact` or the exact `TryFrom<Legacy>` conversion
  when lossless input is required. JSON deserialization, Binance and collector SQL
  use exact parsing. `to_f64` is lossy modeling conversion, not accounting.
- `checked_mul_ceil` is an accounting extension: nonnegative products round UP to
  the selected backend's precision. `checked_mul_div` allocates with one final
  truncation, avoiding a rounded or overflowing intermediate decimal.
- `floor_to` uses mathematical floor, including negatives. If the exact Dnum
  grid point cannot fit 16 digits it fails, rather than rounding above the floor.
  These accounting extensions use exact bounded BigInt intermediates for Dnum;
  its ordinary add/mul/div use native integers. Generic code is statically
  dispatched, **not a promise that every operation is allocation-free**.
- Replay checkpoints record backend identity; old unlabelled journals are
  legacy-only. Cross-backend recovery is rejected, even for equal decimal strings.
  Other checkpoint participants must include backend identity in their own
  state descriptor/configuration digest; there is no global automatic migration.
  Independent decimal strings or generic SQL blobs have no implicit backend label.

## Verification

```sh
cargo test -p bbgo-fixedpoint --locked
cargo test --workspace --all-targets --locked --features dnum
python3 tools/generate-fixedpoint-fixtures.py ../bbgo
```

The generator copies the original `convert.go`, `dec.go`, `div128.go` into an
isolated temporary Go module and executes actual default / `-tags dnum` selection.
The checked-in default fixture covers safe finite common arithmetic; dnum covers
1,007 operand pairs (1,000 deterministic randomized) and parsing/formatting edge
cases. Both concrete types run those tests regardless of the selected alias.
This is substantial numeric coverage, not proof of every upstream method or input.
Go's formatter masks, all float constructors, infinities, and its entire decimal
utility API are not migrated.
