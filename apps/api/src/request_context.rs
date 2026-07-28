use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;
use uuid::Uuid;

use crate::request_query_count;

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

tokio::task_local! {
    static REQUEST_ID: String;
}

pub async fn middleware(mut request: Request, next: Next) -> Response {
    let request_id = request
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| valid_request_id(value))
        .map(str::to_owned)
        .unwrap_or_else(new_request_id);
    let span = tracing::info_span!("http_request", request_id = %request_id);
    request_query_count::scope(REQUEST_ID.scope(request_id.clone(), async move {
        request.extensions_mut().insert(request_id.clone());
        let mut response = next.run(request).instrument(span).await;
        if let Ok(header) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert(REQUEST_ID_HEADER, header);
        }
        response
    }))
    .await
}

pub fn current_request_id() -> String {
    REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| new_request_id())
}

fn new_request_id() -> String {
    format!("req_{}", Uuid::now_v7().simple())
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incoming_request_ids_are_bounded_and_log_safe() {
        assert!(valid_request_id("trace-1234.abc:def"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("contains a space"));
        assert!(!valid_request_id(&"x".repeat(129)));
    }

    #[tokio::test]
    async fn scoped_request_id_is_reused_by_response_models() {
        let value = REQUEST_ID
            .scope("req_test".to_owned(), async { current_request_id() })
            .await;
        assert_eq!(value, "req_test");
    }
}
