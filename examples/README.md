# Examples

Test utilities for exercising rak-basicstation without real gateway hardware.

## Fake Concentratord

`fake_concentratord.rs` mimics the Concentratord ZMQ API: responds to commands and publishes synthetic uplink frames.

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

## Load Test

`load_test.rs` exercises rak-basicstation end-to-end by simulating both sides simultaneously: a fake backend (Concentratord ZMQ or Semtech UDP) injecting uplinks, and a fake LNS (WebSocket server) receiving them and optionally sending downlinks back. It measures throughput, latency (p50/p95/p99), and message loss.

```text
[Load Tester]                [rak-basicstation]              [Load Tester]
 Fake Backend  ──uplinks──►  Backend ──► LNS client  ──►   Fake LNS Server
 (ZMQ or UDP)  ◄──downlinks──         ◄── WebSocket ◄──   (WebSocket server)
```

### Quick start (Concentratord backend)

```sh
# Terminal 1: generate config and start load tester
cargo run --example load_test -- --generate-config > /tmp/lt.toml
cargo run --example load_test -- --rate 10 --duration 30

# Terminal 2: start rak-basicstation with the generated config
cargo run -- -c /tmp/lt.toml
```

### Quick start (Semtech UDP backend)

```sh
cargo run --example load_test -- --backend semtech_udp --generate-config > /tmp/lt_udp.toml
cargo run --example load_test -- --backend semtech_udp --rate 50 --duration 30 &
cargo run -- -c /tmp/lt_udp.toml
```

### CLI flags

| Flag | Default | Description |
|------|---------|-------------|
| `--rate` | `10` | Uplinks per second |
| `--duration` | `60` | Test duration in seconds |
| `--backend` | `concentratord` | `concentratord` or `semtech_udp` |
| `--lns-bind` | `0.0.0.0:8887` | Fake LNS WebSocket listen address |
| `--gateway-id` | `0016c001f156d7e5` | 16-char hex gateway EUI |
| `--downlink-ratio` | `0.1` | Fraction of uplinks triggering a downlink |
| `--zmq-event-bind` | `ipc:///tmp/lt_event` | ZMQ PUB socket bind |
| `--zmq-command-bind` | `ipc:///tmp/lt_command` | ZMQ REP socket bind |
| `--udp-target` | `127.0.0.1:1700` | Semtech UDP target address |
| `--generate-config` | _(flag)_ | Print a matching rak-basicstation TOML config and exit |

### Config generation

The `--generate-config` flag prints a complete rak-basicstation TOML config to stdout (no test is run). The config uses the same endpoint addresses, backend type, and gateway ID as the current CLI flags, so you can pipe it directly to a file:

```sh
# Generate with custom settings
cargo run --example load_test -- \
    --backend semtech_udp --udp-target 127.0.0.1:1700 --lns-bind 0.0.0.0:9999 \
    --generate-config > /tmp/lt.toml
```

The generated config sets `logging.level = "warn"` to reduce noise, `lns.reconnect_interval = "1s"` for fast reconnect, and disables CUPS.
