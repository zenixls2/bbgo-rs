mod common;
use bbgo_core::CheckpointStore;
use bbgo_persistence::SqlCheckpointStore;
use sha2::{Digest, Sha256};
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};

#[tokio::test]
async fn sqlite_memory_contract_and_explicit_schema() {
    let store = SqlCheckpointStore::connect("sqlite::memory:")
        .await
        .unwrap();
    assert!(store.load("before-init").await.is_err());
    store.initialize().await.unwrap();
    store.initialize().await.unwrap();
    common::contract(&store).await;
    store.close().await;
    assert!(store.load("key").await.is_err());
    assert!(
        SqlCheckpointStore::connect("https://invalid")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sqlite_reopen_corruption_and_atomic_readers() {
    static ID: AtomicU64 = AtomicU64::new(0);
    fs::create_dir_all("/tmp/opencode").unwrap();
    let path = format!(
        "/tmp/opencode/bbgo-store-{}-{}.sqlite3",
        std::process::id(),
        ID.fetch_add(1, Ordering::Relaxed)
    );
    // Do not adopt any pre-existing file, including a symlink.
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .unwrap();
    let url = format!("sqlite://{path}");
    let store = SqlCheckpointStore::connect(&url).await.unwrap();
    store.initialize().await.unwrap();
    store.save("saved", b"persistent").await.unwrap();
    store.close().await;
    let first = SqlCheckpointStore::connect(&url).await.unwrap();
    let second = SqlCheckpointStore::connect(&url).await.unwrap();
    assert_eq!(first.load("saved").await.unwrap().unwrap(), b"persistent");
    first.save("atomic", &[1; 16384]).await.unwrap();
    let writer = async {
        for index in 0..30 {
            first.save("atomic", &vec![index; 16384]).await.unwrap();
        }
    };
    let reader = async {
        for _ in 0..30 {
            let value = second.load("atomic").await.unwrap().unwrap();
            assert_eq!(value.len(), 16384);
            assert!(value.iter().all(|byte| *byte == value[0]));
        }
    };
    tokio::join!(writer, reader);
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let hash = Sha256::digest(b"saved").to_vec();
    sqlx::query("UPDATE bbgo_checkpoints_v1 SET payload=? WHERE key_hash=?")
        .bind(b"tampered".as_slice())
        .bind(hash.clone())
        .execute(&pool)
        .await
        .unwrap();
    assert!(first.load("saved").await.is_err());
    first.save("saved", b"repaired").await.unwrap();
    sqlx::query("UPDATE bbgo_checkpoints_v1 SET key_bytes=? WHERE key_hash=?")
        .bind(b"other".as_slice())
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
    assert!(first.load("saved").await.is_err());
    first.close().await;
    second.close().await;
    pool.close().await;
    fs::remove_file(&path).unwrap();
}
