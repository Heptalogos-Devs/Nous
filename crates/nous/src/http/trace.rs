use super::ApiError;
use axum::{
    extract::MatchedPath,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use nous_domain::{Error, TraceId};
use nous_foundation::observability;
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
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
    observability::request_started();
    let mut observed = RequestObservation {
        method,
        route,
        started,
        span: span.clone(),
        status: None,
        response_bytes: 0,
        finished: false,
    };
    request.extensions_mut().insert(trace_id.clone());
    let mut response = observability::in_trace(
        trace_id.clone(),
        async {
            match invalid {
                Some(error) => ApiError::from(error).into_response(),
                None => next.run(request).await,
            }
        }
        .instrument(span),
    )
    .await;
    response.headers_mut().insert(
        "x-nous-trace-id",
        trace_id.to_string().parse().expect("canonical UUID header"),
    );
    observed.status = Some(response.status().as_u16());
    let (parts, body) = response.into_parts();
    if body.is_end_stream() {
        observed.finish("completed");
    }
    Response::from_parts(
        parts,
        axum::body::Body::new(ObservedBody {
            inner: body,
            observed,
        }),
    )
}

struct RequestObservation {
    method: String,
    route: String,
    started: Instant,
    span: tracing::Span,
    status: Option<u16>,
    response_bytes: u64,
    finished: bool,
}
impl RequestObservation {
    fn finish(&mut self, outcome: &'static str) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }
        let elapsed = self.started.elapsed();
        observability::request_finished(
            self.method.clone(),
            self.route.clone(),
            self.status,
            outcome,
            elapsed,
            self.response_bytes,
        );
        tracing::info!(parent:&self.span, event="http.completed", status=self.status, outcome, response_bytes=self.response_bytes, elapsed_ms=elapsed.as_millis() as u64, "request completed");
    }
}
impl Drop for RequestObservation {
    fn drop(&mut self) {
        self.finish("aborted");
    }
}

struct ObservedBody {
    inner: axum::body::Body,
    observed: RequestObservation,
}
impl HttpBody for ObservedBody {
    type Data = axum::body::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        let result = this
            .observed
            .span
            .in_scope(|| Pin::new(&mut this.inner).poll_frame(cx));
        match &result {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    this.observed.response_bytes = this
                        .observed
                        .response_bytes
                        .saturating_add(data.len() as u64);
                }
                // A known-length body can finish on its last frame without a later EOF poll.
                if this.inner.is_end_stream() {
                    this.observed.finish("completed");
                }
            }
            Poll::Ready(Some(Err(error))) => {
                tracing::warn!(parent:&this.observed.span,event="http.body_failed",%error,"response body failed");
                this.observed.finish("failed");
            }
            Poll::Ready(None) => this.observed.finish("completed"),
            Poll::Pending => {}
        }
        result
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
