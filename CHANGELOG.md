# Changelog

All notable changes to this project are documented here.

## [0.3.1] — 2026-07-24

### Added
- **Context caching trace logs** (`debug` level): the uplink path logs each cached
  `rx_info.context` blob (`xtime`, `count_us`, hex context, cache size), and the downlink
  path logs the matching cache hit for Class A and Class C. Both lines carry the same
  `xtime`, so a single exchange can be followed end to end. Cache sweeps log the number
  of expired entries.

---

## [0.3.0] — 2026-02-25

### Added
- **OpenWrt package** (`openwrt/rak-basicstation`): procd init script, UCI→TOML config
  generator shell library, and default UCI config installed to `/etc/config/rak-basicstation`.
- **LuCI web UI** (`openwrt/luci-app-rak-basicstation`): tabbed form covering Backend,
  LNS, and CUPS settings; available under *RAK → BasicStation Forwarder*.
- **MIPSEL build target** (`mipsel-unknown-linux-musl`) for RAK OpenWrt gateways.
  Tier-3 Rust target built with nightly and `-Z build-std`; `zmq-sys` compiles `libzmq`
  from bundled source via cmake — no pre-installed system library required.
- **Context caching** (`backend.concentratord.context_caching`): caches the full
  `rx_info.context` blob on uplink, keyed by the reconstructed `xtime`, and restores it
  verbatim on the matching downlink. Required for ChirpStack Gateway Mesh deployments.
  Entries expire after 60 s; cache is swept every 30 s.
- **Load test utility** (`examples/load_test.rs`): end-to-end load test with a fake
  backend and fake LNS, reporting throughput and p50/p95/p99 latency.
- **Example topology diagrams** (SVG) in `assets/`: four deployment topologies
  (BasicStation gateway, legacy bridge, multiplexer, gateway mesh).
- **GitHub Actions release workflow** (`.github/workflows/release.yml`): cross-compiles
  for amd64, arm64, and armv7; builds `.tar.gz` and `.deb` packages; pushes a multi-arch
  Docker image to Docker Hub on `v*` tag push.
- **Docker environment variable defaults**: all `${VAR_NAME}` placeholders in the config
  template have sensible defaults set by the entrypoint script.

### Changed
- Packaging assets moved to `packaging/` hierarchy:
  - `packaging/docker/` — Docker config template and entrypoint script.
  - `packaging/debian/` — systemd unit and default TOML config for `.deb` builds.
- `Dockerfile.release` copies binaries directly from artifact subdirectories
  (`dist/rak-basicstation-${TARGETARCH}${TARGETVARIANT}/rak-basicstation`), removing
  the need for a separate staging step in CI.

### Fixed
- Peer-review improvements: error message formatting, minor code quality fixes.

---

## [0.2.0] — 2026-02-24

### Added
- **Semtech UDP backend**: implements the Semtech UDP v2 packet forwarder protocol.
  Acts as a UDP server on port 1700, accepting `PUSH_DATA`/`PULL_DATA` from
  `lora_pkt_fwd` or compatible software. Gateway ID is auto-discovered from the first
  `PULL_DATA` packet.
- **Developer guide** (`docs/DEVELOPERS.md`): architecture overview, concurrency model,
  data-flow diagrams, file-by-file walkthrough, key patterns, and a guide to adding new
  backends.

### Fixed
- Device EUI byte order in Join Request frames (was big-endian, must be little-endian
  per LoRaWAN spec).
- Corrected debug log copy for `updf` uplinks.
- Removed credentials from error messages to prevent accidental secret leakage in logs.

### Removed
- `rustls-pemfile` and `dotenv` dependencies (replaced by `rustls-pki-types` PEM
  iterators and inline `${VAR_NAME}` substitution respectively).

---

## [0.1.0] — 2026-02-20

### Added
- Initial implementation of the LoRa Basics Station LNS protocol (WebSocket, v2):
  router discovery, uplink forwarding (`jreq`, `updf`, `propdf`), downlink handling
  (`dnmsg`, `dnsched`), TX confirmation (`dntxed`), time synchronization, and dynamic
  channel-plan configuration via `router_config`.
- CUPS (Configuration and Update Server) HTTPS client: periodic update checks,
  credential persistence, CRC32-based diff to avoid unnecessary credential rotation.
- ChirpStack Concentratord backend (ZMQ IPC).
- TLS authentication modes: server-only, mutual TLS, and token-based (Authorization header).
- CRC filtering: configurable forwarding of ok/invalid/missing CRC frames.
- Multi-file TOML configuration with `${VAR_NAME}` environment-variable substitution.
- `configfile` subcommand to print a commented configuration template.
- Dockerfile and `docker-compose.yml` for running alongside ChirpStack Concentratord.
- Fake Concentratord example (`examples/fake_concentratord.rs`) for integration testing
  without real gateway hardware.
- MIT license.
