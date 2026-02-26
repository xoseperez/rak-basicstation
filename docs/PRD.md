# Product Requirements Document — RAK BasicStation Forwarder

**Version**: 0.3.0
**Date**: 2026-02-26
**Status**: Active Development

## 1. Overview

RAK BasicStation Forwarder is a Rust implementation of the [LoRa Basics Station](https://doc.sm.tc/station/) protocol. It acts as a packet forwarder that bridges LoRaWAN gateway hardware to network servers over WebSocket (LNS) and HTTPS (CUPS), replacing or supplementing the original C-based BasicStation implementation with a modern, safe, and portable codebase.

### 1.1 Problem Statement

Deploying LoRaWAN gateways with BasicStation connectivity today requires either the original C implementation (complex build, limited maintainability) or vendor-specific forks. Operators need a lightweight, cross-platform forwarder that:

- Works with multiple hardware backends without recompilation.
- Connects to any standards-compliant LNS (TTN, ChirpStack, AWS IoT Core, etc.).
- Supports remote credential and configuration management via CUPS.
- Runs on constrained ARM targets (Raspberry Pi, RAK gateways) and x86 servers alike.

### 1.2 Solution

A single statically-linkable Rust binary (~3,000 lines) that implements BasicStation LNS Protocol v2 and the CUPS update protocol. It abstracts gateway hardware behind a pluggable backend trait, with two implementations shipped today: ChirpStack Concentratord (ZMQ IPC) and Semtech UDP Packet Forwarder.

## 2. Goals and Non-Goals

### 2.1 Goals

- **Protocol fidelity**: Implement the BasicStation LNS v2 and CUPS protocols accurately enough to interoperate with TTN, ChirpStack, and AWS IoT Core.
- **Backend flexibility**: Support multiple gateway hardware interfaces through a runtime-selectable backend, with no code changes required to switch.
- **Small footprint**: Produce a stripped, LTO-optimized release binary suitable for embedded Linux gateways.
- **Operational simplicity**: Single binary, single TOML config file, no runtime dependencies beyond the chosen backend.
- **Security**: TLS everywhere (server auth, mutual TLS, token auth), no plaintext in production.
- **Cross-platform builds**: First-class support for x86_64, aarch64, armv7, and armv5te via `cross`.

### 2.2 Non-Goals

- **SX130x hardware driver**: The forwarder does not interact with concentrator chips directly; that responsibility belongs to the backend (Concentratord or `lora_pkt_fwd`).
- **Network server functionality**: No uplink deduplication, device session management, or MAC-layer logic.
- **GUI or web interface**: Configuration is file-based; no built-in management UI.
- **Firmware OTA**: While CUPS can deliver update payloads, applying firmware updates is out of scope — the binary handles credential rotation, not self-update.

## 3. Target Users

| User | Description |
|---|---|
| **Gateway operators** | Deploy and maintain LoRaWAN gateways running Linux, connecting to one or more LNS providers. |
| **Network administrators** | Manage fleets of gateways via CUPS, rotating credentials and updating TC URIs remotely. |
| **Platform integrators** | Embed the forwarder in gateway images (Docker, Yocto, Buildroot) as part of a larger LoRaWAN deployment. |
| **Developers** | Extend the forwarder with new backends or adapt it for custom gateway hardware. |

## 4. Architecture

```text
┌──────────────────────────────────────────────────────────┐
│                     rak-basicstation                     │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌─────┐  ┌───────┐ │
│  │Concentratord │  │  Semtech UDP │  │ LNS │  │ CUPS  │ │
│  │Backend (ZMQ) │  │Backend (UDP) │  │(WSS)│  │(HTTPS)│ │
│  └──────┬───────┘  └──────┬───────┘  └──┬──┘  └──┬────┘ │
│         │                 │             │        │      │
│         └────────┬────────┘    ┌────────┴───┐    │      │
│                  │             │  Protocol  │    │      │
│                  └─────────────┤  Bridge    ├────┘      │
│                                │(proto↔JSON)│           │
│                                └────────────┘           │
└──────────┬──────────┬──────────────┬─────────────┬──────┘
           ▼          ▼              ▼             ▼
    Concentratord  Semtech UDP   LoRaWAN LNS   CUPS Server
     (ZMQ IPC)    Pkt Forwarder  (WebSocket)    (HTTPS)
```

### 4.1 Module Map

| Module | Responsibility |
|---|---|
| `main.rs` | CLI parsing, config loading, signal handling, task orchestration. |
| `config.rs` | TOML config structs with serde, environment variable substitution. |
| `backend/` | `Backend` trait (`get_gateway_id`, `send_downlink_frame`, `send_configuration_command`) with Concentratord and Semtech UDP implementations. |
| `lns/` | WebSocket lifecycle, router discovery, message routing, uplink/downlink conversion, channel plan management, time sync. |
| `cups/` | CUPS update loop, HTTPS client, credential persistence and CRC tracking. |
| `metadata.rs` | Static and command-sourced gateway metadata attached to uplinks. |
| `logging.rs` | stdout and syslog logger initialization. |
| `cmd/configfile.rs` | Config template generator (`rak-basicstation configfile`). |

### 4.2 Concurrency Model

The binary runs on the Tokio multi-thread async runtime. Key concurrent tasks:

| Task | Scheduling | Purpose |
|---|---|---|
| LNS connection loop | `tokio::spawn` | Reconnects WebSocket on failure with configurable interval. |
| WebSocket reader | Inline in connection | Demultiplexes incoming JSON by `msgtype`. |
| WebSocket writer | `tokio::spawn` | Drains outgoing mpsc queue, decoupling producers from the socket. |
| Backend event loop | `spawn_blocking` (ZMQ) or `tokio::spawn` (UDP) | Receives uplinks from hardware. |
| CUPS update loop | `tokio::spawn` | Periodic HTTPS update checks. |
| Context cache sweep | `tokio::spawn` (optional) | Evicts expired entries from the `rx_info.context` cache every 30 s (only when `context_caching` is enabled). |

Global state uses `LazyLock<RwLock<T>>` for read-heavy data (router config, gateway ID, session counter), `mpsc::UnboundedSender` for the WebSocket write queue, and `LazyLock<Mutex<HashMap>>` for the optional context cache.

## 5. Functional Requirements

### 5.1 LNS Protocol (v2)

| ID | Requirement | Status |
|---|---|---|
| LNS-01 | Connect to LNS MUXS endpoint over WebSocket with TLS. | Done |
| LNS-02 | Send `version` message on connect with station, firmware, model, and protocol fields. | Done |
| LNS-03 | Receive and apply `router_config` (channel plan, DR table, frequency ranges). | Done |
| LNS-04 | Forward uplinks as `jreq` (Join Request), `updf` (data), or `propdf` (proprietary) based on MHDR. | Done |
| LNS-05 | Handle `dnmsg` (unicast downlink) for Class A, B, and C with RX1/RX2 windows. | Done |
| LNS-06 | Handle `dnsched` (scheduled/multicast downlink). | Done |
| LNS-07 | Send `dntxed` (TX confirmation) after downlink transmission. | Done |
| LNS-08 | Implement `timesync` request/response for GPS time offset tracking. | Done |
| LNS-09 | Perform router discovery via `GET /router-info` with EUI-64 in ID6 format. | Done |
| LNS-10 | Reconnect automatically on WebSocket disconnect with configurable interval. | Done |
| LNS-11 | Attach gateway metadata (static and command-sourced) to uplink frames. | Done |

### 5.2 CUPS Protocol

| ID | Requirement | Status |
|---|---|---|
| CUPS-01 | POST `/update-info` with router ID, current URIs, credential CRCs, and signature key CRCs. | Done |
| CUPS-02 | Parse binary CUPS response (URI updates, credential blobs, signature, update payload). | Done |
| CUPS-03 | Persist updated TC URI and credentials to disk for restart recovery. | Done |
| CUPS-04 | Inject updated TC URI and auth headers into the LNS connection at runtime. | Done |
| CUPS-05 | Use CRC32-based diffing to avoid unnecessary credential rotation. | Done |
| CUPS-06 | Configurable sync intervals: `oksync_interval` (success, default 24h), `resync_interval` (failure, default 60s). | Done |

### 5.3 Backend: ChirpStack Concentratord

| ID | Requirement | Status |
|---|---|---|
| BE-C01 | Connect to Concentratord via ZMQ IPC (SUB for events, REQ for commands). | Done |
| BE-C02 | Fetch gateway ID via `GetGatewayId` command at startup. | Done |
| BE-C03 | Receive uplink frames, apply CRC filter, forward to LNS. | Done |
| BE-C04 | Send downlink frames via `SendDownlinkFrame` command with 100ms timeout. | Done |
| BE-C05 | Push channel configuration via `SetGatewayConfiguration` command. | Done |
| BE-C06 | Auto-reconnect ZMQ command socket on failure. | Done |
| BE-C07 | Optionally cache the full `rx_info.context` blob on uplink (keyed by `xtime`) and restore it verbatim on the matching downlink; falls back to the 4-byte `count_us` encoding on a miss or when disabled. Enabled via `context_caching = true`. | Done |

### 5.4 Backend: Semtech UDP Packet Forwarder

| ID | Requirement | Status |
|---|---|---|
| BE-S01 | Bind UDP socket and handle Semtech v2 protocol (PUSH_DATA, PULL_DATA, TX_ACK). | Done |
| BE-S02 | Auto-discover gateway ID from first PULL_DATA packet. | Done |
| BE-S03 | Parse uplink frames with per-antenna RSSI/SNR, apply CRC filter, forward to LNS. | Done |
| BE-S04 | Cache downlink frames and match with TX_ACK responses (60s expiry). | Done |
| BE-S05 | Support fine timestamp and GPS time fields when available. | Done |

### 5.5 Authentication

| ID | Requirement | Status |
|---|---|---|
| AUTH-01 | Server-only TLS (custom CA cert). | Done |
| AUTH-02 | Mutual TLS (client cert + key). | Done |
| AUTH-03 | Token-based auth (Authorization header extracted from key file). | Done |
| AUTH-04 | Apply same TLS configuration to both LNS WebSocket and CUPS HTTPS. | Done |

### 5.6 Configuration

| ID | Requirement | Status |
|---|---|---|
| CFG-01 | TOML-based configuration with serde deserialization. | Done |
| CFG-02 | Environment variable substitution (`$VAR_NAME`) in config values. | Done |
| CFG-03 | Multiple config files merged in order (`-c file1.toml -c file2.toml`). | Done |
| CFG-04 | Config template generation via `configfile` subcommand. | Done |
| CFG-05 | CRC filtering per-frame status (ok, invalid, missing). | Done |
| CFG-06 | `context_caching` flag under `[backend.concentratord]` enables full-context preservation (default `false`). | Done |

### 5.7 OpenWrt Integration

| ID | Requirement | Status |
|---|---|---|
| OW-01 | Provide an OpenWrt package (`rak-basicstation`) that installs the binary, a procd init script, and a default UCI config to `/etc/config/rak-basicstation`. | Done |
| OW-02 | Translate the flat UCI config to a TOML file at `/var/etc/rak-basicstation/rak-basicstation.toml` on every service start/reload via a shell library (`rak-basicstation.sh`). | Done |
| OW-03 | Write PEM certificate/key content stored as UCI option strings to discrete files under `/var/etc/rak-basicstation/` (mode 0644 for certs, 0600 for keys); omit the corresponding TOML path when the option is empty. | Done |
| OW-04 | Provide a LuCI web UI package (`luci-app-rak-basicstation`) with tabbed forms for Backend (concentratord slot, context caching), LNS (server URI, certificates), and CUPS settings. | Done |
| OW-05 | Register the service with procd for automatic respawn and config-file-triggered reloads. | Done |

## 6. Non-Functional Requirements

| ID | Requirement | Target | Status |
|---|---|---|---|
| NFR-01 | Release binary size (stripped, LTO, `opt-level=z`). | < 10 MB | Done |
| NFR-02 | Cross-compilation for x86_64, aarch64, armv7, armv5te, mipsel. | All five targets build | Done |
| NFR-03 | Docker image with multi-stage build. | Debian bookworm-slim base | Done |
| NFR-04 | Logging to stdout or syslog, configurable log level. | Done | Done |
| NFR-05 | No unsafe code in the crate (ZMQ is in a dependency). | Zero `unsafe` blocks | Done |
| NFR-06 | Debian packaging metadata (`cargo-deb`). | .deb with systemd unit | Done |
| NFR-07 | OpenWrt packaging for MIPSEL gateways: `rak-basicstation` (.ipk) and `luci-app-rak-basicstation` (.ipk). | Built via OpenWrt build system from `openwrt/` | Done |

## 7. Supported Platforms

### 7.1 Build Targets

| Target | Architecture | Notes |
|---|---|---|
| `x86_64-unknown-linux-musl` | AMD64 | Servers, VMs |
| `aarch64-unknown-linux-musl` | ARM64 | Raspberry Pi 4/5, RAK gateways |
| `armv7-unknown-linux-musleabihf` | ARM32 hard-float | Raspberry Pi 2/3, older gateways |
| `armv5te-unknown-linux-musleabi` | ARM v5 | Legacy embedded |
| `mipsel-unknown-linux-musl` | MIPS little-endian | RAK OpenWrt gateways; packaged as `.ipk` via OpenWrt build system (`openwrt/`); requires nightly toolchain |

### 7.2 Compatible Network Servers

| Server | Protocol | Tested |
|---|---|---|
| The Things Network (TTN/TTI) | LNS v2 + CUPS | Yes |
| ChirpStack | LNS v2 | Yes |
| AWS IoT Core for LoRaWAN | LNS v2 + CUPS | Yes |
| Any BasicStation-compliant LNS | LNS v2 | Expected |

## 8. Dependencies

### 8.1 Runtime

| Dependency | Purpose | Required |
|---|---|---|
| ChirpStack Concentratord | ZMQ-based hardware backend | If using `concentratord` backend |
| Semtech UDP Packet Forwarder | UDP-based hardware backend | If using `semtech_udp` backend |
| `libzmq5` | ZMQ shared library | Only with `concentratord` feature |
| System CA certificates | TLS root trust | Yes |

### 8.2 Build-Time

| Dependency | Purpose |
|---|---|
| Rust 1.89+ | Compiler (edition 2024) |
| `protoc` + protobuf headers | Protobuf code generation (`chirpstack_api`) |
| `libzmq3-dev` | ZMQ headers (only with `concentratord` feature) |

### 8.3 Key Crate Dependencies

| Concern | Crate(s) |
|---|---|
| Async runtime | `tokio` (multi-thread) |
| WebSocket | `tokio-tungstenite` |
| TLS | `rustls`, `rustls-native-certs` |
| HTTP client | `reqwest` (rustls backend) |
| Protobuf / LoRaWAN types | `chirpstack_api`, `lrwn_filters` |
| ZMQ (optional) | `zmq` |
| Serialization | `serde`, `serde_json`, `toml` |
| CLI | `clap` (derive) |

## 9. Testing Strategy

### 9.1 Unit Tests

Unit tests cover protocol-level serialization and conversion logic:

- **Semtech UDP structs** (`backend/semtech_udp/structs.rs`): 15 tests covering LoRa/FSK uplink parsing, GPS timing, fine timestamps, and downlink frame formatting.
- **Router discovery** (`lns/discovery.rs`): EUI-64 to ID6 format conversion.
- **Credential parsing** (`cups/credentials.rs`): ASN.1 DER token extraction.

Run with `cargo test`.

### 9.2 Integration Testing

The `fake_concentratord` example binary (`examples/fake_concentratord.rs`) simulates the Concentratord ZMQ API for end-to-end testing without hardware. It:

- Responds to `GetGatewayId`, `SetGatewayConfiguration`, `SendDownlinkFrame` commands.
- Publishes synthetic uplink frames at a configurable interval.
- Publishes periodic gateway stats.

### 9.3 Linting

`cargo clippy --no-deps` is run as part of `make test`.

## 10. Deployment

### 10.1 Standalone Binary

```sh
cargo build --release
./target/release/rak-basicstation -c /etc/rak-basicstation/rak-basicstation.toml
```

### 10.2 Docker

A multi-stage Dockerfile and docker-compose.yml are provided. The compose file pairs the forwarder with ChirpStack Concentratord via shared `/tmp` volume for ZMQ IPC.

```sh
docker compose up -d
```

### 10.3 Debian Package

`cargo-deb` metadata is defined in `Cargo.toml`. The package installs:

- Binary to `/usr/bin/rak-basicstation`
- Config to `/etc/rak-basicstation/rak-basicstation.toml`
- Systemd service unit (enabled on install)

### 10.4 OpenWrt Package

Two packages in `openwrt/` are built with the OpenWrt build system and produce `.ipk` files installable via `opkg`:

| Package | Installs |
|---|---|
| `rak-basicstation` | Binary (`/usr/bin/rak-basicstation`), init script (`/etc/init.d/rak-basicstation`), config helper (`/lib/functions/rak-basicstation.sh`), default UCI config (`/etc/config/rak-basicstation`) |
| `luci-app-rak-basicstation` | LuCI view and menu entry under **RAK → BasicStation Forwarder** |

On each start or reload, `rak-basicstation.sh` generates `/var/etc/rak-basicstation/rak-basicstation.toml` from the UCI config and writes any certificate/key content to files in the same directory.

```sh
# Inside an OpenWrt build tree with openwrt/ added as a local feed:
./scripts/feeds update -a && ./scripts/feeds install rak-basicstation luci-app-rak-basicstation
make package/rak-basicstation/compile
make package/luci-app-rak-basicstation/compile
```

## 11. Known Limitations

| Area | Limitation |
|---|---|
| Graceful shutdown | The process blocks on SIGINT/SIGTERM and exits; in-flight downlinks are not drained. |
| GPS detection | No mechanism to detect GPS availability at connect time; the `features` field in the version message is sent empty. |
| Self-update | CUPS update payloads are parsed but not applied; the binary does not replace itself. |
| Single gateway | Each instance serves one gateway; fleet management requires one process per gateway. |
| Class B beaconing | Beacon downlinks are forwarded via `dnsched` but no local beacon scheduling is performed. |
| Backend hot-swap | Changing `backend.enabled` requires a restart. |

## 12. Future Considerations

- **Metrics / observability**: Expose Prometheus metrics (uplink/downlink counts, latency, reconnects).
- **Additional backends**: SPI-based direct hardware access, MQTT bridge.
- **GPS feature detection**: Query backend for GPS lock status and populate the `features` field.
- **Integration test suite**: Automated end-to-end tests against a mock LNS WebSocket server.
- **Configuration reload**: Watch config file for changes and apply without restart.
