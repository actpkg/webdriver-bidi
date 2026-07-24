//! Live BiDi connection: per-session current_thread tokio runtime driving a
//! plaintext WebSocket to a loopback endpoint.
//!
//! No TLS. Scope is loopback-only (spec §3) — do not add rustls here.

use crate::caps::CapSet;
use crate::demux::{Step, step};
use crate::logbuf::{Drained, LogBuffer};
use crate::proto::{classify, command};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::runtime::{Builder, Runtime};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{WebSocketStream, client_async};

pub struct ConnConfig {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub timeout_ms: u32,
    pub log_buffer_cap: u32,
    pub caps: CapSet,
}

pub struct BidiConn {
    rt: Runtime,
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    timeout: Duration,
    log: LogBuffer,
    caps: CapSet,
    /// BiDi session id from `session.new` — distinct from the ACT std:session-id.
    #[allow(dead_code)]
    bidi_session_id: String,
    context: String,
}

impl BidiConn {
    pub fn open(cfg: ConnConfig) -> Result<BidiConn, String> {
        let addr = crate::addr::resolve(&cfg.host, cfg.port).map_err(|e| e.to_string())?;
        let url = format!("ws://{}{}", addr, cfg.path);
        let dur = Duration::from_millis(cfg.timeout_ms as u64);

        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;

        let ws = rt.block_on(async {
            let tcp = timeout(dur, TcpStream::connect(addr))
                .await
                .map_err(|_| format!("timed out connecting to {addr}"))?
                .map_err(|e| format!("connect {addr}: {e}"))?;
            let (ws, _resp) = timeout(dur, client_async(&url, tcp))
                .await
                .map_err(|_| "timed out during WebSocket handshake".to_string())?
                .map_err(|e| format!("websocket handshake: {e}"))?;
            Ok::<_, String>(ws)
        })?;

        let mut c = BidiConn {
            rt,
            ws,
            next_id: 1,
            timeout: dur,
            log: LogBuffer::new(cfg.log_buffer_cap as usize),
            caps: cfg.caps,
            bidi_session_id: String::new(),
            context: String::new(),
        };

        // Establish the BiDi session.
        let res = c.send("session.new", serde_json::json!({ "capabilities": {} }))?;
        c.bidi_session_id = res
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        // Subscribe to console/log events so the buffer fills.
        c.send(
            "session.subscribe",
            serde_json::json!({ "events": ["log.entryAdded"] }),
        )?;

        // Resolve a default browsing context.
        let tree = c.send("browsingContext.getTree", serde_json::json!({}))?;
        c.context = tree
            .get("contexts")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|c0| c0.get("context"))
            .and_then(|v| v.as_str())
            .ok_or("BiDi endpoint returned no browsing context")?
            .to_string();

        Ok(c)
    }

    pub fn caps(&self) -> &CapSet {
        &self.caps
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn set_context(&mut self, ctx: String) {
        self.context = ctx;
    }

    pub fn drain_log(&mut self, max: Option<usize>) -> Drained {
        self.log.drain(max)
    }

    /// Send a command and pump the socket until its response arrives, buffering
    /// any events that interleave (spec §5).
    pub fn send(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let payload = command(id, method, params);

        let dur = self.timeout;
        let ws = &mut self.ws;
        let log = &mut self.log;

        self.rt.block_on(async move {
            timeout(dur, ws.send(Message::Text(payload)))
                .await
                .map_err(|_| format!("timed out sending {method}"))?
                .map_err(|e| format!("send {method}: {e}"))?;

            loop {
                let msg = timeout(dur, ws.next())
                    .await
                    .map_err(|_| format!("timed out awaiting response to {method}"))?
                    .ok_or_else(|| "websocket closed by peer".to_string())?
                    .map_err(|e| format!("websocket read: {e}"))?;

                let text = match msg {
                    Message::Text(t) => t,
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => return Err("websocket closed by peer".to_string()),
                    // Ping/Pong are handled by tungstenite; ignore anything else.
                    _ => continue,
                };

                let frame = classify(&text).map_err(|e| e.to_string())?;
                match step(frame, id, log) {
                    Step::Resolved(v) => return Ok(v),
                    Step::Failed(m) => return Err(format!("{method}: {m}")),
                    Step::Continue => continue,
                }
            }
        })
    }

    /// Best-effort `session.end`. Errors are ignored — the socket is being torn
    /// down regardless.
    pub fn end(&mut self) {
        let _ = self.send("session.end", serde_json::json!({}));
    }
}
