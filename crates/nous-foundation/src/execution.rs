//! Owned work retains its resources until asynchronous and blocking I/O actually stop.
use crate::observability::{contextualize, current_trace, in_trace};
use nous_domain::{Error, ErrorCode, Result};
use std::future::Future;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

pub fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(contextualize(Error::new(
            ErrorCode::Cancelled,
            "operation cancelled",
        )))
    } else {
        Ok(())
    }
}

/// Finish resource cleanup independently of the future that released its last guard.
pub fn cleanup<F>(work: F)
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let trace = current_trace();
    let span = tracing::Span::current();
    let _cleanup = runtime.spawn(
        async move {
            let result = match trace {
                Some(trace) => in_trace(trace, work).await,
                None => work.await,
            };
            if let Err(error) = result {
                tracing::warn!(event="execution.cleanup_failed",error=?error);
            }
        }
        .instrument(span),
    );
}

pub async fn owned_async<T, F, Fut>(cancel: &CancellationToken, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Fut + Send + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    check_cancelled(cancel)?;
    let local = cancel.child_token();
    let cancellation = local.clone();
    let _drop_guard = local.clone().drop_guard();
    let trace = current_trace();
    let span = tracing::Span::current();
    let future = async move {
        let run = async move { work(local).await.map_err(contextualize) };
        match trace {
            Some(trace) => in_trace(trace, run).await,
            None => run.await,
        }
    }
    .instrument(span);
    let mut running = tokio::spawn(future);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            cancellation.cancel();
            if let Err(error) = running.await.map_err(Error::dependency).and_then(|result| result)
                && error.code != ErrorCode::Cancelled {
                tracing::warn!(event="execution.cancel_cleanup_failed", error=?error);
            }
            Err(contextualize(Error::new(ErrorCode::Cancelled, "operation cancelled")))
        }
        result = &mut running => result.map_err(Error::dependency)?.map_err(contextualize),
    }
}

pub async fn blocking<T, F>(cancel: &CancellationToken, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(CancellationToken) -> Result<T> + Send + 'static,
{
    let trace = current_trace();
    let span = tracing::Span::current();
    owned_async(cancel, move |local| async move {
        tokio::task::spawn_blocking(move || {
            span.in_scope(|| {
                check_cancelled(&local)?;
                work(local).map_err(|mut error| {
                    if error.trace_id.is_none() {
                        error.trace_id = trace;
                    }
                    error
                })
            })
        })
        .await
        .map_err(Error::dependency)?
    })
    .await
}
