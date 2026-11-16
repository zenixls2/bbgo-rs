#!/usr/bin/env python3
"""Build a synthetic collector-wire fixture using the *actual* Go schema DDL.

No captured account data or CSV is read. Run from bbgo-rs; sibling bbgo is
required only for regeneration, never compilation or ordinary tests.
"""
import hashlib
import pathlib
import sqlite3

root = pathlib.Path(__file__).resolve().parents[1]
schema = (root.parent / "bbgo/cmd/gammacapture-capture/capture_schema_v1.sql").read_bytes()
out = root / "crates/bbgo-backtest/tests/fixtures/capture-v1.sqlite3"
out.parent.mkdir(parents=True, exist_ok=True)
if out.exists():
    out.unlink()
db = sqlite3.connect(out)
db.executescript(schema.decode())
db.execute("INSERT INTO schema_migrations VALUES(1,1,?)", (hashlib.sha256(schema).hexdigest(),))
db.execute("INSERT INTO capture_meta VALUES('symbol','BTCJPY')")
for run in ("run-1", "run-2"):
    db.execute("INSERT INTO capture_writer_runs(writer_run_id,symbol,started_at_ns,ended_at_ns,status) VALUES(?,'BTCJPY',1,9000000000,'closed')", (run,))
    for stream in ("bookticker", "trades"):
        source = f"{run}:{stream}"
        db.execute("""INSERT INTO capture_sources(source_id,symbol,stream,source_kind,writer_run_id,locator,
            source_schema_version,completeness_status,created_at_ns) VALUES(?,'BTCJPY',?,'live_ws',?,?,'capture-csv-v1','validated',1)""",
            (source, stream, run, f"fixture://{source}"))

# Deliberately insert a trade before a same-nanosecond BBO. Replay orders BBO
# first, but refuses to infer that a new quote reaction can fill on that trade.
events = [
    ("trades",     1_000_000_100, "run-1", 0, "10"),
    ("bookticker", 1_000_000_100, "run-1", 0, None),
    ("trades",     1_000_000_101, "run-1", 0, "11"),
    ("trades",     1_000_000_102, "run-1", 0, "12"),
    ("bookticker", 1_002_000_000, "run-2", 1, None),
    ("trades",     1_003_000_000, "run-2", 1, "13"),
    ("bookticker", 7_003_000_000, "run-2", 6000, None),
]
for stream, at, run, gap, aggregate in events:
    exchange = at - 100 if stream == "trades" else None
    seq = db.execute("""INSERT INTO capture_events(symbol,stream,writer_run_id,source_id,event_time_ns,
        received_at_ns,processing_at_ns,ingested_at_ns,gap_before_ms,gap_clock,source_schema_version)
        VALUES('BTCJPY',?,?,?,?,?,?,?,?,'local_receive','capture-csv-v1')""",
        (stream, run, f"{run}:{stream}", exchange, at, at + 1, at + 2, gap)).lastrowid
    db.execute("INSERT INTO capture_event_availability VALUES(?,'capture_service',?,'collector_callback_receive','observed')", (seq, at))
    if stream == "bookticker":
        db.execute("INSERT INTO capture_bookticker VALUES(?,'99','2','101','3')", (seq,))
    else:
        db.execute("INSERT INTO capture_trades VALUES(?,?,'99','0.4','SELL',?,NULL,NULL)", (seq, str(100 + seq), aggregate))
db.commit()
db.close()
print(out)
