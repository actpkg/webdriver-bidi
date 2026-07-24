# webdriver-bidi

An ACT component that drives a locally-running browser over the
[WebDriver BiDi](https://w3c.github.io/webdriver-bidi/) protocol: navigation,
DOM interaction, screenshots, and console capture.

It attaches to a browser that is **already running**. It cannot launch one —
wasm components cannot spawn processes. Browser lifecycle is the caller's job.

**Use Firefox**: it implements WebDriver BiDi natively. Chromium's
`--remote-debugging-port` speaks CDP, not BiDi, and needs the `chromium-bidi`
wrapper in front of it.

```bash
firefox --headless --remote-debugging-port=9222 &

act call webdriver-bidi.wasm navigate \
  --args '{"url":"https://example.com"}' \
  --allow wasi:sockets \
  --session-args '{"host":"127.0.0.1","port":9222,"timeout_ms":45000}'
```

Verified against Firefox: navigation, text extraction, `document.title`
evaluation, element clicks, and a 1366×682 PNG screenshot.

## Tools

| Tool | Purpose |
|---|---|
| `navigate(url, wait?)` | Load a URL |
| `get_text(selector?)` | Extract visible text |
| `evaluate(expression, await_promise?)` | Run JavaScript, return its value |
| `click(selector)` | Click an element |
| `type_text(selector, text)` | Focus an element and type |
| `screenshot()` | PNG bytes |
| `context_list()` / `context_create(type?)` / `context_close(context?)` | Tabs and windows |
| `console_drain(max?)` | Buffered console entries plus a `dropped` count |

## Capability classes

`browser:navigate`, `browser:script`, `browser:input`, `browser:read` — declared
in `act.toml` and narrowed per-session via the `allow` open-session argument.

`click` and `type_text` require **both** `browser:input` and `browser:script`.
BiDi dispatches input against a resolved element, and resolving a CSS selector
goes through script evaluation, so denying `browser:script` also disables
clicking and typing.

## Scope and audit position

This component connects only to **loopback plaintext `ws://`** endpoints. It
contains no TLS stack, no bundled root certificates, and cannot reach remote
`wss://` BiDi services.

That scope has an honest consequence worth stating precisely. Because frames
cross `wasi:sockets` as plaintext BiDi JSON rather than ciphertext, a host-side
inspector *could* parse them and reconstruct semantic actions — something that
is impossible when a guest terminates TLS itself.

**This is a property of the design, not a feature that exists.** `act-cli` does
not parse BiDi frames today. The enforced capability ceiling constrains *which
endpoint is dialled*, not *what is done through it*. Anything within that
ceiling is reachable unobserved until such an inspector is built.

Because a browser visits arbitrary sites carrying user credentials, treat this
as the most sensitive component in the catalogue and grant it narrowly.

## Development

```bash
just init     # fetch WIT deps into wit/deps/
just unit     # unit tests, host target
just test     # unit tests + browserless e2e against a mock BiDi server
just pack     # embed act:component and act:skill metadata
```

`just test` needs no browser: `tests/mock-bidi/` is a small `ws` server that
speaks enough BiDi for the suite. It deliberately emits an unsolicited event
before every command response, so an implementation that naively reads one
frame per command fails rather than passing by luck.

A real-browser check is opt-in and not part of CI. It drives a headless Firefox
through navigate → get_text → evaluate:

```bash
just test-browser
```

## License

MIT OR Apache-2.0
