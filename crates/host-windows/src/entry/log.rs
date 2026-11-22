//! A minimal `tracing` sink for a DLL that must not depend on a logging crate.
//!
//! `tracing` events with no subscriber are dropped, and a host whose report says
//! "the entry did not resolve" gives no way to see why. This module installs the
//! smallest possible subscriber: one line per event, written to
//! `<config dir>/logs/windows-host.log`, or to stderr when the file cannot be
//! opened. No levels beyond `enabled`, no spans with fields, no dynamic reload —
//! the dynamic level came from one place before (`diagnostics`), and that crate is
//! not a dependency of the Windows host.
//!
//! It is installed by the worker thread, never by `DllMain`: opening a file under
//! the loader lock is exactly what the entry point must not do.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tracing::span;
use tracing::{Event, Metadata, Subscriber};

/// Where the lines go once [`install`] has run.
static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// Minimum level this sink writes.
const MIN_LEVEL: tracing::Level = tracing::Level::INFO;

enum Sink {
    File(File),
    Stderr,
}

impl Sink {
    fn write_line(&mut self, line: &str) {
        match self {
            Self::File(file) => {
                // A failed write is logged nowhere - there is nowhere left to
                // log it - but the host keeps running, which is the point.
                let _ = file.write_all(line.as_bytes());
            }
            Self::Stderr => {
                // Nowhere left to report a stderr write failure to; the host keeps
                // running, which is the point of the fallback.
                let _ = std::io::stderr().write_all(line.as_bytes());
            }
        }
    }
}

/// Install the sink, writing to `path` when it can be opened.
///
/// Safe to call more than once; only the first call installs a subscriber.
pub fn install(path: Option<&Path>) {
    let sink = match path {
        Some(path) => open_log(path).map_or(Sink::Stderr, Sink::File),
        None => Sink::Stderr,
    };
    let _ = SINK.set(Mutex::new(sink));
    let _ = tracing::subscriber::set_global_default(LineSubscriber);
}

fn open_log(path: &Path) -> Option<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Writes one `[level] message field=value …` line per event.
struct LineSubscriber;

impl Subscriber for LineSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= MIN_LEVEL
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(1)
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = format!("[{}] ", event.metadata().level());
        event.record(&mut LineVisitor(&mut line));
        line.push('\n');
        if let Some(sink) = SINK.get() {
            if let Ok(mut sink) = sink.lock() {
                sink.write_line(&line);
            }
        }
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

struct LineVisitor<'a>(&'a mut String);

impl tracing::field::Visit for LineVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.write(field.name(), value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let field = field.name();
        self.write(field, &format!("{value:?}"));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.write(field.name(), &value.to_string());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.write(field.name(), &value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.write(field.name(), &value.to_string());
    }
}

impl LineVisitor<'_> {
    fn write(&mut self, field: &str, value: &str) {
        if field == "message" {
            self.0.push_str(value);
        } else {
            self.0.push_str(&format!(" {field}={value}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_file_is_created_under_the_given_directory() {
        let root = std::env::temp_dir().join("vf-host-windows-log-sink");
        let path = root.join("logs/windows-host.log");
        let _ = std::fs::remove_dir_all(&root);
        let file = open_log(&path).expect("the log file opens");
        drop(file);
        assert!(path.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_impossible_log_path_falls_back_to_stderr_instead_of_failing() {
        assert!(open_log(Path::new("/proc/definitely/not/writable/host.log")).is_none());
    }

    #[test]
    fn the_visitor_renders_a_message_and_its_fields() {
        let mut line = String::new();
        let mut visitor = LineVisitor(&mut line);
        visitor.write("message", "hook installed");
        visitor.write("symbol", "CUser::CheckAppOwnership");
        assert_eq!(line, "hook installed symbol=CUser::CheckAppOwnership");
    }
}
