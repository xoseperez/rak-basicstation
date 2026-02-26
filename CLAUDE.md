# CLAUDE.md — Developer Notes for AI Assistants

## Build & test

```sh
cargo build
cargo test
cargo clippy --no-deps
```

Requires Rust 1.89+ (edition 2024). Cross-compilation uses `cross` (see `Makefile` and
`Cross.toml`). Do not run `make` or `cross` unless the user explicitly asks.

Full architecture, functional requirements, and supported targets are documented in
`docs/PRD.md`.

## Configuration changes — mandatory rule

**Every field added to or removed from any struct in `src/config.rs` must also be reflected
in the Handlebars template inside `src/cmd/configfile.rs`.**

That template is what the `configfile` subcommand prints as an example config. New fields
are invisible to users unless the template is updated. Always update both files in the same
change.

## Key files

| File | Purpose |
|---|---|
| `src/config.rs` | TOML config structs (`Configuration`, `Backend`, `Concentratord`, …) |
| `src/cmd/configfile.rs` | Handlebars template for the `configfile` subcommand output |
| `src/lns/mod.rs` | LNS connection loop, global state, `send_uplink()`, context cache |
| `src/lns/uplink.rs` | Converts `gw::UplinkFrame` → BasicStation JSON; assembles `xtime` |
| `src/lns/downlink.rs` | Converts BasicStation `dnmsg` → `gw::DownlinkFrame` |
| `src/lns/websocket.rs` | WebSocket lifecycle, writer task, reconnect loop |
| `src/lns/router_config.rs` | Applies `router_config` (channel plan, DR table) from LNS |
| `src/lns/discovery.rs` | Router discovery (`GET /router-info`), EUI-64 → ID6 |
| `src/lns/timesync.rs` | GPS time-sync request/response handling |
| `src/backend/concentratord.rs` | ZMQ event/command sockets |
| `src/backend/semtech_udp/mod.rs` | Semtech UDP packet forwarder backend |
| `src/cups/mod.rs` | CUPS update loop (HTTPS, credential persistence) |
| `src/metadata.rs` | Static and command-sourced gateway metadata attached to uplinks |
| `openwrt/rak-basicstation/files/rak-basicstation.sh` | UCI → TOML config generator; called by the init script on every start/reload |
| `openwrt/rak-basicstation/files/rak-basicstation.config` | Default UCI config template installed to `/etc/config/rak-basicstation` |
| `openwrt/luci-app-rak-basicstation/htdocs/luci-static/resources/view/rak/rak-basicstation.js` | LuCI view (tabbed form for Backend, LNS, CUPS) |

## xtime encoding

```
xtime bits: [62:56] radio_unit=0 | [55:48] session counter | [47:0] count_us
```

Assembled in `lns/uplink.rs::build_upinfo()`. Echoed back unchanged by the LNS in `dnmsg`.
The session counter is an 8-bit value incremented on each reconnect (`SESSION_COUNTER`).

## Integration testing

`examples/fake_concentratord.rs` simulates the Concentratord ZMQ API without hardware.
Run with `cargo run --example fake_concentratord`. It responds to `GetGatewayId`,
`SetGatewayConfiguration`, and `SendDownlinkFrame` commands, and publishes synthetic uplink
frames and gateway stats at a configurable interval.

## MIPSEL builds

MIPSEL (`mipsel-unknown-linux-musl`) is a tier-3 Rust target requiring nightly
(`nightly-2026-01-27`) and `-Z build-std=panic_abort,std`. Both backends
(`concentratord` and `semtech_udp`) are built by default. The `zmq` crate compiles
`libzmq` from its bundled source via cmake — no pre-installed system library is needed
(same approach used by chirpstack-concentratord for its own MIPSEL builds).

The Makefile target `build-mipsel-unknown-linux-musl` handles this automatically,
including the required `rustup toolchain add` step.

## Context caching (`context_caching`)

- Controlled by `backend.concentratord.context_caching` (default `false`).
- When enabled: `send_uplink()` stores the full `rx_info.context` blob in `CONTEXT_CACHE`
  keyed by the reconstructed `xtime`. `get_cached_context(xtime)` is called in
  `downlink.rs` to restore it, falling back to the legacy 4-byte `count_us` encoding on a
  miss.
- Sweep task runs every 30 s; entries expire after 60 s (`CONTEXT_CACHE_TTL`).
