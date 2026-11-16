use bbgo_core::CheckpointStore;
use bbgo_persistence::{MAX_PAYLOAD, SqlCheckpointStore};

pub async fn contract(store: &SqlCheckpointStore) {
    assert_eq!(store.load("absent").await.unwrap(), None);
    assert!(store.load("").await.is_err());
    assert!(store.save("", b"x").await.is_err());
    assert!(store.save(&"a".repeat(1025), b"x").await.is_err());
    assert!(
        store
            .save("large", &vec![0; MAX_PAYLOAD + 1])
            .await
            .is_err()
    );
    let key = "gamma/日本/../state-v3";
    store.save(key, b"").await.unwrap();
    assert_eq!(store.load(key).await.unwrap().unwrap(), b"");
    store.save(key, &[0, 255, 1, 2]).await.unwrap();
    assert_eq!(store.load(key).await.unwrap().unwrap(), [0, 255, 1, 2]);
    // Exact supported boundary, including binary zeros.
    let payload = vec![17; MAX_PAYLOAD];
    store.save("boundary", &payload).await.unwrap();
    assert_eq!(store.load("boundary").await.unwrap().unwrap(), payload);
    // Invalid replacement must leave the previous value untouched.
    assert!(store.save(key, &vec![0; MAX_PAYLOAD + 1]).await.is_err());
    assert_eq!(store.load(key).await.unwrap().unwrap(), [0, 255, 1, 2]);
    store.save("Key", b"upper").await.unwrap();
    store.save("key", b"lower").await.unwrap();
    assert_eq!(store.load("Key").await.unwrap().unwrap(), b"upper");
    assert_eq!(store.load("key").await.unwrap().unwrap(), b"lower");
    // Concurrent replacements can win in either order, but cannot tear.
    let (a, b) = tokio::join!(
        store.save("atomic", &[1; 8192]),
        store.save("atomic", &[2; 8192])
    );
    a.unwrap();
    b.unwrap();
    let value = store.load("atomic").await.unwrap().unwrap();
    assert!(value == vec![1; 8192] || value == vec![2; 8192]);
}
