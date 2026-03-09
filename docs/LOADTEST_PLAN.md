# Plan: Load Test Utility for RAK BasicStation

## Context

We need a standalone load test tool that exercises `rak-basicstation` end-to-end by
simulating both sides simultaneously: a fake backend (Concentratord ZMQ or Semtech UDP)
injecting uplinks, and a fake LNS (WebSocket server) receiving them and optionally
sending downlinks back. The tool measures throughput, latency, and message loss.

```
[Load Tester]                [rak-basicstation]              [Load Tester]
 Fake Backend  ──uplinks──►  Backend ──► LNS client  ──►   Fake LNS Server
 (ZMQ or UDP)  ◄──downlinks──         ◄── WebSocket ◄──   (WebSocket server)
```

---

## File: `examples/load_test.rs`

Single-file example binary, following the existing `examples/fake_concentratord.rs`
pattern. No new crate dependencies — uses `chirpstack_api`, `zmq`, `tokio`,
`tokio-tungstenite`, `clap`, `serde_json`, `base64`, `hex` (all already in the project).

---

## CLI Interface

```
cargo run --example load_test -- \
    --rate 100 \
    --duration 60s \
    --backend concentratord \
    --lns-bind 0.0.0.0:8887 \
    --gateway-id 0016c001f156d7e5 \
    --downlink-ratio 0.1
```

| Flag | Default | Description |
|------|---------|-------------|
| `--rate` | `10` | Uplinks per second |
| `--duration` | `60s` | Test duration (parsed with `humantime`) |
| `--backend` | `concentratord` | `concentratord` or `semtech_udp` |
| `--lns-bind` | `0.0.0.0:8887` | Fake LNS WebSocket listen address |
| `--gateway-id` | `0016c001f156d7e5` | 16-char hex gateway EUI |
| `--downlink-ratio` | `0.1` | Fraction of uplinks triggering a downlink |
| `--zmq-event-bind` | `ipc:///tmp/lt_event` | ZMQ PUB socket bind |
| `--zmq-command-bind` | `ipc:///tmp/lt_command` | ZMQ REP socket bind |
| `--udp-target` | `127.0.0.1:1700` | Semtech UDP target address |
| `--generate-config` | _(flag)_ | Print a rak-basicstation TOML config matching the current flags and exit |

### `--generate-config` Subcommand

When `--generate-config` is passed, the tool prints a complete rak-basicstation TOML
config to stdout (no test is run). The config uses the same endpoint addresses, backend
type, and gateway ID as the current CLI flags, so the user can pipe it directly:

```sh
# Generate and save config matching load tester defaults
cargo run --example load_test -- --generate-config > /tmp/lt.toml

# Or with custom settings
cargo run --example load_test -- \
    --backend semtech_udp --udp-target 127.0.0.1:1700 --lns-bind 0.0.0.0:9999 \
    --generate-config > /tmp/lt.toml

# Then run both
cargo run --example load_test -- --backend semtech_udp --lns-bind 0.0.0.0:9999 &
cargo run -- -c /tmp/lt.toml
```

The generated config:
- Sets `logging.level = "warn"` (reduce noise during load test)
- Sets `backend.enabled` to the selected backend
- For concentratord: uses `--zmq-event-bind` and `--zmq-command-bind` as connect URLs
- For semtech_udp: uses `--udp-target` as the bind address
- Sets `lns.server = "ws://<lns-bind>"` (plain WebSocket, no TLS)
- Sets `lns.reconnect_interval = "1s"` (fast reconnect for testing)
- Disables CUPS

---

## Shared State (`Arc<SharedState>`)

```rust
struct SharedState {
    metrics: Metrics,
    tracker: UplinkTracker,
    shutdown: AtomicBool,
    args: Args,
}

struct Metrics {
    uplinks_sent: AtomicU64,
    uplinks_received: AtomicU64,
    downlinks_sent: AtomicU64,
    downlinks_received: AtomicU64,
    latencies: Mutex<Vec<Duration>>,
    test_start: Instant,
}

struct UplinkTracker {
    timestamps: Mutex<HashMap<u32, Instant>>,
}
```

---

## Concurrency Architecture

