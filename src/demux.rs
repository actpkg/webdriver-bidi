//! Correlation of BiDi responses against the command we are awaiting.
//!
//! Pure and synchronous by design (spec §5): the I/O loop in `conn.rs` feeds
//! frames in, this decides what each one means. Keeping it separate is what
//! makes the interleaving cases testable without a browser.

use crate::logbuf::{LogBuffer, LogEntry};
use crate::proto::Frame;

#[derive(Debug)]
pub enum Step {
    Resolved(serde_json::Value),
    Failed(String),
    /// Not our frame — buffered if an event, discarded if stale. Keep pumping.
    Continue,
}

pub fn step(frame: Frame, want: u64, log: &mut LogBuffer) -> Step {
    match frame {
        Frame::Response { id, body } if id == want => Step::Resolved(body),
        Frame::Error { id, message } if id == want => Step::Failed(message),
        Frame::Event { method, params } => {
            log.push(LogEntry { method, params });
            Step::Continue
        }
        // Stale response for an abandoned/timed-out command.
        Frame::Response { .. } | Frame::Error { .. } => Step::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logbuf::LogBuffer;
    use crate::proto::Frame;

    #[test]
    fn resolves_matching_response() {
        let mut log = LogBuffer::new(10);
        let f = Frame::Response {
            id: 5,
            body: serde_json::json!({"ok": true}),
        };
        match step(f, 5, &mut log) {
            Step::Resolved(v) => assert_eq!(v["ok"], true),
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn fails_on_matching_error() {
        let mut log = LogBuffer::new(10);
        let f = Frame::Error {
            id: 5,
            message: "no such element".into(),
        };
        match step(f, 5, &mut log) {
            Step::Failed(m) => assert!(m.contains("no such element")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn buffers_event_and_continues() {
        let mut log = LogBuffer::new(10);
        let f = Frame::Event {
            method: "log.entryAdded".into(),
            params: serde_json::json!({"text": "hello"}),
        };
        assert!(matches!(step(f, 5, &mut log), Step::Continue));
        let d = log.drain(None);
        assert_eq!(d.entries.len(), 1);
        assert_eq!(d.entries[0].params["text"], "hello");
    }

    #[test]
    fn discards_stale_response_and_continues() {
        // A response for a command we are no longer waiting on must not resolve
        // the current one.
        let mut log = LogBuffer::new(10);
        let f = Frame::Response {
            id: 4,
            body: serde_json::json!({"stale": true}),
        };
        assert!(matches!(step(f, 5, &mut log), Step::Continue));
    }

    #[test]
    fn discards_stale_error_and_continues() {
        let mut log = LogBuffer::new(10);
        let f = Frame::Error {
            id: 4,
            message: "old failure".into(),
        };
        assert!(matches!(step(f, 5, &mut log), Step::Continue));
    }

    #[test]
    fn event_arriving_before_response_does_not_steal_it() {
        // The regression this module exists to prevent.
        let mut log = LogBuffer::new(10);
        let ev = Frame::Event {
            method: "log.entryAdded".into(),
            params: serde_json::json!({"text": "noise"}),
        };
        assert!(matches!(step(ev, 5, &mut log), Step::Continue));

        let resp = Frame::Response {
            id: 5,
            body: serde_json::json!({"value": 42}),
        };
        match step(resp, 5, &mut log) {
            Step::Resolved(v) => assert_eq!(v["value"], 42),
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert_eq!(log.drain(None).entries.len(), 1);
    }
}
