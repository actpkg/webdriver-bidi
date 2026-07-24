---
name: webdriver-bidi
description: Drive a locally-running browser over WebDriver BiDi — navigate, read page text, click, type, and capture console output.
metadata:
  act: {}
---

# webdriver-bidi

Drives a browser that is **already running** with a BiDi endpoint exposed. This
component cannot launch a browser: wasm components cannot spawn processes.

Start one first. **Firefox implements WebDriver BiDi natively**, so it works
unaided:

    firefox --headless --remote-debugging-port=9222

Chromium exposes CDP on that flag, not BiDi, and needs the `chromium-bidi`
wrapper in front of it. Prefer Firefox unless you have that wrapper running.

Firefox takes a few seconds to open the endpoint; allow a generous
`timeout_ms` (45000 is comfortable) on the first session.

## Opening a session

Every tool needs a session. Open one with `host` and `port`:

    { "host": "127.0.0.1", "port": 9222 }

Only loopback IP literals are accepted. Hostnames — including `localhost` — are
rejected by design: resolving them would open a DNS-rebinding path to a
non-loopback address.

Narrow what the session may do with `allow`:

    { "host": "127.0.0.1", "port": 9222, "allow": ["navigate", "read"] }

## Tools

- `navigate(url, wait?)` — load a URL; `wait` is `none` | `interactive` | `complete`
- `get_text(selector?)` — extract visible text, whole body by default
- `evaluate(expression, await_promise?)` — run JavaScript, return its value
- `click(selector)` / `type_text(selector, text)` — interact with an element
- `screenshot()` — PNG bytes
- `context_list()` / `context_create(type?)` / `context_close(context?)` — tabs and windows
- `console_drain(max?)` — buffered console entries plus a `dropped` count

## Capability classes

`browser:navigate`, `browser:script`, `browser:input`, `browser:read`.

`click` and `type_text` need **both** `browser:input` and `browser:script`: BiDi
dispatches input against a resolved element, and resolving a selector goes
through script evaluation. Denying `browser:script` therefore also disables
clicking and typing — expect that, it is not a bug.

## Console buffer

Console entries accumulate in a bounded buffer (`log_buffer_cap`, default 1000)
and are removed when drained. If entries were discarded to stay within the
bound, `console_drain` reports the count in `dropped` — a non-zero value means
you are draining too slowly, not that the page went quiet.
