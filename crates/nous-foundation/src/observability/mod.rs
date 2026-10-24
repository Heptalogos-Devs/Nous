//! Process-wide diagnostics. Libraries emit through tracing and metrics facades.
mod config;
mod writer;
use crate::instance::InstancePaths;
pub use config::{OBSERVABILITY, ObservabilityConfig, register_configuration};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use nous_domain::{Error, ErrorCode, Result, TraceId};
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};
use tracing_appender::{
    non_blocking::NonBlockingBuilder,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    EnvFilter, Registry, layer::SubscriberExt, reload, util::SubscriberInitExt,
};
use writer::{LogWriter, Target};

static TELEMETRY: OnceLock<Observability> = OnceLock::new();
tokio::task_local! { static TRACE: TraceId; }

pub fn current_trace() -> Option<TraceId> {
    TRACE.try_with(Clone::clone).ok()
}

pub async fn in_trace<T>(trace_id: TraceId, future: impl std::future::Future<Output = T>) -> T {
    TRACE.scope(trace_id, future).await
}

pub fn contextualize(mut error: Error) -> Error {
    if error.trace_id.is_none() {
        error.trace_id = current_trace();
    }
    error
}

pub struct Observability {
    filter: reload::Handle<EnvFilter, Registry>,
    writer: LogWriter,
    metrics: PrometheusHandle,
    applied: Mutex<Option<Value>>,
}

/// Owns flushing the process logger before exit, including CLI failures.
pub struct ObservabilityGuard;

impl Observability {
    pub fn install() -> Result<ObservabilityGuard> {
        let (filter, handle) = reload::Layer::new(
            EnvFilter::try_new(ObservabilityConfig::default().filter).map_err(Error::dependency)?,
        );
        let writer = LogWriter::default();
        Registry::default()
            .with(filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(writer.clone()),
            )
            .try_init()
            .map_err(Error::dependency)?;
        let recorder = PrometheusBuilder::new()
            .set_buckets(&[
                0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2.5, 5., 10., 30., 60., 180., 600.,
            ])
            .map_err(Error::dependency)?
            .build_recorder();
        let metrics = recorder.handle();
        metrics::set_global_recorder(recorder).map_err(Error::dependency)?;
        TELEMETRY
            .set(Self {
                filter: handle,
                writer,
                metrics,
                applied: Mutex::new(None),
            })
            .map_err(|_| Error::new(ErrorCode::Conflict, "observability is already installed"))?;
        describe_metrics();
        Ok(ObservabilityGuard)
    }

    pub fn current() -> Result<&'static Self> {
        TELEMETRY
            .get()
            .ok_or_else(|| Error::internal("process observability is not installed"))
    }

    pub fn configure(&self, paths: &InstancePaths, config: &ObservabilityConfig) -> Result<()> {
        config.validate()?;
        let mut target = Target {
            stderr: config.stderr,
            ..Target::default()
        };
        let directory = paths.resolve("logs").map_err(|source| {
            Error::new(ErrorCode::DependencyUnavailable, source.to_string())
                .with_cause(source.as_ref())
        })?;
        if config.file {
            let appender = RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("nous")
                .filename_suffix("jsonl")
                .max_log_files(config.retained_files)
                .build(&directory)
                .map_err(Error::dependency)?;
            let (writer, guard) = NonBlockingBuilder::default()
                .buffered_lines_limit(config.buffered_lines)
                .thread_name("nous-log-writer")
                .finish(appender);
            target.dropped = Some(writer.error_counter());
            target.file = Some(writer);
            target.guard = Some(guard);
        }
        self.filter
            .reload(EnvFilter::try_new(&config.filter).map_err(Error::dependency)?)
            .map_err(Error::dependency)?;
        let previous = std::mem::replace(
            &mut *self
                .writer
                .0
                .write()
                .map_err(|_| Error::internal("log writer lock poisoned"))?,
            target,
        );
        // The previous worker drains after releasing the writer lock.
        drop(previous);
        *self
            .applied
            .lock()
            .map_err(|_| Error::internal("observability lock poisoned"))? = Some(json!({
            "config":config,"logs":directory,
        }));
        tracing::info!(event="observability.configured", logs=%directory.display(), "diagnostic outputs configured");
        Ok(())
    }

    pub fn diagnostics(&self) -> Result<Value> {
        let applied = self
            .applied
            .lock()
            .map_err(|_| Error::internal("observability lock poisoned"))?
            .clone();
        let target = self
            .writer
            .0
            .read()
            .map_err(|_| Error::internal("log writer lock poisoned"))?;
        Ok(
            json!({"process_id":std::process::id(),"logging":applied,"dropped_log_lines":target.dropped.as_ref().map_or(0, |counter| counter.dropped_lines()),"metrics_format":"prometheus"}),
        )
    }

    pub fn metrics(&self) -> String {
        if let Ok(target) = self.writer.0.read() {
            metrics::gauge!("nous_log_dropped_lines").set(
                target
                    .dropped
                    .as_ref()
                    .map_or(0, |counter| counter.dropped_lines()) as f64,
            );
        }
        self.metrics.run_upkeep();
        self.metrics.render()
    }
}

impl Drop for ObservabilityGuard {
    fn drop(&mut self) {
        if let Some(telemetry) = TELEMETRY.get()
            && let Ok(mut target) = telemetry.writer.0.write()
        {
            let previous = std::mem::take(&mut *target);
            drop(target);
            drop(previous);
        }
    }
}

fn describe_metrics() {
    metrics::describe_counter!(
        "nous_http_requests_total",
        "Completed HTTP requests by method, route and status"
    );
    metrics::describe_histogram!(
        "nous_http_request_duration_seconds",
        "HTTP request duration in seconds"
    );
    metrics::describe_counter!(
        "nous_operations_total",
        "Authority operations by action and state"
    );
    metrics::describe_counter!(
        "nous_tasks_total",
        "Durable task executions by kind and state"
    );
    metrics::describe_histogram!(
        "nous_task_duration_seconds",
        "Durable task execution duration in seconds"
    );
    metrics::describe_counter!(
        "nous_model_attempts_total",
        "Paid model attempts by role and status"
    );
    metrics::describe_histogram!(
        "nous_model_attempt_duration_seconds",
        "External model attempt duration in seconds"
    );
    metrics::describe_gauge!(
        "nous_log_dropped_lines",
        "Log lines dropped by the bounded file writer"
    );
}

pub fn request_finished(method: String, route: String, status: u16, elapsed: std::time::Duration) {
    metrics::counter!("nous_http_requests_total", "method"=>method.clone(), "route"=>route.clone(), "status"=>status.to_string()).increment(1);
    metrics::histogram!("nous_http_request_duration_seconds", "method"=>method, "route"=>route)
        .record(elapsed.as_secs_f64());
}
