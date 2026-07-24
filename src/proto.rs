//! BiDi wire framing. Pure: no I/O, no async.

use serde_json::Value;

#[derive(Debug)]
pub enum Frame {
    Response { id: u64, body: Value },
    Error { id: u64, message: String },
    Event { method: String, params: Value },
}

#[derive(Debug)]
pub enum ProtoError {
    Malformed(String),
    UnknownType(String),
    MissingField(&'static str),
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Malformed(e) => write!(f, "malformed BiDi frame: {e}"),
            ProtoError::UnknownType(t) => write!(f, "unknown BiDi frame type {t:?}"),
            ProtoError::MissingField(n) => write!(f, "BiDi frame missing field {n:?}"),
        }
    }
}

/// Serialize an outgoing BiDi command.
pub fn command(id: u64, method: &str, params: Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

pub fn classify(text: &str) -> Result<Frame, ProtoError> {
    let v: Value = serde_json::from_str(text).map_err(|e| ProtoError::Malformed(e.to_string()))?;

    let ty = v
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ProtoError::MissingField("type"))?;

    match ty {
        "success" => {
            let id = v
                .get("id")
                .and_then(Value::as_u64)
                .ok_or(ProtoError::MissingField("id"))?;
            let body = v.get("result").cloned().unwrap_or(Value::Null);
            Ok(Frame::Response { id, body })
        }
        "error" => {
            let id = v
                .get("id")
                .and_then(Value::as_u64)
                .ok_or(ProtoError::MissingField("id"))?;
            let code = v.get("error").and_then(Value::as_str).unwrap_or("unknown");
            let msg = v.get("message").and_then(Value::as_str).unwrap_or("");
            Ok(Frame::Error {
                id,
                message: format!("{code}: {msg}"),
            })
        }
        "event" => {
            let method = v
                .get("method")
                .and_then(Value::as_str)
                .ok_or(ProtoError::MissingField("method"))?
                .to_string();
            let params = v.get("params").cloned().unwrap_or(Value::Null);
            Ok(Frame::Event { method, params })
        }
        other => Err(ProtoError::UnknownType(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_success_response() {
        let f = classify(r#"{"type":"success","id":7,"result":{"sessionId":"s1"}}"#).unwrap();
        match f {
            Frame::Response { id, body } => {
                assert_eq!(id, 7);
                assert_eq!(body["sessionId"], "s1");
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classifies_error_response() {
        let f = classify(r#"{"type":"error","id":9,"error":"no such node","message":"boom"}"#)
            .unwrap();
        match f {
            Frame::Error { id, message } => {
                assert_eq!(id, 9);
                assert!(message.contains("boom"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn classifies_event_without_id() {
        let f = classify(
            r#"{"type":"event","method":"log.entryAdded","params":{"level":"info","text":"hi"}}"#,
        )
        .unwrap();
        match f {
            Frame::Event { method, params } => {
                assert_eq!(method, "log.entryAdded");
                assert_eq!(params["text"], "hi");
            }
            other => panic!("expected Event, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(matches!(classify("not json"), Err(ProtoError::Malformed(_))));
    }

    #[test]
    fn rejects_unknown_type() {
        assert!(matches!(
            classify(r#"{"type":"weird","id":1}"#),
            Err(ProtoError::UnknownType(_))
        ));
    }

    #[test]
    fn command_serializes_with_id_and_method() {
        let s = command(3, "session.new", serde_json::json!({"capabilities":{}}));
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["id"], 3);
        assert_eq!(v["method"], "session.new");
        assert!(v["params"]["capabilities"].is_object());
    }
}
