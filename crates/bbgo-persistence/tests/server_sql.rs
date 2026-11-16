mod common;
use bbgo_core::CheckpointStore;
use bbgo_persistence::SqlCheckpointStore;

async fn native(postgres: bool) {
    assert_eq!(
        std::env::var("BBGO_ALLOW_TEST_DATABASE_WRITES").as_deref(),
        Ok("1")
    );
    let url = std::env::var(if postgres {
        "BBGO_TEST_POSTGRES_URL"
    } else {
        "BBGO_TEST_MYSQL_URL"
    })
    .unwrap();
    assert_eq!(
        url.split('?').next().unwrap().rsplit('/').next(),
        Some("bbgo_backtest_test")
    );
    let store = SqlCheckpointStore::connect(&url).await.unwrap();
    let pool = sqlx::AnyPool::connect(&url).await.unwrap();
    let ddl = if postgres {
        include_str!("../schemas/postgres-v1.sql")
    } else {
        include_str!("../schemas/mysql-v1.sql")
    };
    // No IF NOT EXISTS in tests: refuse to adopt existing checkpoint tables.
    sqlx::raw_sql(&ddl.replace("IF NOT EXISTS ", ""))
        .execute(&pool)
        .await
        .unwrap();
    store.initialize().await.unwrap();
    common::contract(&store).await;
    store.save("restart", b"durable").await.unwrap();
    store.close().await;
    let restored = SqlCheckpointStore::connect(&url).await.unwrap();
    assert_eq!(restored.load("restart").await.unwrap().unwrap(), b"durable");
    sqlx::query("UPDATE bbgo_checkpoints_v1 SET digest=NULL")
        .execute(&pool)
        .await
        .expect_err("NOT NULL must prevent malformed writes");
    sqlx::query("UPDATE bbgo_checkpoints_v1 SET payload=NULL")
        .execute(&pool)
        .await
        .expect_err("NOT NULL must prevent malformed writes");
    sqlx::query("UPDATE bbgo_checkpoints_v1 SET payload='corrupt'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(restored.load("restart").await.is_err());
    restored.close().await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires an explicit disposable PostgreSQL database"]
async fn postgres_checkpoint_contract() {
    native(true).await;
}

#[tokio::test]
#[ignore = "requires an explicit disposable MySQL database"]
async fn mysql_checkpoint_contract() {
    native(false).await;
}
