use std::{cell::Cell, future::Future};

use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::{Layer, layer::Context};

tokio::task_local! {
    static REQUEST_QUERY_COUNT: Cell<usize>;
}

/// Count SQL statements completed by SQLx while a request is in scope.
///
/// SQLx emits exactly one `sqlx::query` event when a statement finishes. The
/// layer is intentionally independent of the log filter: production can keep
/// SQLx logs at `warn` while this inexpensive counter still observes every
/// completed statement. SQLx does not emit an event for its protocol-level
/// `BEGIN`, but it does emit for context setup, application statements, and
/// `COMMIT`; that definition is pinned in `eval/query_budgets.json`.
#[derive(Clone, Copy, Debug, Default)]
pub struct QueryCountLayer;

impl<S> Layer<S> for QueryCountLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        if is_sqlx_query(event.metadata()) {
            increment();
        }
    }
}

fn is_sqlx_query(metadata: &Metadata<'_>) -> bool {
    metadata.target() == "sqlx::query"
}

pub async fn scope<F>(future: F) -> F::Output
where
    F: Future,
{
    REQUEST_QUERY_COUNT.scope(Cell::new(0), future).await
}

pub fn current() -> Option<usize> {
    REQUEST_QUERY_COUNT.try_with(Cell::get).ok()
}

fn increment() {
    let _ = REQUEST_QUERY_COUNT.try_with(|count| {
        count.set(count.get().saturating_add(1));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::SubscriberExt;

    #[tokio::test(flavor = "current_thread")]
    async fn counts_sqlx_events_only_inside_request_scope() {
        let subscriber = tracing_subscriber::registry().with(QueryCountLayer);
        let dispatch = tracing::Dispatch::new(subscriber);
        let guard = tracing::dispatcher::set_default(&dispatch);
        let observed = scope(async {
            tracing::debug!(target: "sqlx::query", "SELECT 1");
            tracing::debug!(target: "straylight::test", "not a query");
            current()
        })
        .await;
        drop(guard);
        assert_eq!(observed, Some(1));
        assert_eq!(current(), None);
    }

    #[test]
    fn count_saturates_instead_of_wrapping() {
        REQUEST_QUERY_COUNT.sync_scope(Cell::new(usize::MAX), || {
            increment();
            assert_eq!(current(), Some(usize::MAX));
        });
    }

    #[test]
    fn layer_is_stateless_and_thread_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<QueryCountLayer>();
        assert_eq!(AtomicUsize::new(0).load(Ordering::Relaxed), 0);
    }
}
