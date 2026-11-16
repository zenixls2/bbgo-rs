CREATE TABLE IF NOT EXISTS bbgo_checkpoints_v1 (
    key_hash BLOB PRIMARY KEY NOT NULL CHECK(length(key_hash)=32),
    key_bytes BLOB NOT NULL CHECK(length(key_bytes) BETWEEN 1 AND 1024),
    version BIGINT NOT NULL CHECK(version=1),
    payload BLOB NOT NULL CHECK(length(payload)<=4194304),
    digest BLOB NOT NULL CHECK(length(digest)=32)
);
