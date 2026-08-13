"""Shared fixtures for the MCP-driven e2e suite.

The suite drives the packed component through `act run --mcp` over stdio with
a real MCP client, so what the tests observe is what an agent observes.

`webdriver-bidi` is a session-provider. The old suite pre-opened one session
per host process via `act run --session-args ...` — two processes, one per
capability set (full, and navigate+read+input but not script). That pattern
is what is hanging this component's host in CI today: opening the session
happens *before* the listener binds, so a stall inside `open-session` means
the port never comes up, and there is no diagnostic — just a dead port and
sixty failed curl retries. (`postgres` was red for the identical reason; this
is the same fix.)

This suite instead drives the MCP bridge's *virtual* `open_session` /
`close_session` tools during normal serving (ACT-MCP §4.1): the host binds
its listener with nothing pre-opened, and each test opens its own session(s)
after the connection is already up. That failure mode cannot happen here.
Session-of-1 itself (the `--session-args` path) is already covered by
act-cli's own `session_of_1_mcp.rs`, so nothing is lost by not re-testing it.
"""

import asyncio
import json
import os
import shlex
import socket
import subprocess
import time
import pytest
from contextlib import AsyncExitStack
from pathlib import Path

from fastmcp import Client
from fastmcp.client.transports import StdioTransport

# Measured in docs/specs/2026-08-08-e2e-harness-findings.md, question 1.
from mcp.shared.exceptions import McpError

WASM = "target/wasm32-wasip2/release/component_webdriver_bidi.wasm"
COMPONENT_ROOT = Path(__file__).parent.parent
MOCK_DIR = COMPONENT_ROOT / "tests" / "mock-bidi"

# Matches the old justfile's BIDI_PORT and tests/mock-bidi/server.mjs's
# `PORT` env var default.
BIDI_HOST = "127.0.0.1"
BIDI_PORT = 9222

# ACT's audit trail writes to stderr unconditionally — it is not governed by
# RUST_LOG. Every other migrated suite in this workspace redirects it to a
# file via StdioTransport's `log_file` that nothing on an ephemeral CI runner
# ever reads, so a host that fails to start, or exits early, leaves zero
# trace anywhere. Earlier in this investigation this suite instead dropped
# `log_file` (letting stderr fall through to pytest's own fd-level capture),
# which was enough to rule out four hypotheses via the "Captured stderr"
# block on a `pytest-timeout` failure. This round needs more than that
# capture can promise under a thread-based timeout, plus the process's exit
# status, which pytest has no way to observe at all — so `client` now writes
# to an explicit file again, and the CI job `cat`s it with `if: always()`
# right after `just test`, so it survives even a cancellation.
LOG_FILE = Path(".pytest-act-stderr.log")

# Healthy connects are measured in fractions of a second; this only has
# to be loose enough never to trip on a slow runner.
CONNECT_TIMEOUT = 30


def _wait_for_port(host: str, port: int, timeout: float = 30.0) -> None:
    deadline = time.monotonic() + timeout
    last_error: OSError | None = None
    while time.monotonic() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.5)
            try:
                s.connect((host, port))
                return
            except OSError as e:
                last_error = e
                time.sleep(0.5)
    raise TimeoutError(f"{host}:{port} did not accept connections within {timeout}s ({last_error})")


