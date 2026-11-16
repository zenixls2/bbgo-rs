-- Requires MySQL >=8.0.16 (CHECK enforcement and window functions).
-- Native relational contract, not SQLite DDL. Use an explicit migration account;
-- replay should connect with SELECT-only privileges and verified TLS remotely.
CREATE TABLE capture_meta (`key` VARCHAR(64) PRIMARY KEY, value TEXT NOT NULL)
 ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
INSERT INTO capture_meta VALUES ('schema_contract','bbgo-capture-v1');
CREATE TABLE capture_writer_runs (
 writer_run_id VARCHAR(128) PRIMARY KEY, symbol VARCHAR(32) NOT NULL,
 started_at_ns BIGINT NOT NULL, ended_at_ns BIGINT,
 status VARCHAR(16) NOT NULL CHECK(status IN ('running','closed','failed','unknown'))
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE TABLE capture_sources (
 source_id VARCHAR(512) PRIMARY KEY, symbol VARCHAR(32) NOT NULL,
 stream VARCHAR(16) NOT NULL CHECK(stream IN ('bookticker','trades')),
 source_kind VARCHAR(32) NOT NULL, writer_run_id VARCHAR(128),
 locator TEXT NOT NULL, source_schema_version VARCHAR(64) NOT NULL,
 completeness_status VARCHAR(32) NOT NULL, created_at_ns BIGINT NOT NULL,
 FOREIGN KEY(writer_run_id) REFERENCES capture_writer_runs(writer_run_id)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE TABLE capture_events (
 ingest_seq BIGINT AUTO_INCREMENT PRIMARY KEY, symbol VARCHAR(32) NOT NULL,
 stream VARCHAR(16) NOT NULL CHECK(stream IN ('bookticker','trades')),
 writer_run_id VARCHAR(128), source_id VARCHAR(512) NOT NULL,
 event_time_ns BIGINT, received_at_ns BIGINT, processing_at_ns BIGINT, ingested_at_ns BIGINT NOT NULL,
 gap_before_ms BIGINT CHECK(gap_before_ms>=0), gap_clock VARCHAR(32),
 source_schema_version VARCHAR(64) NOT NULL,
 FOREIGN KEY(writer_run_id) REFERENCES capture_writer_runs(writer_run_id),
 FOREIGN KEY(source_id) REFERENCES capture_sources(source_id),
 INDEX capture_events_stream_sequence(stream,ingest_seq)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE TABLE capture_event_availability (
 ingest_seq BIGINT NOT NULL, consumer_id VARCHAR(64) NOT NULL,
 available_at_ns BIGINT, availability_basis VARCHAR(64) NOT NULL, status VARCHAR(32) NOT NULL,
 PRIMARY KEY(ingest_seq,consumer_id), FOREIGN KEY(ingest_seq) REFERENCES capture_events(ingest_seq),
 INDEX capture_availability_clock(consumer_id,available_at_ns,ingest_seq)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE TABLE capture_bookticker (
 ingest_seq BIGINT PRIMARY KEY, bid_price TEXT NOT NULL, bid_quantity TEXT NOT NULL,
 ask_price TEXT NOT NULL, ask_quantity TEXT NOT NULL,
 FOREIGN KEY(ingest_seq) REFERENCES capture_events(ingest_seq)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
CREATE TABLE capture_trades (
 ingest_seq BIGINT PRIMARY KEY, trade_id_text TEXT NOT NULL, price TEXT NOT NULL, quantity TEXT NOT NULL,
 side VARCHAR(4) NOT NULL CHECK(side IN ('BUY','SELL')), aggregate_id_text TEXT,
 first_trade_id_text TEXT, last_trade_id_text TEXT,
 FOREIGN KEY(ingest_seq) REFERENCES capture_events(ingest_seq)
) ENGINE=InnoDB DEFAULT CHARACTER SET utf8mb4 COLLATE utf8mb4_bin;
