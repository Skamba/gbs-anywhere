//! The server's own log, kept for the app: the last lines it printed
//! (`GET /api/log`) and new ones as they come (`GET /api/log/stream`), so a
//! person can look at it, or copy it into a bug report, without a terminal.
//!
//! [`LogLayer`] sits next to the console output in the binary's `tracing`
//! setup and copies every INFO, WARN and ERROR line into a [`LogBook`].

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// Lines kept.
pub const CAPACITY: usize = 1000;

/// One log line as the app gets it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LogLine {
    /// Counts up from 1 for every line since the start.
    pub seq: u64,
    /// UTC, RFC 3339 with milliseconds.
    pub ts: String,
    /// `error`, `warn` or `info`, lower case like the API's other codes.
    pub level: &'static str,
    /// The message as printed, without colour codes.
    pub text: String,
}

/// The last lines, and a feed of new ones.
pub struct LogBook {
    lines: Mutex<Lines>,
    live: broadcast::Sender<LogLine>,
    capacity: usize,
}

struct Lines {
    kept: VecDeque<LogLine>,
    seq: u64,
}

impl LogBook {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: Mutex::new(Lines {
                kept: VecDeque::with_capacity(capacity),
                seq: 0,
            }),
            live: broadcast::channel(256).0,
            capacity,
        }
    }

    /// The one the binary logs into (see [`LogLayer`]) and the API reads.
    pub fn global() -> Arc<LogBook> {
        static GLOBAL: LazyLock<Arc<LogBook>> = LazyLock::new(|| Arc::new(LogBook::new(CAPACITY)));
        GLOBAL.clone()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Adds a line; the oldest goes once the book is full.
    pub fn push(&self, level: Level, text: &str) -> LogLine {
        let mut lines = self.lock();
        lines.seq += 1;
        let line = LogLine {
            seq: lines.seq,
            ts: super::utc::with_millis(SystemTime::now()),
            level: level_name(level),
            text: shorten(strip_ansi(text)),
        };
        if lines.kept.len() >= self.capacity {
            lines.kept.pop_front();
        }
        lines.kept.push_back(line.clone());
        // Sent under the lock (it does not wait), so lines reach listeners
        // in order. No listeners is fine.
        let _ = self.live.send(line.clone());
        line
    }

    /// Lines with `seq > after`, oldest first, and the newest `seq`.
    pub fn after(&self, after: u64) -> (Vec<LogLine>, u64) {
        let lines = self.lock();
        let newer = lines
            .kept
            .iter()
            .filter(|l| l.seq > after)
            .cloned()
            .collect();
        (newer, lines.seq)
    }

    /// New lines as they are added.
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.live.subscribe()
    }

    /// New lines as they are added, and the newest `seq` so far: every line
    /// after it comes through the receiver.
    pub fn subscribe_at(&self) -> (broadcast::Receiver<LogLine>, u64) {
        let lines = self.lock();
        (self.live.subscribe(), lines.seq)
    }

    fn lock(&self) -> MutexGuard<'_, Lines> {
        self.lines.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

/// Longest text kept for one line; the rest is cut off.
const MAX_TEXT_BYTES: usize = 4096;

/// Cuts `text` to [`MAX_TEXT_BYTES`] (at a character boundary), marking the cut.
fn shorten(mut text: String) -> String {
    if text.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push('…');
    }
    text
}

/// Removes terminal escape sequences (colours), in case a message has them.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // `ESC [ ... final byte`; any other escape drops only the next char.
        match chars.next() {
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            _ => continue,
        }
    }
    out
}

/// Copies INFO, WARN and ERROR events into a [`LogBook`], formatted like
/// the console line (message, then `key=value` fields). It never logs
/// itself, which would call back into it.
pub struct LogLayer {
    book: Arc<LogBook>,
}

impl LogLayer {
    pub fn new(book: Arc<LogBook>) -> Self {
        Self { book }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        // More verbose levels compare greater.
        if level > Level::INFO {
            return;
        }
        let mut text = Text::default();
        event.record(&mut text);
        self.book.push(level, &text.finish());
    }
}

/// An event's message and other fields as one line.
#[derive(Default)]
struct Text {
    message: String,
    fields: String,
}

impl Text {
    fn finish(self) -> String {
        match (self.message.is_empty(), self.fields.is_empty()) {
            (_, true) => self.message,
            (true, false) => self.fields,
            (false, false) => format!("{} {}", self.message, self.fields),
        }
    }
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.record_debug(field, &value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        if name == "message" {
            let _ = write!(self.message, "{value:?}");
        } else if !name.starts_with("log.") {
            // `log.*` describe where a `log` crate record came from, which
            // the console line leaves out too.
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            let _ = write!(self.fields, "{name}={value:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn keeps_the_last_lines_in_order() {
        let book = LogBook::new(3);
        for i in 1..=5 {
            book.push(Level::INFO, &format!("line {i}"));
        }
        let (lines, last) = book.after(0);
        assert_eq!(last, 5);
        let texts: Vec<_> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["line 3", "line 4", "line 5"]);
        assert_eq!(book.after(4).0.len(), 1);
        assert!(book.after(5).0.is_empty());
        assert!(
            lines[0].ts.ends_with('Z') && lines[0].ts.len() == 24,
            "{}",
            lines[0].ts
        );
    }

    #[test]
    fn colour_codes_are_removed() {
        assert_eq!(strip_ansi("\u{1b}[2mgrey\u{1b}[0m plain"), "grey plain");
        assert_eq!(strip_ansi("\u{1b}[1;31mred\u{1b}[m"), "red");
        assert_eq!(strip_ansi("ä 😀"), "ä 😀");
    }

    #[test]
    fn long_lines_are_cut() {
        let book = LogBook::new(2);
        let line = book.push(Level::WARN, &"é".repeat(5000));
        assert!(line.text.len() <= MAX_TEXT_BYTES + '…'.len_utf8());
        assert!(line.text.ends_with('…'));
        assert_eq!(book.push(Level::INFO, "short").text, "short");
    }

    #[test]
    fn the_layer_copies_info_and_above() {
        let book = Arc::new(LogBook::new(10));
        let mut live = book.subscribe();
        let subscriber = tracing_subscriber::registry().with(LogLayer::new(book.clone()));
        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!("not kept");
            tracing::info!("grinder: paired");
            tracing::warn!(port = 80, name = "x", "cannot bind");
            tracing::error!("broken");
        });
        let (lines, _) = book.after(0);
        let got: Vec<_> = lines.iter().map(|l| (l.level, l.text.as_str())).collect();
        assert_eq!(
            got,
            [
                ("info", "grinder: paired"),
                ("warn", "cannot bind port=80 name=\"x\""),
                ("error", "broken"),
            ]
        );
        assert_eq!(live.try_recv().unwrap().text, "grinder: paired");
    }
}
