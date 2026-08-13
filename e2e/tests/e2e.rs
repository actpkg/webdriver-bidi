//! Drive the packed component through `act run --mcp` with a real MCP client.
//!
//! Rust translation of the python fastmcp/pytest suite that still lives in
//! this directory (kept for reference); the tests observe exactly what an
//! agent observes, over the same client stack (`rmcp`) the host bridge itself
//! is built on.
//!
//! webdriver-bidi is a session-provider. The suite drives the MCP bridge's
//! *virtual* `open_session` / `close_session` tools during normal serving
//! (ACT-MCP §4.1) — the host binds its listener with nothing pre-opened and
//! each test opens its own session(s) after the connection is already up, so
//! a stall inside `open-session` cannot wedge a pre-bound port the way the
//! old `--session-args` pattern did. Real tool calls address the session
//! through the argument `_meta` channel — `{"_meta": {"std:session-id": sid}}`
//! inside `arguments` — which is ordinary JSON there and keeps its `std:`
//! spelling (only response metadata gets the `dev.actcore/*` respelling).
//!
//! The browser is faked by `tests/mock-bidi/server.mjs`, a Node WebSocket
//! stub that deliberately interleaves an unsolicited `log.entryAdded` event
//! before *every* command response, so the component's demux is actually
//! exercised rather than passed by a naive in-order implementation. One
//! instance serves the whole suite on the fixed loopback port the component's
//! ceiling allows (127.0.0.1:9222), exactly as the python conftest's
//! session-scoped `mock_bidi` fixture did.
//!
//! Env: WASM — path to the packed component (default: the component's
//!      release build output);
//!      ACT  — the act invocation (default `act`; whitespace-split);
//!      SKIP_MOCK_BIDI — set to skip spawning the mock; a CI-hang bisect
//!      switch inherited from the python conftest, never set by `just test`.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::{
    ServiceExt,
    model::CallToolRequestParams,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

/// `().serve(transport)` hands back the client-role service running over the
/// child process: role first, the unit client handler second.
type Client = rmcp::service::RunningService<rmcp::service::RoleClient, ()>;

const BIDI_HOST: &str = "127.0.0.1";
const BIDI_PORT: u16 = 9222;

/// Deliberately loose — the python conftest's CONNECT_TIMEOUT, kept at the
/// same value on purpose. `act run --mcp` instantiates the component before
/// it answers `initialize`, so "connect" includes that cost, and this
/// component's CI history is precisely a hang that only a bounded connect
/// turned into a diagnostic instead of a dead port.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);

fn wasm_path() -> PathBuf {
    let path = PathBuf::from(std::env::var("WASM").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../target/wasm32-wasip2/release/component_webdriver_bidi.wasm"
        )
        .into()
    }));
    ensure_packed(&path);
    path
}

/// The python `wasm_path` fixture's probe, run once per process. Existence is
/// not enough and neither is a fresh mtime: `cargo build` produces a wasm
/// with no `act:component` custom section, and an unpacked artifact declares
/// no capability ceiling, so every grant is refused as "outside ceiling" and
/// the failures point anywhere but here. This has already bitten this
/// workspace repeatedly. The justfile's `test: build` ordering exists so
/// this check passes.
fn ensure_packed(path: &Path) {
    static CHECKED: OnceLock<()> = OnceLock::new();
    CHECKED.get_or_init(|| {
        if !path.exists() {
            panic!("{path:?} is missing — run `just build` first");
        }
        let mut cmd = new_std_command();
        cmd.args(["inspect", "component-manifest"]).arg(path);
        let output = cmd.output().expect("run act inspect component-manifest");
        let manifest: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        let name = manifest["std"]["name"].as_str().unwrap_or("unknown");
        if name.is_empty() || name == "unknown" {
            panic!("{path:?} is built but not packed — run `just pack`");
        }
    });
}

