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

    // ── DOM interaction ───────────────────────────────────────────────────────

    /// Resolve a CSS selector to a BiDi node sharedId.
    ///
    /// Requires `browser:script` — `input.performActions` needs an element
    /// origin, and resolving one goes through `script.evaluate` (spec §6).
    fn resolve_node(c: &mut BidiConn, selector: &str) -> ActResult<String> {
        let context = c.context().to_string();
        let expr = format!(
            "document.querySelector({})",
            serde_json::to_string(selector).unwrap_or_else(|_| "null".into())
        );
        let res = c
            .send(
                "script.evaluate",
                serde_json::json!({
                    "expression": expr,
                    "target": { "context": context },
                    "awaitPromise": false,
                    "resultOwnership": "root",
                }),
            )
            .map_err(ActError::internal)?;
        res.get("result")
            .and_then(|r| r.get("sharedId"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                ActError::invalid_args(format!("no element matched selector {selector:?}"))
            })
    }

    /// Pointer action sequence that clicks a resolved element.
    fn click_actions(shared_id: &str) -> serde_json::Value {
        serde_json::json!([{
            "type": "pointer",
            "id": "mouse",
            "actions": [
                { "type": "pointerMove", "x": 0, "y": 0,
                  "origin": { "type": "element",
                              "element": { "sharedId": shared_id } } },
                { "type": "pointerDown", "button": 0 },
                { "type": "pointerUp", "button": 0 }
            ]
        }])
    }

    #[act_tool(
        description = "Evaluate a JavaScript expression in the current page and return its value."
    )]
    fn evaluate(
        /// JavaScript expression to evaluate.
        expression: String,
        /// Await the result if it is a promise. Default true.
        await_promise: Option<bool>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Script)?;
            let context = c.context().to_string();
            let res = c
                .send(
                    "script.evaluate",
                    serde_json::json!({
                        "expression": expression,
                        "target": { "context": context },
                        "awaitPromise": await_promise.unwrap_or(true),
                    }),
                )
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    #[act_tool(
        description = "Extract visible text from the page, or from one element by CSS selector.",
        read_only
    )]
    fn get_text(
        /// CSS selector. Defaults to the whole document body.
        selector: Option<String>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<String> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Read)?;
            gate(c, BrowserCap::Script)?;
            let context = c.context().to_string();
            let expr = match selector {
                Some(ref s) => format!(
                    "(document.querySelector({})||{{}}).innerText ?? ''",
                    serde_json::to_string(s).unwrap_or_else(|_| "null".into())
                ),
                None => "document.body.innerText".to_string(),
            };
            let res = c
                .send(
                    "script.evaluate",
                    serde_json::json!({
                        "expression": expr,
                        "target": { "context": context },
                        "awaitPromise": false,
                    }),
                )
                .map_err(ActError::internal)?;
            Ok(res
                .get("result")
                .and_then(|r| r.get("value"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string())
        })
    }

    #[act_tool(
        description = "Click the element matching a CSS selector. \
                       Requires browser:input and browser:script."
    )]
    fn click(
        /// CSS selector of the element to click.
        selector: String,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Input)?;
            gate(c, BrowserCap::Script)?;
            let shared_id = resolve_node(c, &selector)?;
            let context = c.context().to_string();
            let res = c
                .send(
                    "input.performActions",
                    serde_json::json!({
                        "context": context,
                        "actions": click_actions(&shared_id),
                    }),
                )
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    #[act_tool(
        description = "Focus the element matching a CSS selector and type text into it. \
                       Requires browser:input and browser:script."
    )]
    fn type_text(
        /// CSS selector of the element to type into.
        selector: String,
        /// Text to type.
        text: String,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Input)?;
            gate(c, BrowserCap::Script)?;
            let shared_id = resolve_node(c, &selector)?;
            let context = c.context().to_string();

            // Focus by clicking, then dispatch key actions.
            c.send(
                "input.performActions",
                serde_json::json!({
                    "context": context,
                    "actions": click_actions(&shared_id),
                }),
            )
            .map_err(ActError::internal)?;

            let keys: Vec<serde_json::Value> = text
                .chars()
                .flat_map(|ch| {
                    let s = ch.to_string();
                    [
                        serde_json::json!({ "type": "keyDown", "value": s }),
                        serde_json::json!({ "type": "keyUp", "value": s }),
                    ]
                })
                .collect();

            let res = c
                .send(
                    "input.performActions",
                    serde_json::json!({
                        "context": context,
                        "actions": [{ "type": "key", "id": "keyboard", "actions": keys }]
                    }),
                )
                .map_err(ActError::internal)?;
            json_to_cbor(&res)
        })
    }

    // ── Console ───────────────────────────────────────────────────────────────

    #[act_tool(
        description = "Drain buffered console and log entries captured since the last call. \
                       Reports how many entries were dropped to stay within the buffer bound.",
        read_only
    )]
    fn console_drain(
        /// Maximum entries to return. Defaults to all buffered.
        max: Option<u32>,
        ctx: &mut ActContext<ToolMeta>,
    ) -> ActResult<Cv> {
        let id = require_session(ctx)?;
        with_session_mut(&id, |c| {
            gate(c, BrowserCap::Read)?;
            let d = c.drain_log(max.map(|n| n as usize));
            let as_json = serde_json::to_value(&d)
                .map_err(|e| ActError::internal(format!("serialize log: {e}")))?;
            json_to_cbor(&as_json)
        })
    }
}
