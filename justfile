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

# Fetch WIT deps into wit/deps/.
init:
    act-build init

build:
    cargo build --release

# Embed act:component metadata and act:skill into the wasm.
pack: build
    {{actbuild}} pack {{wasm}}

# Unit tests run on the host target, not wasm.
unit:
    cargo test --target x86_64-unknown-linux-gnu

test: pack
    {{act}} call {{wasm}} --help
