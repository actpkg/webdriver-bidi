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

    // ── Navigation / browsing contexts ────────────────────────────────────────

    #[act_tool(description = "Navigate the current browsing context to a URL.")]
    fn navigate(
        /// Absolute URL to load.
        url: String,
        /// Readiness to wait for: none | interactive | complete. Default complete.
        wait: Option<String>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Navigate)?;
            let context = c.context().to_string();
            let res = c
                .send(
                    "browsingContext.navigate",
                    serde_json::json!({
                        "context": context,
                        "url": url,
                        "wait": wait.unwrap_or_else(|| "complete".into()),
                    }),
                )
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    #[act_tool(description = "List open browsing contexts (tabs/windows).", read_only)]
    fn context_list(ctx: &mut ActContext<ToolMeta>) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Read)?;
            let res = c
                .send("browsingContext.getTree", serde_json::json!({}))
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    #[act_tool(
        description = "Create a new browsing context and make it current for this session."
    )]
    fn context_create(
        /// Context type: tab | window. Default tab.
        r#type: Option<String>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Navigate)?;
            let res = c
                .send(
                    "browsingContext.create",
                    serde_json::json!({ "type": r#type.unwrap_or_else(|| "tab".into()) }),
                )
                .map_err(ActError::internal)?;
            if let Some(new_ctx) = res.get("context").and_then(|v| v.as_str()) {
                c.set_context(new_ctx.to_string());
            }
            json_to_cbor(&res)
        })
    }

    #[act_tool(description = "Close a browsing context. Defaults to the current one.")]
    fn context_close(
        /// Context id to close. Defaults to the session's current context.
        context: Option<String>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Navigate)?;
            let target = context.unwrap_or_else(|| c.context().to_string());
            let res = c
                .send(
                    "browsingContext.close",
                    serde_json::json!({ "context": target }),
                )
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    #[act_tool(
        description = "Capture a PNG screenshot of the current browsing context.",
        read_only
    )]
    fn screenshot(ctx: &mut ActContext<ToolMeta>) -> ActResult<Vec<u8>> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Read)?;
            let context = c.context().to_string();
            let res = c
                .send(
                    "browsingContext.captureScreenshot",
                    serde_json::json!({ "context": context }),
                )
                .map_err(ActError::internal)?;
            let b64 = res
                .get("data")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ActError::internal("screenshot response had no data field"))?;
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| ActError::internal(format!("bad base64 screenshot: {e}")))
        })
    }
}
