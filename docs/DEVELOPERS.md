# Developer Guide

This document is aimed at developers who want to understand, maintain, or extend `rak-basicstation`. It covers the solution architecture, key design patterns, and a file-by-file walkthrough of the codebase.

---

## Table of Contents

1. [Overview](#overview)
2. [Technology Stack](#technology-stack)
3. [Architecture](#architecture)
   - [Layered Design](#layered-design)
   - [Concurrency Model](#concurrency-model)
   - [Data Flow](#data-flow)
4. [Feature Flags](#feature-flags)
5. [File-by-File Walkthrough](#file-by-file-walkthrough)
   - [src/main.rs](#srcmainrs)
   - [src/lib.rs](#srclibrs)
   - [src/config.rs](#srcconfigrs)
   - [src/logging.rs](#srcloggingrs)
   - [src/metadata.rs](#srcmetadatars)
   - [src/backend/mod.rs](#srcbackendmodrs)
   - [src/backend/concentratord.rs](#srcbackendconcentratordrs)
   - [src/backend/semtech\_udp/mod.rs](#srcbackendsemtech_udpmodrs)
   - [src/backend/semtech\_udp/structs.rs](#srcbackendsemtech_udpstructsrs)
   - [src/lns/mod.rs](#srclnsmodrs)
   - [src/lns/websocket.rs](#srclnswebsocketrs)
   - [src/lns/discovery.rs](#srclnsdiscoveryrs)
   - [src/lns/messages.rs](#srclnsmessagesrs)
   - [src/lns/uplink.rs](#srclnsuplinkrs)
   - [src/lns/downlink.rs](#srclnsdownlinkrs)
   - [src/lns/router\_config.rs](#srclnsrouter_configrs)
   - [src/lns/timesync.rs](#srclnstimesyncrs)
   - [src/cups/mod.rs](#srccupsmodrs)
   - [src/cups/client.rs](#srccupsclientrs)
   - [src/cups/credentials.rs](#srccupscredentialsrs)
   - [src/cmd/configfile.rs](#srccmdconfigfilers)
   - [examples/fake\_concentratord.rs](#examplesfake_concentratordrs)
6. [Key Patterns](#key-patterns)
7. [Testing](#testing)
8. [Adding a New Backend](#adding-a-new-backend)

---

## Overview

`rak-basicstation` is a LoRaWAN packet forwarder written in Rust. It bridges gateway hardware to a LoRaWAN Network Server (LNS) using the [LoRa Basics Station](https://doc.sm.tc/station/) protocol over WebSocket, while optionally receiving remote configuration updates from a CUPS (Configuration and Update Server) over HTTPS.

The service supports two hardware backends:

- **ChirpStack Concentratord**: communicates with `concentratord` via ZMQ IPC sockets using the ChirpStack protobuf API.
- **Semtech UDP Packet Forwarder**: implements the standard Semtech UDP protocol, accepting connections from `lora_pkt_fwd` or compatible software.

Both backends are compiled in by default but either can be excluded at compile time via feature flags.

---

## Technology Stack

| Concern | Crate |
|---|---|
| Async runtime | `tokio` (multi-thread) |
| CLI parsing | `clap` (derive) |
| Serialization | `serde`, `serde_json`, `toml` |
| Protobuf | `prost` via `chirpstack_api` |
| WebSocket | `tokio-tungstenite` |
| TLS | `rustls` + `rustls-native-certs` + `rustls-pki-types` |
| HTTP client | `reqwest` |
| ZMQ | `zmq` (optional) |
| Error handling | `anyhow` |
| Logging | `log` + `simple_logger` / `syslog` |
| Config templates | `handlebars` |
| CRC checksums | `crc32fast` |
| Time | `chrono` |
| Async traits | `async-trait` |

---

## Architecture

### Layered Design

```
┌──────────────────────────────────────────────────────────────┐
│                          main.rs                             │
│          CLI parsing · initialization · signal handling      │
└──────────┬──────────────────────────────────────┬───────────┘
           │                                      │
    ┌──────▼──────┐                      ┌────────▼────────┐
    │   backend/  │                      │     cups/       │
    │  (Hardware) │                      │  (Remote cfg)   │
    └──────┬──────┘                      └────────┬────────┘
           │  UplinkFrame / DownlinkFrame          │ TC URI / creds
    ┌──────▼──────────────────────────────────────▼────────┐
    │                        lns/                           │
    │        BasicStation protocol · WebSocket · TLS        │
    └──────────────────────────────────────────────────────┘
```

Vertical dependencies:
- `main.rs` owns startup and wires the layers together.
- `backend` handles hardware I/O and speaks the internal `gw::*` protobuf types from `chirpstack_api`.
- `lns` translates those protobuf types to/from BasicStation JSON and manages the WebSocket connection.
- `cups` handles remote credential and URI updates, pushing results into `lns` globals.
- `metadata` and `config` are shared utilities used across layers.

### Concurrency Model

The program runs a single Tokio multi-thread runtime. After startup, it spawns several independent long-running tasks:

| Task | Location | Description |
|---|---|---|
| LNS connection loop | `lns::setup` | Reconnects WebSocket, drives reconnection logic |
| WebSocket writer | `lns::websocket::run` | Drains outgoing message queue to socket |
| WebSocket reader | `lns::websocket::run` | Demultiplexes incoming BasicStation messages |
| Backend event loop | `backend::concentratord` | Receives ZMQ uplink events (blocking task) |
| Backend receive loop | `backend::semtech_udp` | Receives UDP packets |
| CUPS update loop | `cups::setup` | Periodically polls CUPS server |

Shared state between tasks is coordinated with:

- **`std::sync::LazyLock`** — zero-cost lazy initialization of global state.
- **`tokio::sync::RwLock`** — shared data read frequently, written rarely (router config, gateway ID, session counter).
- **`std::sync::Mutex`** — wraps the ZMQ command socket, which must be accessed from a single thread at a time.
- **`tokio::sync::mpsc::unbounded_channel`** — decouples uplink/downlink producers from the WebSocket writer.

Blocking operations (ZMQ receive, which is not async-native) are offloaded via `tokio::task::spawn_blocking` so they don't starve the async runtime.

### Data Flow

**Uplink (gateway → LNS):**

```
Hardware
  │ ZMQ event / UDP packet
  ▼
backend (concentratord.rs or semtech_udp/mod.rs)
  │ gw::UplinkFrame (protobuf)
  │ CRC filter applied
  ▼
lns::send_uplink()
  │ queued into WS_SENDER channel
  ▼
lns/uplink.rs  ← frame_to_json()
  │ parses LoRaWAN MHDR, builds jreq/updf/propdf JSON
  ▼
WebSocket writer task
  │ tokio-tungstenite Text message
  ▼
Network Server
```

**Downlink (LNS → gateway):**

```
Network Server
  │ dnmsg / dnsched WebSocket message
  ▼
lns/websocket.rs  ← message demultiplexer
  ▼
lns/downlink.rs  ← handle_dnmsg() / handle_dnsched()
  │ builds gw::DownlinkFrame (protobuf)
  ▼
backend::send_downlink_frame()
  │ ZMQ REQ/REP / UDP PULL_RESP
  ▼
Hardware
  │ TX ACK
  ▼
lns/websocket.rs  ← sends dntxed confirmation
```

---

## Feature Flags

Defined in `Cargo.toml`:

```toml
[features]
default = ["concentratord", "semtech_udp"]
concentratord = ["dep:zmq"]
semtech_udp = []
```

- `concentratord` pulls in the `zmq` crate and compiles `src/backend/concentratord.rs`. Remove it if you want a ZMQ-free binary (e.g. for embedded targets that do not have `libzmq`).
- `semtech_udp` has no extra dependencies and is essentially always enabled unless you explicitly exclude it.
- Both can be compiled in simultaneously; the active backend is chosen at runtime via `conf.backend.enabled`.

Conditional compilation is handled with `#[cfg(feature = "concentratord")]` guards in `src/backend/mod.rs`.

---

## File-by-File Walkthrough

### src/main.rs

The entry point. Responsibilities:

1. **CLI parsing** via `clap` derive macros. `--config` accepts a `Vec<String>` so multiple TOML files can be merged. The only subcommand is `configfile`, which prints a configuration template and exits.
2. **Initialization sequence**: `metadata::setup` → `backend::setup` → `lns::setup` → `cups::setup`. The order matters: the backend must be ready before LNS, and LNS must be initialized before CUPS can push a new TC URI into it.
3. **Syslog retry loop**: `logging::setup` can fail transiently when the system logger hasn't started yet (common on embedded systems). The code retries every second rather than crashing.
4. **Signal handling**: After all tasks are spawned, the main thread blocks on `SIGINT`/`SIGTERM`. There is no cleanup; all tasks simply stop when the process exits.

```rust
// The signal wait is deliberately simple — no graceful drain.
let mut signals = Signals::new([SIGINT, SIGTERM]).unwrap();
signals.forever().next();
```

### src/lib.rs

Declares the public module tree and imports the `anyhow` macro crate-wide:

```rust
#[macro_use]
extern crate anyhow;
```

This makes `anyhow!()` and `bail!()` available in every module without individual imports.

### src/config.rs

Defines all configuration structures as plain Rust structs with `serde::Deserialize`. Notable points:

- **Multi-file merge**: `Configuration::get(filenames)` reads files in order; each is deserialized independently and merged with a recursive TOML merge so later files override earlier ones.
- **Environment variable substitution**: Before parsing, the raw TOML text is scanned for `${VAR_NAME}` patterns and replaced with `std::env::var`. This lets sensitive values (tokens, paths) stay out of config files.
- **`humantime-serde`** is used for duration fields like `reconnect_interval` and `oksync_interval`, accepting human-readable strings like `"5s"` or `"1h"`.
- Default values are provided via `#[serde(default = "...")]` attributes on fields, pointing to small `fn` that return the default.

```rust
fn default_lns_server() -> String { "wss://localhost:8887".to_string() }

pub struct Lns {
    #[serde(default = "default_lns_server")]
    pub server: String,
    ...
}
```

### src/logging.rs

Thin wrapper that configures either `simple_logger` (stdout/stderr, with timestamps) or the system syslog daemon, depending on `conf.logging.log_to_syslog`. Both write to the standard `log` facade, so all `log::info!`, `log::debug!`, etc. calls in the codebase are backend-agnostic.

### src/metadata.rs

Collects key/value metadata that is attached to version messages sent to the LNS. There are two sources:

- **Static**: key/value pairs in `conf.metadata.static` — written once at startup.
- **Commands**: external programs defined in `conf.metadata.commands` — executed on demand via `tokio::process::Command` every time metadata is requested.

Command output is parsed line-by-line using the configured `split_delimiter` (default `=`). A command producing a single line uses the command name as the key; multi-line output generates keys prefixed with the command name.

The `METADATA`, `COMMANDS`, and `SPLIT_DELIMITER` globals are `LazyLock<RwLock<...>>` so they can be safely read from multiple async tasks.

### src/backend/mod.rs

Defines the `Backend` trait and acts as the runtime dispatcher:

```rust
#[async_trait]
pub trait Backend {
    async fn get_gateway_id(&self) -> Result<String>;
    async fn send_downlink_frame(&self, pl: gw::DownlinkFrame) -> Result<gw::DownlinkTxAck>;
    async fn send_configuration_command(&self, pl: gw::GatewayConfiguration) -> Result<()>;
}
```

The active backend is stored as a `OnceCell<Box<dyn Backend + Sync + Send>>`, initialized once by `setup()`. The `OnceCell` guarantees the value is set exactly once.

`setup()` contains a polling loop for `get_gateway_id()` because in the Semtech UDP backend the gateway ID is not known until the first `PULL_DATA` packet arrives from the packet forwarder:

```rust
loop {
    match backend.get_gateway_id().await {
        Ok(id) if !id.is_empty() => break,
        _ => sleep(Duration::from_secs(1)).await,
    }
}
```

The three public free functions (`get_gateway_id`, `send_downlink_frame`, `send_configuration_command`) delegate to the `OnceCell` value, providing a clean API for the rest of the codebase without exposing the `dyn Backend` type.

### src/backend/concentratord.rs

Implements the `Backend` trait over ChirpStack Concentratord's ZMQ IPC API.

**Initialization**: connects a `SUB` socket to the event URL (uplinks, stats) and a `REQ` socket to the command URL (gateway ID, downlinks, configuration). The gateway ID is fetched synchronously during `new()`.

**Event loop**: runs in a `spawn_blocking` task because `zmq::Socket::recv_bytes` is a blocking call. It receives raw bytes, decodes them as a `gw::Event` protobuf, applies the CRC filter from configuration, and calls `lns::send_uplink()` for accepted frames.

**Command socket**: wrapped in a `Mutex<zmq::Socket>` so it can be called from multiple async contexts safely. Each call locks, sends a command protobuf, polls for a reply with a 100 ms timeout, and decodes the response. On timeout or decode failure the socket is recreated to recover from a stuck state.

**Pattern — async/blocking bridge**:
```rust
// ZMQ receive is blocking; wrap in spawn_blocking to avoid blocking the runtime
let bytes = tokio::task::spawn_blocking(move || sock.recv_bytes(0)).await??;
```

### src/backend/semtech\_udp/mod.rs

Implements the `Backend` trait over the Semtech UDP v2 packet forwarder protocol.

**State** is wrapped in an `Arc<State>` cloned into every handler. Key fields:

- `gateway_id`: populated on first `PULL_DATA` (packet type `0x02`), which carries the 8-byte hardware EUI.
- `pull_addr`: the `SocketAddr` of the last `PULL_DATA` sender — downlinks are sent back to this address.
- `downlink_cache`: `RwLock<HashMap<u16, DownlinkCache>>` keyed by a random token. Entries expire after 60 seconds.

**Downlink caching**: When the LNS sends a `dnmsg`, the backend immediately returns a synthetic ACK to avoid blocking the LNS while waiting for the hardware TX result. The actual `PULL_RESP` is sent to the packet forwarder, and when a `TX_ACK` arrives (type `0x05`) the cache is looked up to find the original `DownlinkFrame` and complete the confirmation loop. If the hardware reports an error, the next item in the `DownlinkFrame::items` list is tried.

**CRC filtering**: applied per-packet based on the `stat` field of each `RxPk`. Frames with unwanted CRC status are silently dropped before conversion.

### src/backend/semtech\_udp/structs.rs

Defines the Semtech UDP v2 wire format as Rust structs. This is the most serialization-heavy file in the project.

Key serialization patterns:

- **Custom `Deserialize`** for `DataRate`: the wire format is a single string like `"SF7BW125"` (LoRa) or `"50000"` (FSK). The deserializer parses the string inline.
- **Custom time deserializers** (`deserialize_compact_time`, `deserialize_expanded_time`): the gateway sends time in two different ISO 8601 variants depending on firmware version.
- **`#[serde(rename_all = "lowercase")]`** and field renames bring the snake_case Rust names in line with the lowercase/abbreviated JSON field names in the Semtech spec.
- **`PushDataPayload`** wraps `rxpk` and `stat` because the spec puts them in a nested JSON object inside the UDP packet body.

The `to_proto_uplink_frames()` method on `RxPk` handles the conversion to `gw::UplinkFrame`. When multiple antennas are present (`rsig` array), it emits one `UplinkFrame` per antenna to give the LNS per-antenna RSSI/SNR.

The file includes 15 unit tests covering LoRa, FSK, GPS timing, fine timestamps, and downlink packet formats. These tests serve as the authoritative documentation for protocol edge cases.

### src/lns/mod.rs

The connection manager for the BasicStation LNS protocol. It holds the most global state in the application:

| Global | Type | Purpose |
|---|---|---|
| `WS_SENDER` | `UnboundedSender<String>` | Queue outgoing WebSocket messages |
| `SESSION_COUNTER` | `RwLock<u8>` | Incremented on each reconnect; embedded in `xtime` |
| `ROUTER_CONFIG` | `RwLock<Option<RouterConfigState>>` | DR table from the LNS |
| `LAST_MUX_TIME` | `RwLock<Option<f64>>` | Last `MuxTime` echo value |
| `CUPS_TC_URI` | `RwLock<Option<String>>` | TC URI pushed by CUPS |
| `CUPS_TC_AUTH_HEADERS` | `RwLock<Vec<(String, String)>>` | Auth headers pushed by CUPS |

**Connection loop logic**:
1. Increment `SESSION_COUNTER`.
2. Determine the WebSocket endpoint: prefer `CUPS_TC_URI` if CUPS has provided one; fall back to `discovery_endpoint` or `conf.lns.server`.
3. Detect token auth mode: if `tls_cert` is empty but `tls_key` is non-empty, read `tls_key` as a raw Bearer token and build an `Authorization` header.
4. If a discovery endpoint is configured, run router discovery to resolve the actual MUXS WebSocket URI.
5. Connect and run the WebSocket session. On disconnect, wait `reconnect_interval` and loop.

**Auth token vs mutual TLS**: The decision between auth modes is made here based on which config fields are populated. This is documented in the README authentication table.

### src/lns/websocket.rs

Manages a single WebSocket session lifetime.

**On connect**: sends a `version` message with station identity, firmware version, hardware model, and supported features. This is required by the BasicStation protocol before any other exchange.

**Message demultiplexer**: reads `msgtype` from every incoming JSON frame using the `GenericMessage` wrapper, then dispatches to the appropriate handler:

| `msgtype` | Handler |
|---|---|
| `router_config` | Store DR table, send `GatewayConfiguration` to backend |
| `dnmsg` | `downlink::handle_dnmsg()` |
| `dnsched` | `downlink::handle_dnsched()` |
| `timesync` | `timesync::update_gps_offset()`, reply with `TimesyncRequest` |
| `error` | Log at WARN level |

**Write task**: a separate Tokio task drains the `WS_SENDER` channel and writes messages to the socket. Decoupling reads from writes prevents a slow write from blocking uplink handling.

**TLS connector** (`build_tls_connector`): loads system root certificates via `rustls-native-certs`, optionally appends a custom CA from config, and optionally loads a client certificate + key for mutual TLS. PEM parsing uses `rustls_pki_types::CertificateDer::pem_slice_iter` and `PrivateKeyDer::from_pem_slice` directly (the `PemObject` trait from `rustls-pki-types`).

### src/lns/discovery.rs

Implements the BasicStation router discovery step. The gateway connects to `{endpoint}/router-info`, sends `{"router": "<id6>"}`, and receives a `RouterInfoResponse` containing the actual MUXS WebSocket URI.

**ID6 format**: the BasicStation spec uses a colon-separated groups-of-four-hex format for gateway identifiers:
```
EUI-64:  0016c001ff10a235
ID6:     0016:c001:ff10:a235
```

The conversion is implemented in `to_id6()` and tested with a unit test.

### src/lns/messages.rs

Defines all BasicStation JSON message types as Rust structs. These are pure data types with `serde` derive macros; no logic lives here.

Notable patterns:

- **`GenericMessage`**: contains only `msgtype: String`. Used for initial deserialization to route messages without deserializing the full payload first.
- **`#[serde(rename = "...")]`** extensively: BasicStation uses `PascalCase` names (`DevEui`, `MuxTime`, `JoinEui`) which are mapped to idiomatic Rust snake_case fields.
- **`null_as_empty_vec`**: a custom deserializer helper that treats a JSON `null` array field as an empty `Vec` rather than failing. Used for `RouterConfig::sx1301_conf` and similar optional lists.
- `RouterConfig` carries the full channel plan: `DRs` is a `Vec<Vec<i32>>` of `[SF, BW_kHz, downlink_only]` triples. The `RouterConfigState` in `router_config.rs` parses this into a more ergonomic form.

### src/lns/uplink.rs

Converts a `gw::UplinkFrame` (protobuf) to a BasicStation JSON uplink message (`jreq`, `updf`, or `propdf`).

**MHDR parsing**: the LoRaWAN MHDR byte determines message type via bits `[7:5]`:

| Bits 7–5 | LoRaWAN type | BasicStation message |
|---|---|---|
| `000` | Join Request | `jreq` |
| `010` | Unconfirmed Data Up | `updf` |
| `100` | Confirmed Data Up | `updf` |
| `111` | Proprietary | `propdf` |

**EUI byte order**: LoRaWAN EUIs are transmitted over-the-air in little-endian order. The `eui_from_le_bytes()` helper reverses them to the big-endian string format expected by BasicStation (`"XX-XX-XX-XX-XX-XX-XX-XX"`).

**xtime encoding**: the `xtime` field used throughout BasicStation carries both a session identifier and a microsecond timestamp:
```
bits [55:48] = session counter (incremented on reconnect)
bits [47:0]  = count_us (from rx_info.context, big-endian u32 zero-extended)
```

This encoding allows the LNS to detect when a reconnection has occurred (session byte changed) and correlate downlink timing back to the original uplink.

**DR mapping**: `modulation_to_dr()` looks up the `(SF, BW_kHz)` pair in the `ROUTER_CONFIG` DR table. If not found it returns `-1`, which signals an unknown data rate to the LNS.

### src/lns/downlink.rs

Converts BasicStation `dnmsg`/`dnsched` JSON to `gw::DownlinkFrame` protobuf and dispatches it to the backend.

**Class dispatch**: `dC` field in `DownlinkMessage`:

| `dC` | Class | Timing |
|---|---|---|
| `0` | A | Delay relative to uplink (`rx_delay` seconds after `xtime`) |
| `1` | B | GPS epoch (`gpstime` field) |
| `2` | C | If `xtime` present: RX1+RX2 delay; if absent: immediate on RX2 |

**Class A** generates two `DownlinkFrameItem` entries (RX1 and RX2). The `context` field is extracted from bits `[47:0]` of `xtime` and passed to the backend so the hardware can compute the precise transmit timestamp.

**`handle_dnsched()`** handles multicast scheduling. Each entry in the schedule array becomes its own `DownlinkFrame` sent to the backend.

**Return path**: after sending, a `DnTxedMessage` is queued via `lns::send_ws_message()` regardless of success or failure, carrying a `diid` (downlink interaction ID) that the LNS uses to correlate the confirmation.

### src/lns/router\_config.rs

Manages the DR (Data Rate) table received from the LNS and translates it to hardware configuration.

**`RouterConfigState`** holds:
- `drs: Vec<(u32, u32)>` — DR index → `(SF, BW_kHz)` mapping.
- `net_ids`, `join_eui_ranges` — for future filtering use.
- `freq_range`, `region` — informational.

**DR lookup helpers**:
- `sf_bw_to_dr()` — for uplink encoding.
- `dr_to_sf_bw()` — for downlink decoding.
- `fsk_to_dr()` — finds FSK entry in the DR table (identified by `SF == 0`).

**`to_gateway_configuration()`** translates the full `RouterConfig` to a `gw::GatewayConfiguration` protobuf sent to the backend (Concentratord) so the hardware reconfigures its radio channels to match the LNS's channel plan. This is called every time a `router_config` message is received.

### src/lns/timesync.rs

Handles the BasicStation time synchronization exchange. The GPS time offset is stored as `Option<i64>` (microseconds) in a `LazyLock<RwLock<...>>` global.

When a `timesync` message arrives from the LNS:
1. The GPS offset is updated: `gpstime - xtime`.
2. A `TimesyncRequest` reply is queued to echo the `txtime`.

`get_gps_offset()` is called by `uplink.rs` when building `gpstime` for uplink frames, converting the concentrator's internal microsecond counter to GPS epoch time.

### src/cups/mod.rs

The CUPS (Configuration and Update Server) client loop. CUPS allows a central server to push new LNS URIs and TLS credentials to gateways without manual intervention.

**Startup**: if `conf.cups.enabled`, previously persisted credentials are restored from disk (so a restart doesn't require an immediate CUPS round-trip). Then `update_loop` is spawned.

**Update loop**: calls `run_update()` periodically. On success, uses `conf.cups.oksync_interval` (default 24 hours) as the next wait. On failure, uses `conf.cups.resync_interval` (default 60 seconds) for faster retry.

**`run_update()`**:
1. Computes CRC32 of current CUPS credentials, TC credentials, and signature keys.
2. POSTs to `{cups_server}/update-info` with the CRCs.
3. If the server returns new credentials or a new TC URI, persists them to `conf.cups.credentials_dir`.
4. Calls `lns::set_cups_tc_uri()` and `lns::set_cups_tc_auth_headers()` to inject the new values into the LNS connection loop. The next reconnect will use them.

The CRC-based diff prevents unnecessary credential rotation when nothing has changed.

### src/cups/client.rs

Implements the CUPS HTTP protocol.

**Request format** (`post_update_info`): a JSON body sent to `/update-info` containing the gateway's current credential CRCs, station version, and hardware model. The server compares these against its records and returns only what has changed.

**Response format** (`parse_response`): a compact binary format parsed manually byte-by-byte:

```
[ 1-byte cups_uri_len ][ cups_uri ]
[ 1-byte tc_uri_len   ][ tc_uri   ]
[ 2-byte cups_cred_len ][ cups_cred ]
[ 2-byte tc_cred_len   ][ tc_cred   ]
[ 4-byte sig_len       ][ 4-byte key_crc ][ sig_bytes ]
[ 4-byte upd_data_len  ][ upd_data ]
```

All lengths are little-endian. A length of zero means the field is absent (no update for that item). The binary parser uses a cursor-style index into a `Vec<u8>`, advancing manually after each field.

**TLS client**: `build_client()` mirrors `websocket::build_tls_connector()` for the HTTP side, using `reqwest` with `rustls` backend, supporting the same CA cert / mutual TLS / token auth modes as the LNS connection.

### src/cups/credentials.rs

Low-level credential management for CUPS.

**CRC computation**: `compute_cred_crc()` concatenates the raw bytes of certificate files and hashes with `crc32fast`. This matches what the CUPS server expects.

**DER blob parsing** (`parse_token_from_cred`): the CUPS TC credential blob is a concatenation of DER-encoded items:
1. Trust anchor (CA certificate) — a DER `SEQUENCE`.
2. Either a client certificate (another DER `SEQUENCE`) or a 4-byte zero sentinel `0x00000000`, which signals token-based authentication.
3. Either a private key (DER `SEQUENCE`) or a raw HTTP header line `"Name: Value\r\n"`.

`asn1_seq_total_len()` parses the DER length field, supporting both short-form (1 byte) and the long-form 2-byte variant (`0x82` prefix) used by BasicStation.

**Persistence**: `save_uri()` and `save_credentials()` write to files under `credentials_dir`. The directory is created if it does not exist.

### src/cmd/configfile.rs

Generates a commented configuration template using the `handlebars` template engine. The template iterates over the live configuration structs (with defaults populated) and renders them as TOML. This ensures the generated template always reflects the current default values rather than a manually maintained static file.

### examples/fake\_concentratord.rs

A self-contained mock of the ChirpStack Concentratord ZMQ API, used for integration testing without real gateway hardware.

It binds:
- A `PUB` socket (events: uplinks, stats) on `event_bind`.
- A `REP` socket (commands: gateway ID, downlink ACKs) on `command_bind`.

It publishes:
- `GatewayStats` at `stats_interval` seconds.
- A synthetic LoRaWAN unconfirmed data uplink (`MHDR=0x40`, `DevAddr=0x26011234`, incrementing `FCnt`) at `uplink.interval` seconds.

Commands handled:
- `GetGatewayIdRequest` → responds with the configured `gateway_id`.
- `SetGatewayConfiguration` → logs the channel plan, responds with success.
- `SendDownlinkFrame` → logs the downlink, responds with a success `DownlinkTxAck`.

Run it alongside `rak-basicstation` for end-to-end testing:
```sh
cargo run --example fake_concentratord -- -c examples/fake_concentratord.toml
```

---

## Key Patterns

### Global state with LazyLock + RwLock

The project uses this pattern extensively for shared async state:

```rust
static SOME_STATE: LazyLock<RwLock<Option<String>>> =
    LazyLock::new(|| RwLock::new(None));

// Writer
*SOME_STATE.write().await = Some("value".to_string());

// Reader
let val = SOME_STATE.read().await.clone();
```

`LazyLock` provides one-time initialization without `unsafe`. `RwLock` allows concurrent reads (cheap) with exclusive writes (rare). The alternative of passing state through function parameters was avoided to keep function signatures simple across the call tree.

### Error handling with anyhow

All fallible functions return `anyhow::Result<T>`. The `?` operator propagates errors upward. At the top level (task spawns, setup functions), errors are handled with `.expect()` for unrecoverable failures or logged and retried for transient ones.

The `bail!()` macro (from `anyhow`, imported globally in `lib.rs`) is used for early returns with a formatted error:
```rust
bail!("No private key found in TLS key file");
```

### Retry loops for transient failures

Rather than complex error classification, the code uses simple sleep-and-retry loops for known transient conditions (backend not ready, syslog not started, LNS connection dropped):

```rust
loop {
    match try_something().await {
        Ok(result) => break result,
        Err(e) => {
            warn!("Transient error: {}, retrying...", e);
            sleep(Duration::from_secs(1)).await;
        }
    }
}
```

### Feature-gated compilation

The `concentratord` feature gates both the dependency and the code:

```toml
# Cargo.toml
zmq = { version = "0.10", optional = true }
```

```rust
// backend/mod.rs
#[cfg(feature = "concentratord")]
use crate::backend::concentratord::Backend as ConcentratordBackend;

#[cfg(feature = "concentratord")]
if conf.backend.enabled == "concentratord" {
    ...
}
```

This ensures the binary has no ZMQ symbols at all when compiled without the feature.

### MPSC channel as message queue

WebSocket writes are decoupled from the rest of the application via an unbounded MPSC channel. Senders (`lns::send_uplink`, `lns::send_ws_message`) just push strings without blocking. The write task drains the channel:

```rust
while let Some(msg) = receiver.recv().await {
    ws_sender.send(Message::Text(msg.into())).await?;
}
```

This prevents uplink handling from stalling while waiting for the WebSocket send buffer.

---

## Testing

Unit tests live inline in the files they test (`#[cfg(test)]` modules). Run them with:

```sh
cargo test
```

Lint checks:
```sh
cargo clippy --no-deps
```

Both are wrapped in `make test`.

Current test coverage by file:

| File | What is tested |
|---|---|
| `lns/discovery.rs` | ID6 conversion for valid and invalid gateway IDs |
| `lns/uplink.rs` | EUI little-endian to string conversion |
| `backend/semtech_udp/structs.rs` | 15 cases: LoRa/FSK packet parsing, GPS time, fine timestamp, downlink timing modes |
| `cups/client.rs` | Binary response parsing: empty response, response with URIs |

**Integration testing** uses `examples/fake_concentratord.rs` as a hardware mock. Point a running `rak-basicstation` instance at it to test the full uplink/downlink flow without physical hardware.

---

## Adding a New Backend

To add a third backend (e.g. a different IPC mechanism):

1. **Add a feature flag** in `Cargo.toml`:
   ```toml
   my_backend = ["dep:some-crate"]
   ```

2. **Create `src/backend/my_backend.rs`** and implement the `Backend` trait:
   ```rust
   use async_trait::async_trait;
   use anyhow::Result;
   use chirpstack_api::gw;
   use crate::backend::Backend;

   pub struct MyBackend { ... }

   #[async_trait]
   impl Backend for MyBackend {
       async fn get_gateway_id(&self) -> Result<String> { ... }
       async fn send_downlink_frame(&self, pl: gw::DownlinkFrame) -> Result<gw::DownlinkTxAck> { ... }
       async fn send_configuration_command(&self, pl: gw::GatewayConfiguration) -> Result<()> { ... }
   }
   ```

3. **Register it in `src/backend/mod.rs`** under a `#[cfg(feature = "my_backend")]` guard, handling `conf.backend.enabled == "my_backend"` in `setup()`.

4. **Add a `SemtechUdp`-equivalent config struct** in `src/config.rs` with a matching `[backend.my_backend]` TOML section.

5. **Update `src/cmd/configfile.rs`** template to include the new config section.

6. **Update the README** prerequisites and build instructions if the new backend introduces system dependencies.
