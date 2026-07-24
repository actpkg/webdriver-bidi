use act_sdk::prelude::*;
use ciborium::value::Value as Cv;

pub mod addr;
pub mod caps;
pub mod demux;
pub mod logbuf;
pub mod proto;

mod conn;
use caps::{BrowserCap, CapSet};
use conn::{BidiConn, ConnConfig};

#[act_component]
mod component {
    use super::*;

    thread_local! {
        static SESSIONS: SessionRegistry<BidiConn> = SessionRegistry::new("webdriver-bidi");
    }

    // ── OpenArgs ──────────────────────────────────────────────────────────────

    #[derive(Deserialize, JsonSchema)]
    pub struct OpenArgs {
        /// Loopback IP literal of the BiDi endpoint. Hostnames are rejected.
        #[serde(default = "default_host")]
        pub host: String,
        /// Port of the BiDi endpoint (e.g. 9222 for Chrome, 4444 for geckodriver).
        pub port: u16,
        /// WebSocket path. Defaults to `/session`.
        #[serde(default = "default_path")]
        pub path: String,
        /// Per-command timeout in milliseconds. Defaults to 30000.
        #[serde(default = "default_timeout")]
        pub timeout_ms: u32,
        /// Maximum buffered console entries before oldest are dropped. Defaults to 1000.
        #[serde(default = "default_log_cap")]
        pub log_buffer_cap: u32,
        /// Browser capability classes granted to this session.
        /// Defaults to all four. Narrow it to restrict what tools may do.
        #[serde(default)]
        pub allow: Option<Vec<BrowserCap>>,
    }

    fn default_host() -> String {
        "127.0.0.1".to_string()
    }
    fn default_path() -> String {
        "/session".to_string()
    }
    fn default_timeout() -> u32 {
        30_000
    }
    fn default_log_cap() -> u32 {
        1000
    }

    // ── Tool metadata ─────────────────────────────────────────────────────────

    #[derive(Deserialize)]
    pub struct ToolMeta {
        #[serde(rename = "std:session-id")]
        session_id: Option<String>,
    }

    // ── Session lifecycle ─────────────────────────────────────────────────────

    #[session_open]
    fn open(args: OpenArgs) -> ActResult<String> {
        let caps = match args.allow {
            Some(list) => CapSet::from_list(list),
            None => CapSet::all(),
        };
        let conn = BidiConn::open(ConnConfig {
            host: args.host,
            port: args.port,
            path: args.path,
            timeout_ms: args.timeout_ms,
            log_buffer_cap: args.log_buffer_cap,
            caps,
        })
        .map_err(ActError::internal)?;
        Ok(SESSIONS.with(|r| r.insert(conn)))
    }

    #[session_close]
    fn close(session_id: String) {
        SESSIONS.with(|r| {
            if let Some(mut c) = r.remove(&session_id) {
                c.end();
            }
        });
    }

    fn require_session(ctx: &mut ActContext<ToolMeta>) -> ActResult<String> {
        ctx.metadata()
            .session_id
            .clone()
            .ok_or_else(|| ActError::session_not_found("Missing std:session-id metadata"))
    }

    fn with_session_mut<F, T>(id: &str, f: F) -> ActResult<T>
    where
        F: FnOnce(&mut BidiConn) -> ActResult<T>,
    {
        SESSIONS
            .with(|r| r.with_mut(id, f))
            .ok_or_else(|| ActError::session_not_found(format!("Unknown session-id: {id}")))?
    }

    /// TODO(act-consent): self-enforced until the host becomes the enforcement point.
    fn gate(c: &BidiConn, cap: BrowserCap) -> ActResult<()> {
        c.caps().require(cap).map_err(ActError::capability_denied)
    }

    /// Convert a serde_json value into a ciborium value for the tool response.
    fn json_to_cbor(v: &serde_json::Value) -> ActResult<Cv> {
        let mut buf = Vec::new();
        ciborium::into_writer(v, &mut buf)
            .map_err(|e| ActError::internal(format!("cbor encode: {e}")))?;
        ciborium::from_reader(&buf[..])
            .map_err(|e| ActError::internal(format!("cbor decode: {e}")))
    }
}
