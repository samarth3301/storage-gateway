use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::env;
use std::sync::OnceLock;
use std::time::Duration;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter, Registry,
};

const DEFAULT_FILTER: &str = concat!(env!("CARGO_CRATE_NAME"), "=info");

#[derive(Debug, Clone)]
pub struct LoggerConfig {
    pub filter: String,
    pub json_format: bool,
    pub with_file_info: bool,
    pub with_thread_ids: bool,
    pub with_thread_names: bool,
    pub with_span_events: bool,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            filter: DEFAULT_FILTER.to_string(),
            json_format: false,
            with_file_info: false,
            with_thread_ids: false,
            with_thread_names: false,
            with_span_events: false,
        }
    }
}

impl LoggerConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        let lower = |name| env::var(name).ok().map(|v| v.to_lowercase());

        if let Ok(filter) = env::var("RUST_LOG") {
            config.filter = filter;
        } else if let Some(level) = lower("LOG_LEVEL") {
            config.filter = format!("{}={}", env!("CARGO_CRATE_NAME"), level);
        }
        if let Some(format) = lower("LOG_FORMAT") {
            config.json_format = format == "json";
        }
        if let Some(thread_info) = lower("LOG_THREAD_INFO") {
            config.with_thread_ids = thread_info.contains("id");
            config.with_thread_names = thread_info.contains("name");
        }
        if let Some(span_events) = lower("LOG_SPAN_EVENTS") {
            config.with_span_events = span_events == "true";
        }
        if let Some(file_info) = lower("LOG_FILE_INFO") {
            config.with_file_info = file_info == "true";
        }
        config
    }
}

pub fn init_logger() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_logger_with_config(LoggerConfig::from_env())
}

pub fn init_logger_with_config(
    config: LoggerConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter = EnvFilter::try_new(&config.filter)
        .map_err(|e| format!("Failed to create log filter: {}", e))?;
    let span_events = if config.with_span_events {
        FmtSpan::NEW | FmtSpan::CLOSE
    } else {
        FmtSpan::NONE
    };

    let layer = fmt::layer()
        .with_file(config.with_file_info)
        .with_line_number(config.with_file_info)
        .with_thread_ids(config.with_thread_ids)
        .with_thread_names(config.with_thread_names)
        .with_span_events(span_events);

    let registry = Registry::default().with(filter);
    if config.json_format {
        registry.with(layer.json()).try_init()?;
    } else {
        registry
            .with(layer.with_target(false).compact())
            .try_init()?;
    }
    Ok(())
}

const LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub fn metrics_handle() -> &'static PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Suffix("duration_seconds".to_string()),
                LATENCY_BUCKETS,
            )
            .expect("latency buckets are non-empty")
            .install_recorder()
            .expect("metrics recorder is installed once")
    })
}

pub async fn run_metrics_upkeep() {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        metrics_handle().run_upkeep();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_targets_this_crate() {
        let config = LoggerConfig::default();
        assert_eq!(config.filter, "s3_cdn_network=info");
        assert!(EnvFilter::try_new(&config.filter).is_ok());
    }
}
