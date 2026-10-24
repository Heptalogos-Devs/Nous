use super::ApiError;
use axum::{
    extract::MatchedPath,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use nous_domain::{Error, TraceId};
use nous_foundation::observability;
use std::time::Instant;
use tracing::Instrument;

pub(super) async fn trace_request(mut request: Request<axum::body::Body>, next: Next) -> Response {
    let trace = request
        .headers()
        .get("x-nous-trace-id")
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Error::invalid("x-nous-trace-id must be a canonical UUID"))?
                .parse::<TraceId>()
        })
        .transpose();
    let (trace_id, invalid) = match trace {
        Ok(trace) => (trace.unwrap_or_default(), None),
        Err(error) => (TraceId::new(), Some(error)),
    };
    let started = Instant::now();
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|route| route.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let span =
        tracing::info_span!("http.request", trace_id=%trace_id, method=%method, route=%route);
    request.extensions_mut().insert(trace_id.clone());
    let mut response = observability::in_trace(
        trace_id.clone(),
        async {
            let response = match invalid {
                Some(error) => ApiError::from(error).into_response(),
                None => next.run(request).await,
            };
            let elapsed = started.elapsed();
            observability::request_finished(method, route, response.status().as_u16(), elapsed);
            tracing::info!(
                event = "http.completed",
                status = response.status().as_u16(),
                elapsed_ms = elapsed.as_millis() as u64,
                "request completed"
            );
            response
        }
        .instrument(span),
    )
    .await;
    response.headers_mut().insert(
        "x-nous-trace-id",
        trace_id.to_string().parse().expect("canonical UUID header"),
    );
    response
}