```
main()
  ├── std::thread::spawn  →  zmq_rep_handler()      [ZMQ only, blocking]
  ├── std::thread::spawn  →  zmq_pub_publisher()     [ZMQ only, blocking]
  │    OR
  ├── tokio::spawn        →  udp_sender_task()       [UDP only, async]
  ├── tokio::spawn        →  udp_receiver_task()     [UDP only, async]
  │
  ├── tokio::spawn        →  lns_server_task()       [accepts WebSocket connections]
  │     └── tokio::spawn  →  lns_connection_handler() [per connection]
  │
  ├── tokio::spawn        →  progress_reporter()     [prints every 5s]
  │
  └── tokio::time::sleep(duration)  →  set shutdown  →  print report
```

---

## Message Flow

### 1. Startup Sequence

1. Load tester starts fake LNS (TCP listener) and fake backend (ZMQ or UDP)
2. User starts `rak-basicstation` configured to use the load tester's endpoints
3. Backend handshake:
   - **Concentratord**: `GetGatewayId` on REP → respond with gateway_id
   - **Semtech UDP**: Fake backend sends PULL_DATA every 5s to register
4. LNS handshake:
   - rak-basicstation opens WebSocket to `/router-info` → fake LNS responds with MUXS URI
   - rak-basicstation opens main WebSocket → sends `version` → fake LNS sends `router_config`
   - **Concentratord**: rak-basicstation sends `SetGatewayConfiguration` on REP → fake backend acks
5. Load tester detects readiness (first `version` message received) → starts uplink generation

### 2. Uplink Flow

```
Fake Backend                    rak-basicstation                 Fake LNS
    │                                │                               │
    │── gw::Event{UplinkFrame} ────►│                               │
    │   (seq_num in FRMPayload)     │── {"msgtype":"updf",...} ───►│
    │                                │                               │ record latency
    │                                │                               │ metrics++
```

### 3. Downlink Flow (when triggered)

```
Fake LNS                        rak-basicstation                 Fake Backend
    │                                │                               │
    │── {"msgtype":"dnmsg",...} ───►│                               │
    │   (echo xtime, rctx)         │── SendDownlinkFrame ────────►│
    │                                │                               │ metrics++
    │                                │◄── DownlinkTxAck{Ok} ────────│
    │◄── {"msgtype":"dntxed"} ──────│                               │
```

---

## Uplink PHY Payload Generation

Build unconfirmed data uplink frames (`MHdr = 0x40`):

```rust
fn build_uplink_payload(seq: u32, dev_addr: u32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(17);
    payload.push(0x40);                              // MHdr: unconfirmed data up
    payload.extend_from_slice(&dev_addr.to_le_bytes()); // DevAddr (4 bytes LE)
    payload.push(0x00);                              // FCtrl
    payload.extend_from_slice(&(seq as u16).to_le_bytes()); // FCnt (2 bytes LE)
    payload.push(0x01);                              // FPort = 1
    payload.extend_from_slice(&seq.to_be_bytes());   // FRMPayload = seq (4 bytes BE)
    payload.extend_from_slice(&[0x00; 4]);           // MIC (dummy)
    payload
}
```

The full 4-byte `seq` in `FRMPayload` is extractable by the fake LNS from the `updf`
message's `FRMPayload` hex field for latency correlation.

Context value: `(seq * 1000).to_be_bytes()` → gives unique `count_us` for xtime.

Frequencies: cycle through EU868 channels `[868_100_000, 868_300_000, 868_500_000,
867_100_000, 867_300_000, 867_500_000, 867_700_000, 867_900_000]`.

---

## Fake LNS Server

### Connection Type Detection

Since `tokio-tungstenite::accept_async` doesn't expose the HTTP path, detect
connection type from the first WebSocket message:

- Contains `"router"` key (no `msgtype`) → discovery → respond with URI, close
- Contains `"msgtype": "version"` → main connection → send `router_config`, enter loop

### Router Config (EU868)

Static JSON constant with:
- `DRs`: DR0=SF12/125 through DR5=SF7/125, DR6=SF7/250, DR7=FSK
- `sx1301_conf`: 8 multi-SF channels + 1 LoRa std + 1 FSK
- `nocca: true, nodc: true, nodwell: true` (disable regulatory limits for load testing)
- `MuxTime`: current Unix timestamp as f64

### Downlink Generation

When `uplinks_received % (1.0 / downlink_ratio).round() == 0`, send:

