//! Log setup shared by the binaries (issues 01, 330).
//!
//! The whole workspace logs through `tracing`; the libraries only emit
//! records, and an application installs the subscriber that writes them with
//! [`init`]. Records from dependencies that still use `log` are forwarded into
//! it. Which records are written is controlled by `RUST_LOG`; the format and
//! the destination come from `AppConfig::logging`.

use anyhow::{Context, Result};
use caelumora_model::config::{LogFormat, LoggingConfig};
use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Mutex;
use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::EnvFilter;

/// Where a binary logs when the configuration names no file.
#[derive(Debug, Clone)]
pub enum LogTarget {
    Stderr,
    File(PathBuf),
}

/// Installs the global subscriber.
///
/// `default_target` applies unless `config.file` is set; `default_filter` is
/// used when `RUST_LOG` is not set.
///
/// # Errors
/// If the log file cannot be opened or a subscriber is already installed.
pub fn init(config: &LoggingConfig, default_target: LogTarget, default_filter: &str) -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let target = config.file.clone().map(LogTarget::File).unwrap_or(default_target);

    let (writer, ansi) = match target {
        LogTarget::Stderr => (BoxMakeWriter::new(io::stderr), io::stderr().is_terminal()),
        LogTarget::File(path) => {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                fs::create_dir_all(parent)
                    .with_context(|| format!("cannot create log directory {}", parent.display()))?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .with_context(|| format!("cannot open log file {}", path.display()))?;
            (BoxMakeWriter::new(Mutex::new(file)), false)
        }
    };

    let builder = tracing_subscriber::fmt().with_env_filter(filter).with_writer(writer);
    let result = match config.format {
        LogFormat::Text => builder.with_ansi(ansi).try_init(),
        LogFormat::Json => builder.json().with_current_span(true).with_span_list(true).try_init(),
    };
    result.map_err(|e| anyhow::anyhow!("cannot install log subscriber: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use caelumora_model::actor::{ActorId, EventContext};
    use caelumora_runtime::logging::envelope_span;
    use serde_json::Value;
    use std::env;
    use tracing::info;
    use uuid::Uuid;

    // The only test in this crate that installs the global subscriber.
    #[test]
    fn json_log_to_file_carries_the_envelope_ids() {
        let dir = env::temp_dir().join(format!("caelumora-log-test-{}", Uuid::new_v4()));
        let path = dir.join("nested").join("test.log");
        let config = LoggingConfig { format: LogFormat::Json, file: Some(path.clone()) };
        init(&config, LogTarget::Stderr, "info").expect("subscriber installs");

        let context = EventContext::new(
            Default::default(),
            Default::default(),
            ActorId::None,
            "tester".to_string(),
        );
        envelope_span(&context, "ResponseRequested").in_scope(|| info!("inside the span"));

        let content =
            fs::read_to_string(&path).expect("the log file and its directory are created");
        let line =
            content.lines().find(|l| l.contains("inside the span")).expect("the event is written");
        let json: Value = serde_json::from_str(line).expect("each line is one JSON object");
        assert_eq!(json["span"]["span_id"], context.span_id.to_string());
        assert_eq!(json["span"]["event"], "ResponseRequested");
        assert_eq!(json["span"]["event_id"], context.event_id.to_string());
        let _ = fs::remove_dir_all(dir);
    }
}
