//! Bounded, drop-oldest event buffer.
//!
//! Deliberately channel-free: async channels do not work in the wasm component
//! async runtime, so events are accumulated in a plain deque (spec §5).

use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub method: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct Drained {
    pub entries: Vec<LogEntry>,
    /// Entries discarded to stay within the bound since the last drain.
    pub dropped: u64,
}

pub struct LogBuffer {
    entries: VecDeque<LogEntry>,
    cap: usize,
    dropped: u64,
}

impl LogBuffer {
    pub fn new(cap: usize) -> Self {
        LogBuffer {
            entries: VecDeque::new(),
            cap: cap.max(1),
            dropped: 0,
        }
    }

    pub fn push(&mut self, e: LogEntry) {
        while self.entries.len() >= self.cap {
            self.entries.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries.push_back(e);
    }

    pub fn drain(&mut self, max: Option<usize>) -> Drained {
        let n = max.unwrap_or(self.entries.len()).min(self.entries.len());
        let entries: Vec<LogEntry> = self.entries.drain(..n).collect();
        let dropped = std::mem::take(&mut self.dropped);
        Drained { entries, dropped }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> LogEntry {
        LogEntry {
            method: "log.entryAdded".into(),
            params: serde_json::json!({ "text": text }),
        }
    }

    #[test]
    fn drains_in_order() {
        let mut b = LogBuffer::new(10);
        b.push(entry("a"));
        b.push(entry("b"));
        let d = b.drain(None);
        assert_eq!(d.entries.len(), 2);
        assert_eq!(d.entries[0].params["text"], "a");
        assert_eq!(d.dropped, 0);
    }

    #[test]
    fn drain_empties_the_buffer() {
        let mut b = LogBuffer::new(10);
        b.push(entry("a"));
        assert_eq!(b.drain(None).entries.len(), 1);
        assert_eq!(b.drain(None).entries.len(), 0);
    }

    #[test]
    fn drops_oldest_when_full_and_counts() {
        let mut b = LogBuffer::new(2);
        b.push(entry("a"));
        b.push(entry("b"));
        b.push(entry("c")); // evicts "a"
        let d = b.drain(None);
        assert_eq!(d.entries.len(), 2);
        assert_eq!(d.entries[0].params["text"], "b");
        assert_eq!(d.dropped, 1);
    }

    #[test]
    fn dropped_count_resets_after_drain() {
        let mut b = LogBuffer::new(1);
        b.push(entry("a"));
        b.push(entry("b"));
        assert_eq!(b.drain(None).dropped, 1);
        assert_eq!(b.drain(None).dropped, 0);
    }

    #[test]
    fn max_limits_returned_entries_but_keeps_rest() {
        let mut b = LogBuffer::new(10);
        b.push(entry("a"));
        b.push(entry("b"));
        b.push(entry("c"));
        let d = b.drain(Some(2));
        assert_eq!(d.entries.len(), 2);
        let rest = b.drain(None);
        assert_eq!(rest.entries.len(), 1);
        assert_eq!(rest.entries[0].params["text"], "c");
    }

    #[test]
    fn zero_cap_is_treated_as_one() {
        let mut b = LogBuffer::new(0);
        b.push(entry("a"));
        assert_eq!(b.drain(None).entries.len(), 1);
    }
}
