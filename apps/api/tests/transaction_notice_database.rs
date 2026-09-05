use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use brunn::db::operator_pool;
use serde_json::Value;
use tracing::Instrument;

#[tokio::test]
async fn cancelled_read_commit_reproduces_notice_but_dropped_read_does_not() {
    let Ok(url) = std::env::var("BRUNN_TEST_DATABASE_URL") else {
        eprintln!("BRUNN_TEST_DATABASE_URL is unset; skipping transaction notice database test");
        return;
    };
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let output = logs.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || LogWriter(output.clone()))
            .finish(),
    )
    .expect("install notice collector");
    let pool = operator_pool(&url).await.expect("connect disposable pool");

    // A read-only transaction under a lane deadline can send COMMIT and be
    // cancelled before SQLx reads ReadyForQuery. Drop then queues ROLLBACK
    // after that COMMIT; Postgres correctly warns outside the request task.
    for _ in 0..20 {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
        let mut commit = Box::pin(tx.commit());
        let polled =
            std::future::poll_fn(|cx| std::task::Poll::Ready(commit.as_mut().poll(cx))).await;
        assert!(polled.is_pending());
        drop(commit);
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
    }
    let before = notices(&logs);
    assert!(
        !before.is_empty(),
        "cancelled COMMIT must reproduce the incident mechanism"
    );
    assert!(
        before
            .iter()
            .all(|notice| notice["span"]["name"] == "db_pool")
    );

    // Read-only lanes need no commit. Dropping once queues a single rollback;
    // there is no cancellable await between transaction completion and drop.
    for _ in 0..20 {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SELECT 1").execute(&mut *tx).await.unwrap();
        drop(tx);
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
    }
    assert_eq!(notices(&logs).len(), before.len());

    sqlx::query("ROLLBACK")
        .execute(&pool)
        .instrument(tracing::info_span!(
            "http_request",
            request_id = "req_notice_test"
        ))
        .await
        .unwrap();
    let after = notices(&logs);
    assert_eq!(
        after.last().unwrap()["span"]["request_id"],
        "req_notice_test"
    );
    eprintln!(
        "cancelled read COMMIT: {} notices; dropped read: 0; request and pool spans verified",
        before.len()
    );
    pool.close().await;
}

struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn notices(logs: &Arc<Mutex<Vec<u8>>>) -> Vec<Value> {
    String::from_utf8(logs.lock().unwrap().clone())
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| {
            event["target"] == "sqlx::postgres::notice"
                && event["fields"]["message"] == "there is no transaction in progress"
        })
        .collect()
}