/// The ACT invocation, honouring the same override the component justfile
/// uses. Values with spaces (e.g. `npx @actcore/act`) are whitespace-split
/// into program + leading args, like the shlex.split the python conftest did.
fn act_argv() -> Vec<String> {
    std::env::var("ACT")
        .unwrap_or_else(|_| "act".into())
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn new_std_command() -> std::process::Command {
    let argv = act_argv();
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd
}

/// Spawn `act run <wasm> --mcp` with the grant the python conftest gave.
///
/// Grants are NOT optional: the default policy mode is `ask` and a headless
/// run degrades it to deny. `--allow wasi:sockets` opens the component's full
/// declared ceiling (loopback on the standard WebDriver/DevTools ports,
/// act.toml) and nothing wider — the four `browser:*` classes are declared
/// but self-enforced in-component, so they take no host grant.
fn act_command() -> tokio::process::Command {
    let argv = act_argv();
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.arg("run").arg(wasm_path()).arg("--mcp");
    cmd.args(["--allow", "wasi:sockets"]);
    cmd
}

/// Spawn with stderr captured: the audit trail (refusals, per-call rollup)
/// writes there unconditionally — RUST_LOG never silences it — and the
/// python conftest went out of its way (LOG_FILE, pytest_sessionfinish) to
/// keep that stream diagnosable. The [`ActStderr`] guard reprints it when a
/// test fails, which is that machinery's job here.
fn spawn_with_captured_stderr() -> (TokioChildProcess, Arc<AsyncMutex<String>>) {
    let (transport, stderr) = TokioChildProcess::builder(act_command())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn act run --mcp with piped stderr");

    let captured = Arc::new(AsyncMutex::new(String::new()));
    let sink = captured.clone();
    let mut lines = BufReader::new(stderr.expect("stderr was piped")).lines();
    tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            let mut buf = sink.lock().await;
            buf.push_str(&line);
            buf.push('\n');
        }
    });

    (transport, captured)
}

/// Reprints the captured audit trail if the test is unwinding — on an
/// ephemeral CI runner nothing would otherwise ever read it. Diagnosing a
/// CI-only hang in this fleet cost several rounds of probing that one line
/// of this stream would have answered.
struct ActStderr(Arc<AsyncMutex<String>>);

