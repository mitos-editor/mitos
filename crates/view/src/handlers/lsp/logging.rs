//! Server execution traces and local telemetry stay out of editor status messages.

use std::borrow::Cow;

use editor_config::LspTrace;
use lsp_client::{lsp, LanguageServerId};

use crate::Editor;

type Telemetry = lsp::OneOf<lsp::LSPObject, lsp::LSPArray>;

impl Editor {
    pub fn handle_language_server_trace(
        &self,
        server: LanguageServerId,
        params: lsp::LogTraceParams,
    ) {
        if let Some(client) = self.language_server_by_id(server) {
            log_trace(client.name(), server, self.config().lsp.trace, &params);
        }
    }

    pub fn handle_language_server_telemetry(&self, server: LanguageServerId, params: Telemetry) {
        if let Some(client) = self.language_server_by_id(server) {
            log_telemetry(client.name(), server, &params);
        }
    }
}

fn trace_message(trace: LspTrace, params: &lsp::LogTraceParams) -> Option<Cow<'_, str>> {
    match trace {
        LspTrace::Off => None,
        LspTrace::Messages => Some(Cow::Borrowed(&params.message)),
        LspTrace::Verbose => Some(match &params.verbose {
            Some(verbose) => Cow::Owned(format!("{}\n{verbose}", params.message)),
            None => Cow::Borrowed(&params.message),
        }),
    }
}

fn log_trace(name: &str, server: LanguageServerId, trace: LspTrace, params: &lsp::LogTraceParams) {
    if let Some(message) = trace_message(trace, params) {
        log::info!(target: "lsp_client::trace", "{name} [{server}]: {message}");
    }
}

fn log_telemetry(name: &str, server: LanguageServerId, params: &Telemetry) {
    if log::log_enabled!(target: "lsp_client::telemetry", log::Level::Debug)
        && let Ok(payload) = serde_json::to_string(params)
    {
        log::debug!(target: "lsp_client::telemetry", "{name} [{server}]: {payload}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Capture(Mutex<Vec<(log::Level, String)>>);

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            let message = record.args().to_string();
            if message.starts_with("trace-handler-test ") {
                self.0.lock().unwrap().push((record.level(), message));
            }
        }
        fn flush(&self) {}
    }

    static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

    #[test]
    fn trace_filtering_and_telemetry_levels_preserve_server_identity() {
        log::set_logger(&CAPTURE).unwrap();
        let previous = log::max_level();
        log::set_max_level(log::LevelFilter::Debug);
        let server = LanguageServerId::default();
        let params = lsp::LogTraceParams {
            message: "summary".into(),
            verbose: Some("extra detail".into()),
        };
        log_trace("trace-handler-test", server, LspTrace::Off, &params);
        assert!(CAPTURE.0.lock().unwrap().is_empty());
        log_trace("trace-handler-test", server, LspTrace::Messages, &params);
        log_trace("trace-handler-test", server, LspTrace::Verbose, &params);
        for payload in [serde_json::json!({"count": 2}), serde_json::json!([1, 2])] {
            let payload = serde_json::from_value(payload).unwrap();
            log_telemetry("trace-handler-test", server, &payload);
        }
        {
            let records = CAPTURE.0.lock().unwrap();
            assert_eq!(records.len(), 4);
            assert_eq!(records[0].0, log::Level::Info);
            assert!(records[0].1.contains(&format!("[{server}]: summary")));
            assert!(!records[0].1.contains("extra detail"));
            assert!(records[1].1.contains("summary\nextra detail"));
            assert!(records[2..]
                .iter()
                .all(|record| record.0 == log::Level::Debug));
        }
        log::set_max_level(log::LevelFilter::Info);
        log_telemetry("trace-handler-test", server, &lsp::OneOf::Right(vec![]));
        assert_eq!(CAPTURE.0.lock().unwrap().len(), 4);
        log::set_max_level(previous);
    }
}
