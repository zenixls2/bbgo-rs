CREATE TABLE IF NOT EXISTS bbgo_checkpoints_v1 (
    key_hash BYTEA PRIMARY KEY CHECK(OCTET_LENGTH(key_hash)=32),
    key_bytes BYTEA NOT NULL CHECK(OCTET_LENGTH(key_bytes) BETWEEN 1 AND 1024),
    version BIGINT NOT NULL CHECK(version=1),
    payload BYTEA NOT NULL CHECK(OCTET_LENGTH(payload)<=4194304),
    digest BYTEA NOT NULL CHECK(OCTET_LENGTH(digest)=32)
);