impl Drop for ActStderr {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        // The drain task holds the lock only per line; a short retry is
        // enough to catch a quiet moment.
        for _ in 0..20 {
            if let Ok(buf) = self.0.try_lock() {
                eprintln!("--- act stderr ---\n{}", buf);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("--- act stderr: buffer busy, not dumped ---");
    }
}

/// A connected MCP client, one `act` process per test: the component holds
/// live connections and per-session console buffers in a per-process session
/// registry, so sharing one process across tests would let a session opened
/// by one test leak into another. The connect — not the test body — is
/// bounded, so a stalled handshake produces a diagnostic of its own.
async fn connect() -> (Client, ActStderr) {
    let (transport, captured) = spawn_with_captured_stderr();
    let client = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .expect(
            "MCP client did not connect within 120s; act's stderr is dumped by the failure guard",
        )
        .expect("rmcp handshake with act run --mcp");
    (client, ActStderr(captured))
}

// ── the mock-bidi server ─────────────────────────────────────────────────────

/// pid of the spawned mock, read by the atexit handler.
static MOCK_PID: AtomicI32 = AtomicI32::new(-1);

/// Registered with `atexit` when the mock is spawned: the Rust analogue of
/// the python fixture's `finally: proc.terminate()`. SIGTERM is what
/// `proc.terminate()` sent; the node stub handles it (closes the server,
/// exits) and the default disposition kills it either way.
extern "C" fn kill_mock_at_exit() {
    let pid = MOCK_PID.load(Ordering::SeqCst);
    if pid > 0 {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
}

struct MockGuard;

static MOCK: OnceLock<Arc<MockGuard>> = OnceLock::new();

/// The python conftest's session-scoped `mock_bidi` fixture: start the Node
/// WebDriver BiDi stub once for the whole suite, wait until its port accepts
/// a connection, and terminate it at process exit. Synchronous by design —
/// the spawn and the port probe are plain blocking calls, and every caller
/// would block on the same [`OnceLock`] until the mock is up anyway (the
/// python autouse fixture serialised its tests the same way).
fn mock_bidi() -> Arc<MockGuard> {
    MOCK.get_or_init(|| {
        if std::env::var_os("SKIP_MOCK_BIDI").is_some() {
            return Arc::new(MockGuard);
        }
        let mock_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/mock-bidi");
        let mut cmd = std::process::Command::new("node");
        cmd.arg("server.mjs").current_dir(&mock_dir);
        cmd.env("PORT", BIDI_PORT.to_string());
        // stdout/stderr devnull'd, as the python fixture did: the stub's own
        // connection diagnostics are not test output.
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = cmd.spawn().expect(
            "spawn node tests/mock-bidi/server.mjs — is `npm install` run in \
             tests/mock-bidi? (`just test` does it)",
        );
        MOCK_PID.store(child.id() as i32, Ordering::SeqCst);
        unsafe { libc::atexit(kill_mock_at_exit) };
        // The pid is all atexit needs; the handle is never polled again.
        std::mem::forget(child);
        wait_for_port(BIDI_HOST, BIDI_PORT, Duration::from_secs(30));
        Arc::new(MockGuard)
    })
    .clone()
}

fn wait_for_port(host: &str, port: u16, timeout: Duration) {
    let addr: SocketAddr = format!("{host}:{port}").parse().expect("mock addr");
    let deadline = std::time::Instant::now() + timeout;
    let mut last: Option<std::io::Error> = None;
    while std::time::Instant::now() < deadline {
        match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
            Ok(_) => return,
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    panic!(
        "{host}:{port} did not accept connections within {timeout:?} ({last:?})"
    );
}

// ── tool-call plumbing ───────────────────────────────────────────────────────

async fn call(client: &Client, tool: &str, arguments: Value) -> rmcp::model::CallToolResult {
    client
        .call_tool(
            CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .expect("call_tool")
}

fn structured(result: &rmcp::model::CallToolResult) -> &Value {
    result
        .structured_content
        .as_ref()
        .expect("a successful tool result must carry structured content")
}

fn first_text_block(result: &rmcp::model::CallToolResult) -> &rmcp::model::TextContent {
    match result.content.first() {
        Some(rmcp::model::ContentBlock::Text(t)) => t,
        other => panic!("expected the first content block to be Text, got: {other:?}"),
    }
}

/// The python conftest's `with_session` fixture: merge a session id into a
/// tool call's arguments via the argument metadata channel, keeping the
/// `std:` spelling.
fn with_session(session_id: &str, arguments: Value) -> Value {
    let mut map = arguments.as_object().cloned().unwrap_or_default();
    map.insert("_meta".into(), json!({ "std:session-id": session_id }));
    Value::Object(map)
}

/// The kind and message of a failed call may arrive on either path: as a
/// JSON-RPC error response (`ErrorData.data` / `message`) or as an isError
/// result (`_meta` / first text content). The python conftest's
/// `expect_error` fixture handled both; so does this. `call-tool` has no
/// `result<>` wrapper, so a guest reporting a failed call can only do it
/// through `tool-event::error` — the isError path — while the JSON-RPC path
/// exists for failures that are not the guest's tool body: `list-tools`, the
/// session operations, a wasmtime trap, an unreachable actor.
async fn error_kind_of(
    client: &Client,
    params: CallToolRequestParams,
) -> Option<(String, String)> {
    match client.call_tool(params).await {
        Err(rmcp::ServiceError::McpError(e)) => {
            let kind = e
                .data
                .as_ref()
                .and_then(|d| d.get("dev.actcore/error-kind"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            kind.map(|k| (k, e.message.to_string()))
        }
        Ok(result) => {
            assert_eq!(result.is_error, Some(true), "call must fail: {result:?}");
            let kind = result
                .meta
                .as_ref()
                .and_then(|m| m.0.get("dev.actcore/error-kind"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let message = result
                .content
                .first()
                .and_then(|b| match b {
                    rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            kind.map(|k| (k, message))
        }
        Err(other) => panic!("unexpected transport failure: {other:?}"),
    }
}

/// The python conftest's `expect_error` fixture: assert a call fails with a
/// specific ACT error kind, and optionally a substring of the human-readable
/// error message.
async fn expect_error(
    client: &Client,
    tool: &str,
    arguments: Value,
    kind: &str,
    message_contains: Option<&str>,
) {
    let params =
        CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone());
    let Some((actual_kind, message)) = error_kind_of(client, params).await else {
        panic!("expected {tool} to fail with {kind}, but no named error kind came back");
    };
    assert_eq!(
        actual_kind, kind,
        "expected {kind}, got {actual_kind} ({message:?})"
    );
    if let Some(needle) = message_contains {
        assert!(message.contains(needle), "expected {needle:?} in {message:?}");
    }
}

/// Call the virtual `open_session` tool. Its argument shape is the
/// component's `get-open-session-args-schema` directly — no wrapper key —
/// and its result is a JSON object in `content[0].text`, carrying
/// `{"id": ..., "metadata": {...}}` (ACT-MCP §4.1). This is NOT
/// `structured_content`: the synthesized session tools bypass the normal
/// tool-result folding a real tool call goes through.
async fn open_session(client: &Client, allow: Option<&[&str]>) -> String {
    let mut args = json!({ "host": BIDI_HOST, "port": BIDI_PORT });
    if let Some(allow) = allow {
        args["allow"] = json!(allow);
    }
    let result = call(client, "open_session", args).await;
    assert_ne!(result.is_error, Some(true), "open_session failed: {result:?}");
    let reply: Value =
        serde_json::from_str(&first_text_block(&result).text).expect("reply is JSON");
    reply["id"]
        .as_str()
        .expect("open_session reply carries an id")
        .to_string()
}

/// `close_session`'s one argument is `session_id` itself, a plain top-level
/// key — it is the object of the close, not contextual metadata, unlike
/// `std:session-id` on every other tool call.
async fn close_session(client: &Client, session_id: &str) {
    let result = call(client, "close_session", json!({ "session_id": session_id })).await;
    assert_ne!(result.is_error, Some(true), "close_session failed: {result:?}");
}

// ── test_info.py ─────────────────────────────────────────────────────────────

#[test]
fn test_manifest_reports_name_and_capabilities() {
    let wasm = wasm_path();
    let mut cmd = new_std_command();
    cmd.args(["inspect", "component-manifest"]).arg(&wasm);
    let output = cmd.output().expect("run act inspect component-manifest");
    assert!(
        output.status.success(),
        "inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: Value = serde_json::from_slice(&output.stdout).expect("manifest is JSON");
    assert_eq!(manifest["std"]["name"], "webdriver-bidi");
    let caps = manifest["std"]["capabilities"]
        .as_object()
        .expect("manifest must declare capabilities");
    assert!(
        caps.contains_key("wasi:sockets"),
        "wasi:sockets must be a declared capability, got: {caps:?}"
    );
    assert!(
        caps.contains_key("browser:script"),
        "browser:script must be a declared capability, got: {caps:?}"
    );
}

// ── test_list_tools.py ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_list_tools_includes_core_tools() {
    let _mock = mock_bidi();
    let (client, _stderr) = connect().await;
    let tools = client.list_all_tools().await.expect("list_all_tools");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for n in ["navigate", "console_drain", "click", "screenshot"] {
        assert!(
            names.contains(&n),
            "`{n}` must be among the tools, got: {names:?}"
        );
    }
    client.cancel().await.ok();
}

// ── test_navigate_console.py ─────────────────────────────────────────────────

#[tokio::test]
async fn test_navigate_then_drain_console() {
    let _mock = mock_bidi();
    let (client, _stderr) = connect().await;
    let sid = open_session(&client, None).await;

    // navigate succeeds, and the events the mock interleaved before each
    // response are buffered rather than mistaken for the command response
    // itself.
    let result = call(
        &client,
        "navigate",
        with_session(&sid, json!({ "url": "https://example.com" })),
    )
    .await;
    assert_ne!(
        result.is_error,
        Some(true),
        "navigate failed: {:?}",
        first_text_block(&result).text
    );
    let parsed = structured(&result);
    assert_eq!(parsed["url"], "https://example.com");
    assert_eq!(parsed["navigation"], "nav-1");

    let result = call(&client, "console_drain", with_session(&sid, json!({}))).await;
    assert_ne!(
        result.is_error,
        Some(true),
        "console_drain failed: {:?}",
        first_text_block(&result).text
    );
    let parsed = structured(&result);
    let entries = parsed["entries"]
        .as_array()
        .expect("entries must be a list");
    assert!(!entries.is_empty(), "expected buffered entries");
    assert_eq!(
        parsed["dropped"].as_u64(),
        Some(0),
        "nothing should have been dropped from the buffer yet"
    );
    assert_eq!(entries[0]["method"], "log.entryAdded");

    // Second drain is empty — the buffer was consumed by the first.
    let result = call(&client, "console_drain", with_session(&sid, json!({}))).await;
    assert_ne!(
        result.is_error,
        Some(true),
        "console_drain failed: {:?}",
        first_text_block(&result).text
    );
    let entries = structured(&result)["entries"]
        .as_array()
        .expect("entries must be a list");
    assert_eq!(entries.len(), 0, "the buffer was consumed by the first drain");

    close_session(&client, &sid).await;
    client.cancel().await.ok();
}

// ── test_dom.py ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_text_and_click() {
    let _mock = mock_bidi();
    let (client, _stderr) = connect().await;
    let sid = open_session(&client, None).await;

    // get_text goes through script.evaluate and returns text/plain.
    let result = call(&client, "get_text", with_session(&sid, json!({}))).await;
    assert_ne!(
        result.is_error,
        Some(true),
        "get_text failed: {:?}",
        first_text_block(&result).text
    );
    let block = first_text_block(&result);
    assert_eq!(block.text, "mock text");
    let meta = block
        .meta
        .as_ref()
        .expect("the text block must carry _meta");
    assert_eq!(
        meta.0.get("dev.actcore/mime-type").and_then(Value::as_str),
        Some("text/plain")
    );

    // click is two-step: resolve the selector to a node handle via
    // script.evaluate, then dispatch pointer actions against that element
    // origin. The old hurl file asserted only HTTP 200 here (no jsonpath) —
    // preserved as "the call succeeds", the same claim in test terms.
    let result = call(
        &client,
        "click",
        with_session(&sid, json!({ "selector": "#go" })),
    )
    .await;
    assert_ne!(
        result.is_error,
        Some(true),
        "click failed: {:?}",
        first_text_block(&result).text
    );

    close_session(&client, &sid).await;
    client.cancel().await.ok();
}

// ── test_caps_denied.py ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_restricted_session_denies_script_transitively() {
    let _mock = mock_bidi();
    let (client, _stderr) = connect().await;
    // The restricted session is granted navigate + read + input, but NOT script.
    let sid = open_session(&client, Some(&["navigate", "read", "input"])).await;

    expect_error(
        &client,
        "evaluate",
        with_session(&sid, json!({ "expression": "1+1" })),
        "std:capability-denied",
        Some("browser:script"),
    )
    .await;

    // click transitively needs browser:script to resolve the selector to a
    // node handle, so it is denied even though browser:input IS granted.
    // Asserting the message names browser:script is the point: it proves the
    // coupling documented in the spec rather than just failing on the first
    // missing capability.
    expect_error(
        &client,
        "click",
        with_session(&sid, json!({ "selector": "#go" })),
        "std:capability-denied",
        Some("browser:script"),
    )
    .await;

    // Navigation is granted and still works.
    let result = call(
        &client,
        "navigate",
        with_session(&sid, json!({ "url": "https://example.com" })),
    )
    .await;
    assert_ne!(
        result.is_error,
        Some(true),
        "navigate failed: {:?}",
        first_text_block(&result).text
    );
    let parsed = structured(&result);
    assert_eq!(parsed["url"], "https://example.com");

    close_session(&client, &sid).await;
    client.cancel().await.ok();
}