```json
{
    "msgtype": "dnmsg",
    "DevEui": "00-00-00-00-00-00-00-01",
    "dC": 0,
    "diid": <counter>,
    "pdu": "60<devaddr_le>0000010001020304aabbccdd",
    "RxDelay": 1,
    "RX1DR": 5,
    "RX1Freq": <uplink_freq>,
    "RX2DR": 0,
    "RX2Freq": 869525000,
    "xtime": <from_uplink>,
    "rctx": <from_uplink>,
    "priority": 0
}
```

---

## Fake Concentratord Backend

Adapted from `examples/fake_concentratord.rs`.

### REP Thread

```rust
loop {
    let msg = cmd_sock.recv_bytes(0)?;
    let cmd = gw::Command::decode(msg.as_slice())?;
    match cmd.command {
        GetGatewayId(_) => respond with GetGatewayIdResponse { gateway_id },
        SetGatewayConfiguration(_) => respond with empty bytes (ack),
        SendDownlinkFrame(_) => {
            metrics.downlinks_received++;
            respond with DownlinkTxAck { status: Ok }
        }
    }
}
```

### PUB Thread

```rust
let interval = Duration::from_secs(1) / rate;
loop {
    if shutdown.load() { break; }
    let seq = counter.fetch_add(1);
    let payload = build_uplink_payload(seq, DEV_ADDR);
    let frame = build_uplink_frame(seq, payload, freq);
    let event = gw::Event { UplinkFrame(frame) };
    pub_sock.send(event.encode_to_vec(), 0)?;
    tracker.record(seq, Instant::now());
    metrics.uplinks_sent++;
    thread::sleep(interval);
}
```

---

## Fake Semtech UDP Backend

### Sender Task

```rust
let mut interval = tokio::time::interval(Duration::from_secs(1) / rate);
let mut pull_interval = tokio::time::interval(Duration::from_secs(5));

loop {
    select! {
        _ = interval.tick() => { send PUSH_DATA with rxpk JSON }
        _ = pull_interval.tick() => { send PULL_DATA to register gateway }
    }
}
```

### Receiver Task

```rust
loop {
    let (size, remote) = socket.recv_from(&mut buf).await?;
    match buf[3] {
        0x01 => { /* PUSH_ACK - ignore */ }
        0x04 => { /* PULL_ACK - ignore */ }
        0x03 => { /* PULL_RESP - downlink received */
            metrics.downlinks_received++;
            send TX_ACK with error="NONE"
        }
    }
}
```

---

## Progress and Final Report

### Progress (every 5s)

```
[  5s] uplinks: 50/50 (100.0%) | downlinks: 5/5 | throughput: 10.0 msg/s
[ 10s] uplinks: 100/100 (100.0%) | downlinks: 10/10 | throughput: 10.0 msg/s
```

### Final Report

```
=== Load Test Report ===
Duration:            60.0s
Backend:             concentratord
Rate (configured):   100 msg/s

--- Uplinks ---
  Sent:              6000
  Received:          5998
  Lost:              2 (0.03%)
  Throughput:        99.97 msg/s

--- Downlinks ---
  Sent (by LNS):    600
  Received:          598

--- Latency (end-to-end uplink) ---
  Samples:           5998
  p50:               1.23 ms
  p95:               3.45 ms
  p99:               7.89 ms
  Max:               12.34 ms
========================
```

---

## Implementation Order

1. Shared state structs (Metrics, UplinkTracker, SharedState)
2. CLI args parsing + `--generate-config` handler
3. Uplink PHY payload builder
4. Fake LNS server (discovery + main connection + router_config + uplink counting + downlink sending)
5. Fake Concentratord backend (REP + PUB threads)
6. Fake Semtech UDP backend (sender + receiver tasks)
7. Progress reporter and final report
8. Main orchestrator (spawn all tasks, wait for duration, shutdown)

## Verification

```sh
# Generate a matching config
cargo run --example load_test -- --rate 10 --duration 30s --generate-config > /tmp/lt.toml

# Terminal 1: start load tester
cargo run --example load_test -- --rate 10 --duration 30s

# Terminal 2: start rak-basicstation with generated config
cargo run -- -c /tmp/lt.toml

# Observe progress output in terminal 1, final report after 30s
```

Also test with `--backend semtech_udp`:
```sh
cargo run --example load_test -- --backend semtech_udp --generate-config > /tmp/lt_udp.toml
cargo run --example load_test -- --backend semtech_udp --rate 50 --duration 30s &
cargo run -- -c /tmp/lt_udp.toml
```