@pytest.fixture(scope="session", autouse=True)
def mock_bidi():
    """Start the Node WebDriver BiDi mock for the whole suite, wait until its
    port accepts a connection, and terminate it afterwards.

    `npm install` for the mock stays a justfile-level build prerequisite
    (like `cargo build`), but owning the server process itself — start,
    wait-for-port, terminate — belongs here: every test needs it up, and
    unlike `postgres`'s docker-compose Postgres this is a purpose-built test
    double with no lifecycle tooling of its own to defer to.

    `SKIP_MOCK_BIDI=1` exists purely to bisect a CI-only hang under
    investigation (never reproduced locally) — it is not a way to skip a
    slow dependency, and nothing in the normal `just test` path sets it.
    This is the only fixture in the fleet that spawns a long-lived
    `subprocess.Popen` child that outlives every test, started
    autouse/session-scoped before any MCP client exists — a structural
    difference from every green component. Setting the var skips the spawn
    entirely, holding everything else (test selection, grants, wasm) fixed,
    to isolate whether the mock's mere presence — its process, or the
    listening socket it opens — is upstream of the hang. (A first guess at
    *why* — the child's `stdin` inheriting this process's own — was tested
    directly with a real fix attempt and killed: redirecting it to
    `subprocess.DEVNULL` did not clear the hang. See commit history for that
    result; it is not repeated here since the fix was reverted.) Unset (the
    default), behavior is exactly as before this diagnostic was added.
    """
    if os.environ.get("SKIP_MOCK_BIDI"):
        yield
        return
    env = {**os.environ, "PORT": str(BIDI_PORT)}
    proc = subprocess.Popen(
        ["node", "server.mjs"],
        cwd=MOCK_DIR,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_for_port(BIDI_HOST, BIDI_PORT)
        yield
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()


@pytest.fixture(scope="session")
def act_command() -> list[str]:
    """The ACT invocation, honouring the same override the justfile uses.

    Parsed with shlex, not treated as a single path: some components' `act`
    variable defaults to the two-word `npx @actcore/act`, which cannot be
    `argv[0]` for a non-shell `subprocess.run`/`StdioTransport` call. A bare
    `os.environ.get("ACT", "act")` string breaks that default; splitting it
    is what makes both forms actually spawn. (This component's own justfile
    default is the bare `act`/`act-build`, but the fixture is written to the
    general case for consistency with the rest of the fleet.)
    """
    return shlex.split(os.environ.get("ACT", "act"))


@pytest.fixture(scope="session")
def wasm_path(act_command: list[str]) -> Path:
    """The packed component.

    Existence is not enough and neither is a fresh mtime: `cargo build`
    produces a wasm with no `act:component` custom section, and an unpacked
    artifact declares no capability ceiling, so every grant is refused as
    "outside ceiling" and the failures point anywhere but here. This has
    already bitten repeatedly in this workspace, so the fixture checks the
    section rather than the file.
    """
    path = Path(WASM)
    if not path.exists():
        pytest.fail(f"{path} is missing — run `just build` first")
    probe = subprocess.run(
        [*act_command, "inspect", "component-manifest", str(path)],
        capture_output=True, text=True,
    )
    name = json.loads(probe.stdout or "{}").get("std", {}).get("name", "unknown")
    if name in ("", "unknown"):
        pytest.fail(f"{path} is built but not packed — run `just pack`")
    return path


@pytest.fixture
async def client(act_command: list[str], wasm_path: Path):
    """An MCP client granted the component's full declared `wasi:sockets`
    ceiling (act.toml allows loopback on the standard WebDriver/DevTools
    ports), with no session pre-opened — see the module docstring for why.
    Every test that needs a session opens its own via the virtual
    `open_session` tool, through the `session_id`/`restricted_session_id`
    fixtures below.

    Function-scoped, fresh process per test: the component holds live
    connections and per-session console buffers in a per-process session
    registry, so sharing one process across tests would let a session opened
    by one test leak into another.
    """
    transport = StdioTransport(
        command=act_command[0],
        args=[*act_command[1:], "run", str(wasm_path), "--mcp", "--allow", "wasi:sockets"],
        keep_alive=False,
        log_file=LOG_FILE,  # see the module-level comment above LOG_FILE
    )
    async with AsyncExitStack() as stack:
        # Bound the connect, not the test body. A stalled handshake otherwise
        # consumes the whole pytest timeout with no diagnostic at all — which
        # is precisely how the webdriver-bidi CI hang presented for hours.
        try:
            async with asyncio.timeout(CONNECT_TIMEOUT):
                connected = await stack.enter_async_context(Client(transport))
        except TimeoutError:
            pytest.fail(
                f"MCP client did not connect within {CONNECT_TIMEOUT}s; "
                f"act's stderr, if it wrote any, is dumped at session end"
            )
        yield connected


async def _open_session(client, allow: list[str] | None = None) -> str:
    """Call the virtual `open_session` tool.

    Its argument shape is the component's `get-open-session-args-schema`
    directly — no wrapper key — and its result is a JSON object in
    `content[0].text`, carrying `{"id": ..., "metadata": {...}}`
    (ACT-MCP §4.1). This is NOT `structured_content`: the synthesized
    session tools bypass the normal tool-result folding a real tool call
    goes through.
    """
    args: dict = {"host": BIDI_HOST, "port": BIDI_PORT}
    if allow is not None:
        args["allow"] = allow
    result = await client.call_tool("open_session", args)
    return json.loads(result.content[0].text)["id"]


async def _close_session(client, session_id: str):
    # `close_session`'s one argument is `session_id` itself, a plain
    # top-level key — it is the object of the close, not contextual
    # metadata, unlike `std:session-id` on every other tool call.
    await client.call_tool("close_session", {"session_id": session_id})


@pytest.fixture
async def session_id(client):
    """A session with the default (unrestricted) capability set — `allow`
    omitted, so the component grants all four `browser:*` classes. Matches
    the old justfile's first `--session-args` host ("SA_FULL").
    """
    sid = await _open_session(client)
    yield sid
    await _close_session(client, sid)


@pytest.fixture
async def restricted_session_id(client):
    """A session granted `navigate`+`read`+`input` but NOT `script`, matching
    the old justfile's second `--session-args` host ("SA_RO") — used only by
    the transitive-capability-denial test.
    """
    sid = await _open_session(client, allow=["navigate", "read", "input"])
    yield sid
    await _close_session(client, sid)


@pytest.fixture
def with_session():
    """Merge a session id into a tool call's arguments via the argument
    metadata channel: `{"_meta": {"std:session-id": sid}}` inside
    `arguments`, keeping the `std:` spelling. That channel is ordinary JSON
    inside `params.arguments` and is deliberately exempt from the
    `dev.actcore/*` respelling transport-level metadata goes through.
    """

    def _with(session_id: str, **kwargs) -> dict:
        return {**kwargs, "_meta": {"std:session-id": session_id}}

    return _with


@pytest.fixture
def expect_error():
    """Assert a call fails with a specific ACT error kind, and optionally a
    substring of the human-readable error message.

    Exposed as a fixture rather than a plain function so tests never have to
    import from `conftest` — that import only resolves when the test
    directory happens to be on `sys.path`, which is not something to rely on.

    Measured, not assumed. `call-tool` in `act:tools` returns a bare
    `tool-result` with NO `result<>` wrapper — only `list-tools` has one — so
    a guest reporting a failed tool call can only do it through
    `tool-event::error`, which arrives as a result with `is_error` set and the
    kind in `_meta`. **That is the path a tool test will take**, and on that
    path the human message lands in `content[0].text`.

    The JSON-RPC error path exists for failures that are not the guest's tool
    body: `list-tools`, the session operations, a wasmtime trap, an
    unreachable actor. It raises `mcp.shared.exceptions.McpError` with the
    payload at `exc.error.data` and the message at `exc.error.message`. No
    tool test in this suite is expected to reach it, but both are handled
    here so callers need not care.
    """

    async def _expect(client, tool: str, arguments: dict, kind: str, *, message_contains: str | None = None):
        try:
            result = await client.call_tool(tool, arguments, raise_on_error=False)
        except McpError as exc:
            data = getattr(getattr(exc, "error", None), "data", None) or {}
            assert data.get("dev.actcore/error-kind") == kind, (
                f"expected {kind} on the JSON-RPC error path, got {data!r}"
            )
            if message_contains is not None:
                message = getattr(exc.error, "message", "") or ""
                assert message_contains in message, (
                    f"expected message to contain {message_contains!r}, got {message!r}"
                )
            return

        assert result.is_error, f"expected {tool} to fail, got {result!r}"
        meta = result.meta or {}
        assert meta.get("dev.actcore/error-kind") == kind, (
            f"expected {kind} on the isError path, got {meta!r}"
        )
        if message_contains is not None:
            text = result.content[0].text if result.content else ""
            assert message_contains in text, (
                f"expected message to contain {message_contains!r}, got {text!r}"
            )

    return _expect


def pytest_sessionfinish(session, exitstatus):
    """Print act's stderr when the run did not pass.

    `log_file` keeps the audit trail out of the test output, which is right
    for a green run and wrong for every other kind: on an ephemeral CI runner
    nothing ever reads that file. Diagnosing a CI-only hang in this fleet
    cost several rounds of probing that one line of this stream would have
    answered. A hook rather than a fixture finaliser on purpose — fixture
    teardown does not run when the session dies mid-test.
    """
    if exitstatus == 0 or not LOG_FILE.exists():
        return
    text = LOG_FILE.read_text(errors="replace").strip()
    if text:
        print(f"\n--- act stderr ({LOG_FILE}) ---\n{text}")
