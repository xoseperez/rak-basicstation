# RAK BasicStation Forwarder

A LoRaWAN packet forwarder that implements the [LoRa Basics Station](https://doc.sm.tc/station/) protocol. It supports two backends for receiving uplink frames and sending downlinks:

- **ChirpStack Concentratord** (ZMQ IPC) - communicates with concentratord for SX130x hardware access.
- **Semtech UDP Packet Forwarder** - acts as a UDP server for any standard Semtech UDP packet forwarder (`lora_pkt_fwd` or compatible).

## What It Does

This is a Rust implementation of the [LoRa Basics Station](https://github.com/lorabasics/basicstation) protocol that bridges gateway hardware (via either backend) with LoRaWAN Network Servers using the BasicStation LNS/CUPS protocols.

```text
┌──────────────────────────────────────────────────────────┐
│                     rak-basicstation                     │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌─────┐  ┌───────┐  │
│  │Concentratord │  │  Semtech UDP │  │ LNS │  │ CUPS  │  │
│  │Backend (ZMQ) │  │Backend (UDP) │  │(WSS)│  │(HTTPS)│  │
│  └──────┬───────┘  └──────┬───────┘  └──┬──┘  └──┬────┘  │
│         │                 │             │        │       │
│         └────────┬────────┘    ┌────────┴───┐    │       │
│                  │             │  Protocol  │    │       │
│                  └─────────────┤  Bridge    ├────┘       │
│                                │(proto↔JSON)│            │
│                                └────────────┘            │
└──────────┬──────────┬──────────────┬─────────────┬───────┘
           ▼          ▼              ▼             ▼
    Concentratord  Semtech UDP   LoRaWAN LNS   CUPS Server
     (ZMQ IPC)    Pkt Forwarder  (WebSocket)    (HTTPS)
```

## Features

- **LNS Protocol (v2)**: WebSocket-based communication with LoRaWAN Network Servers
  - Router discovery (`/router-info`)
  - Uplink forwarding (`jreq`, `updf`, `propdf`)
  - Downlink handling (`dnmsg`, `dnsched`) for Class A, B, and C
  - Downlink TX confirmation (`dntxed`)
  - Time synchronization
  - Dynamic channel plan configuration via `router_config`
- **CUPS Protocol**: HTTPS-based credential and configuration management
  - Periodic update checks with configurable intervals
  - Credential persistence and CRC32 tracking
  - URI and credential updates from server
- **Authentication**: TLS server auth, mutual TLS, and token-based auth
- **CRC filtering**: Configurable forwarding of ok/invalid/missing CRC frames
- **Dual backend**: ChirpStack Concentratord (ZMQ) or Semtech UDP Packet Forwarder, selectable via config

## Compatible With

- [The Things Network](https://www.thethingsnetwork.org/) (TTN/TTI)
- [ChirpStack](https://www.chirpstack.io/) (with BasicStation support)
- [AWS IoT Core for LoRaWAN](https://docs.aws.amazon.com/iot/latest/developerguide/connect-iot-lorawan.html)
- Any LNS implementing the BasicStation protocol

## Requirements

- A gateway backend (one of):
  - [ChirpStack Concentratord](https://github.com/chirpstack/chirpstack-concentratord) running and configured for your gateway hardware
  - A Semtech UDP Packet Forwarder (`lora_pkt_fwd` or compatible) sending to the configured UDP bind address
- A LoRaWAN Network Server with BasicStation/LNS protocol support

## Building

### Prerequisites

- Rust 1.89+ (automatically managed via `rust-toolchain.toml`)
- protobuf compiler (`protoc`) and include files
- ZeroMQ development libraries (`libzmq3-dev` / `zeromq-devel`) — only needed for the `concentratord` feature

### Build

```sh
# Both backends (default)
cargo build --release

# Only Semtech UDP backend (no ZMQ dependency)
cargo build --release --no-default-features --features semtech_udp

# Only Concentratord backend
cargo build --release --no-default-features --features concentratord
```

### Cross-compilation

Cross-compilation for embedded targets uses the `cross` tool:

```sh
# Install cross
cargo install cross

# Build for all targets
make build

# Build for a specific target
cross build --target aarch64-unknown-linux-musl --release
```

Supported targets:
- `x86_64-unknown-linux-musl` (AMD64)
- `aarch64-unknown-linux-musl` (ARM64, e.g. Raspberry Pi 4)
- `armv7-unknown-linux-musleabihf` (ARM32 hard-float)
- `armv5te-unknown-linux-musleabi` (ARM v5)

### Tests

```sh
cargo test
cargo clippy --no-deps
```

## Configuration

Configuration is via TOML file. Generate a template:

```sh
rak-basicstation configfile
```

Pass one or more config files at startup:

```sh
rak-basicstation -c /etc/rak-basicstation/rak-basicstation.toml
```

### Example Configuration (Concentratord backend)

```toml
[logging]
  level = "info"
  log_to_syslog = false

[backend]
  enabled = "concentratord"

  [backend.filters]
    forward_crc_ok = true
    forward_crc_invalid = false
    forward_crc_missing = false

  [backend.concentratord]
    event_url = "ipc:///tmp/concentratord_event"
    command_url = "ipc:///tmp/concentratord_command"

[lns]
  server = "wss://lns.example.com:8887"
  reconnect_interval = "5s"
  ca_cert="..."
  tls_cert="..."
  tls_key="..."

[cups]
  enabled = false
```

### Example Configuration (Semtech UDP backend)

```toml
[logging]
  level = "info"
  log_to_syslog = false

[backend]
  enabled = "semtech_udp"
  # gateway_id = ""  # Optional; auto-discovered from first PULL_DATA if empty

  [backend.filters]
    forward_crc_ok = true
    forward_crc_invalid = false
    forward_crc_missing = false

  [backend.semtech_udp]
    bind = "0.0.0.0:1700"
    time_fallback_enabled = false

[lns]
  server = "wss://lns.example.com:8887"
  reconnect_interval = "5s"
  ca_cert="..."
  tls_cert="..."
  tls_key="..."

[cups]
  enabled = false
```

Environment variables can be substituted in the config file using `$VAR_NAME` syntax.

### Authentication Modes

| Mode | `ca_cert` | `tls_cert` | `tls_key` | Description |
|------|-----------|------------|-----------|-------------|
| No Auth | - | - | - | Plain WS/HTTP (development only) |
| Server Auth | CA cert | - | - | TLS, server verified |
| Mutual TLS | CA cert | Client cert | Client key | Both sides verified |
| Token Auth | CA cert | - | Auth token file | TLS + Authorization header |

## Testing with Fake Concentratord

A `fake_concentratord` example binary is included for testing without real gateway hardware. It mimics the Concentratord ZMQ API: responds to commands and publishes synthetic uplink frames.

**1.** Edit `examples/fake_concentratord.toml` with your gateway ID and desired settings:

```toml
gateway_id = "0016c001f156d7e5"
stats_interval = 30

[api]
  event_bind = "ipc:///tmp/test_concentratord_event"
  command_bind = "ipc:///tmp/test_concentratord_command"

[uplink]
  frequency = 868100000
  spreading_factor = 7
  bandwidth = 125000
  interval = 10
```

**2.** Update `rak-basicstation.toml` to match the ZMQ URLs:

```toml
[backend.concentratord]
  event_url = "ipc:///tmp/test_concentratord_event"
  command_url = "ipc:///tmp/test_concentratord_command"
```

**3.** Start the fake concentratord (with optional `-c` for a custom config path):

```sh
cargo run --example fake_concentratord
cargo run --example fake_concentratord -- -c /path/to/config.toml
```

**4.** In another terminal, start the service:

```sh
cargo run -- -c /path/to/rak-basicstation.toml
```

The fake concentratord:
- Publishes test unconfirmed data uplinks at the configured interval
- Publishes gateway stats periodically
- Handles `GetGatewayId`, `SetGatewayConfiguration`, and `SendDownlinkFrame` commands

## Project Structure

```
src/
├── main.rs                 # Entry point, CLI, signal handling
├── lib.rs                  # Module declarations
├── config.rs               # TOML configuration structures
├── logging.rs              # stdout/syslog setup
├── metadata.rs             # Gateway metadata collection
├── backend/
│   ├── mod.rs              # Backend trait, dispatch, and setup
│   ├── concentratord.rs    # ZMQ event/command sockets
│   └── semtech_udp/
│       ├── mod.rs          # UDP server, protocol handling
│       └── structs.rs      # Semtech UDP v2 protocol structs
├── lns/
│   ├── mod.rs              # LNS connection loop
│   ├── discovery.rs        # Router discovery (/router-info)
│   ├── websocket.rs        # WebSocket client
│   ├── messages.rs         # JSON message types (serde)
│   ├── router_config.rs    # Channel plan translation
│   ├── uplink.rs           # Uplink frame conversion
│   ├── downlink.rs         # Downlink frame conversion
│   └── timesync.rs         # Time synchronization
├── cups/
│   ├── mod.rs              # CUPS update loop
│   ├── client.rs           # HTTPS client and response parser
│   └── credentials.rs      # Credential management
└── cmd/
    ├── mod.rs
    └── configfile.rs        # Config template generator
```

## License

MIT

## Related Projects

- [ChirpStack Concentratord](https://github.com/chirpstack/chirpstack-concentratord) - Hardware abstraction daemon for SX130x chips
- [ChirpStack MQTT Forwarder](https://github.com/chirpstack/chirpstack-mqtt-forwarder) - MQTT-based forwarder (architecture reference)
- [LoRa Basics Station](https://github.com/lorabasics/basicstation) - Original BasicStation implementation (protocol reference)
