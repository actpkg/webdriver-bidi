wasm := "target/wasm32-wasip2/release/component_webdriver_bidi.wasm"
# OCI reference to publish to (registry/namespace/name, no tag). Override with OCI_REF.
component_ref := env("OCI_REF", "ghcr.io/actpkg/webdriver-bidi")

act := env("ACT", "act")
actbuild := env("ACT_BUILD", "act-build")
hurl := env("HURL", "hurl")
# Random ports for the e2e servers, above common dev ports and below the Linux
# outbound ephemeral range (32768+).
port := `shuf -i 10000-19999 -n 1`
addr := "[::1]:" + port
baseurl := "http://" + addr
port2 := `shuf -i 20000-29999 -n 1`
addr2 := "[::1]:" + port2
baseurl2 := "http://" + addr2

# No-op: the WIT dependencies are committed under wit/deps/ rather than
# fetched. This component was scaffolded outside the copier template, targets
# act:tools@0.2.0 / act:sessions@0.2.0, and carries no wkg registry mapping —
# so there is nothing for a fetch step to resolve. The three package.wit files
# total 20 KB; act-sdk-rs commits its deps the same way. The recipe stays so
# that CI and the documented `just init && just build` flow keep working.
init:
    @true

# Build and pack. Packing is part of building on purpose: `cargo build` alone
# produces a wasm with no `act:component` section, which declares no capability
# ceiling, so at runtime every grant is refused as "outside ceiling" and the
# failure points anywhere but at the missing metadata.
build:
    cargo build --release
    {{actbuild}} pack {{wasm}}

# Re-embed act:component metadata and act:skill without rebuilding. `pack` is
# idempotent, so running it after `build` is harmless.
pack:
    {{actbuild}} pack {{wasm}}

# Unit tests run on the host target, not wasm.
unit:
    cargo test --target x86_64-unknown-linux-gnu

test: unit build
    #!/usr/bin/env bash
    set -euo pipefail
    (cd tests/mock-bidi && npm install --silent)
    BIDI_PORT=9222
    PORT=$BIDI_PORT node tests/mock-bidi/server.mjs &
    MOCK=$!
    # Wait for the mock to accept connections before starting the hosts. Both
    # are launched with --session-args, so `act run` opens a session — and
    # therefore dials this port — during startup: if the mock is not listening
    # yet, startup fails and the HTTP server never binds. Locally node usually
    # wins the race; on a CI runner it does not.
    for _ in $(seq 1 60); do
        (echo > /dev/tcp/127.0.0.1/$BIDI_PORT) >/dev/null 2>&1 && break
        sleep 0.5
    done
    SA_FULL="{\"host\":\"127.0.0.1\",\"port\":$BIDI_PORT}"
    # Grants input but NOT script, so `click` is denied specifically on
    # browser:script — proving the transitive coupling rather than merely
    # failing on the first missing capability.
    SA_RO="{\"host\":\"127.0.0.1\",\"port\":$BIDI_PORT,\"allow\":[\"navigate\",\"read\",\"input\"]}"
    {{act}} run {{wasm}} --http --listen "{{addr}}" --allow wasi:sockets --session-args "$SA_FULL" &
    PID=$!
    {{act}} run {{wasm}} --http --listen "{{addr2}}" --allow wasi:sockets --session-args "$SA_RO" &
    PID2=$!
    trap "kill $PID $PID2 $MOCK 2>/dev/null || true" EXIT
    curl --retry 60 --retry-connrefused --retry-delay 1 -fsS -o /dev/null {{baseurl}}/info
    curl --retry 60 --retry-connrefused --retry-delay 1 -fsS -o /dev/null {{baseurl2}}/info
    # --jobs 1 is required, not incidental: every file shares one session-of-1
    # server, and the console buffer is session state. Under hurl's default
    # parallel execution, one file's commands generate log events that race with
    # another file's "buffer is now empty" assertion.
    {{hurl}} --test --jobs 1 --variable "baseurl={{baseurl}}" e2e/info.hurl e2e/list_tools.hurl e2e/dom.hurl e2e/navigate_console.hurl
    {{hurl}} --test --jobs 1 --variable "baseurl2={{baseurl2}}" e2e/caps_denied.hurl

publish: build
    #!/usr/bin/env bash
    set -euo pipefail
    INFO=$({{act}} inspect component-manifest {{wasm}})
    VERSION=$(echo "$INFO" | jq -r .std.version)
    OUTPUT=$({{actbuild}} push {{wasm}} "{{component_ref}}:$VERSION" \
      --skip-if-exists \
      --also-tag latest 2>&1) || { echo "$OUTPUT" >&2; exit 1; }
    echo "$OUTPUT"
    DIGEST=$(echo "$OUTPUT" | grep "^Digest:" | awk '{print $2}' || true)
    if [ -n "${GITHUB_OUTPUT:-}" ]; then
      echo "image={{component_ref}}" >> "$GITHUB_OUTPUT"
      echo "digest=$DIGEST" >> "$GITHUB_OUTPUT"
    fi

# Opt-in: requires a local Firefox. Not run in CI.
#
# Firefox is the target because it implements WebDriver BiDi natively. Chromium
# exposes CDP on --remote-debugging-port and needs the chromium-bidi wrapper to
# speak BiDi, so it will not work here unaided.
test-browser: build
    #!/usr/bin/env bash
    set -euo pipefail
    PROFILE=$(mktemp -d)
    firefox --headless --no-remote --profile "$PROFILE" --remote-debugging-port=9222 &
    BROWSER=$!
    trap "kill $BROWSER 2>/dev/null || true; rm -rf $PROFILE" EXIT
    # Firefox needs several seconds before the BiDi endpoint accepts connections.
    for i in $(seq 30); do
      ss -ltn 2>/dev/null | grep -q 127.0.0.1:9222 && break
      sleep 1
    done
    SA='{"host":"127.0.0.1","port":9222,"timeout_ms":45000}'
    {{act}} call {{wasm}} navigate --args '{"url":"https://example.com"}' \
      --allow wasi:sockets --session-args "$SA"
    {{act}} call {{wasm}} get_text --args '{}' \
      --allow wasi:sockets --session-args "$SA"
    {{act}} call {{wasm}} evaluate --args '{"expression":"document.title"}' \
      --allow wasi:sockets --session-args "$SA"
