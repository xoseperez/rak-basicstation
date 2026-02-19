//! Fake Concentratord for testing.
//!
//! Mimics concentratord's ZMQ API: responds to GetGatewayId on a REP socket
//! and publishes a test UplinkFrame on a PUB socket.
//!
//! Usage:
//!   1. Edit examples/fake_concentratord.toml with your gateway_id and URLs.
//!   2. Update rak-basicstation.toml to match the URLs:
//!        event_url   = "ipc:///tmp/test_concentratord_event"
//!        command_url = "ipc:///tmp/test_concentratord_command"
//!   3. Run this example:
//!        cargo run --example fake_concentratord
//!      or with a custom config:
//!        cargo run --example fake_concentratord -- -c /path/to/config.toml
//!   4. Run the main service in another terminal.

use chirpstack_api::{gw, prost::Message};
use serde::Deserialize;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct Config {
    gateway_id: String,
    #[serde(default = "default_stats_interval")]
    stats_interval: u64,
    api: ApiConfig,
    uplink: UplinkConfig,
}

#[derive(Debug, Deserialize)]
struct ApiConfig {
    event_bind: String,
    command_bind: String,
}

#[derive(Debug, Deserialize)]
struct UplinkConfig {
    #[serde(default = "default_frequency")]
    frequency: u32,
    #[serde(default = "default_sf")]
    spreading_factor: u32,
    #[serde(default = "default_bandwidth")]
    bandwidth: u32,
    #[serde(default = "default_uplink_interval")]
    interval: u64,
}

fn default_frequency() -> u32 { 868_100_000 }
fn default_sf() -> u32 { 7 }
fn default_bandwidth() -> u32 { 125_000 }
fn default_uplink_interval() -> u64 { 10 }
fn default_stats_interval() -> u64 { 30 }

fn main() {
    let config_path = parse_args();
    let config_str = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("Failed to read config '{}': {}", config_path, e));
    let config: Config = toml::from_str(&config_str)
        .unwrap_or_else(|e| panic!("Failed to parse config '{}': {}", config_path, e));

    println!("Fake Concentratord starting...");
    println!("  Config:       {}", config_path);
    println!("  Gateway ID:   {}", config.gateway_id);
    println!("  Event bind:   {}", config.api.event_bind);
    println!("  Command bind: {}", config.api.command_bind);
    println!(
        "  Uplink:       {} Hz, SF{}, BW {} Hz, every {}s",
        config.uplink.frequency,
        config.uplink.spreading_factor,
        config.uplink.bandwidth,
        config.uplink.interval,
    );
    println!("  Stats:        every {}s", config.stats_interval);

    // Clean up stale IPC sockets.
    if let Some(path) = config.api.event_bind.strip_prefix("ipc://") {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = config.api.command_bind.strip_prefix("ipc://") {
        let _ = std::fs::remove_file(path);
    }

    let ctx = zmq::Context::new();

    // PUB socket for events.
    let event_sock = ctx.socket(zmq::PUB).expect("PUB socket");
    event_sock
        .bind(&config.api.event_bind)
        .expect("bind PUB socket");
    println!("PUB socket bound");

    // REP socket for commands.
    let cmd_sock = ctx.socket(zmq::REP).expect("REP socket");
    cmd_sock
        .bind(&config.api.command_bind)
        .expect("bind REP socket");
    println!("REP socket bound");

    let config = Arc::new(config);

    // Spawn command handler thread.
    thread::spawn({
        let config = config.clone();
        move || {
            println!("Command handler ready");
            command_loop(&cmd_sock, &config.gateway_id);
        }
    });

    // Wait for subscriber to connect.
    println!("\nWaiting 3 seconds for subscribers...");
    thread::sleep(Duration::from_secs(3));

    println!("Publishing events...\n");

    let mut uplink_count: u32 = 0;
    let mut tick: u64 = 0;

    loop {
        tick += 1;

        if config.stats_interval > 0 && tick % config.stats_interval == 0 {
            let stats_event = gw::Event {
                event: Some(gw::event::Event::GatewayStats(gw::GatewayStats {
                    gateway_id: config.gateway_id.clone(),
                    ..Default::default()
                })),
            };
            event_sock
                .send(stats_event.encode_to_vec(), 0)
                .expect("send stats");
            println!("[tick {}] Published gateway stats", tick);
        }

        if config.uplink.interval > 0 && tick % config.uplink.interval == 0 {
            uplink_count += 1;
            publish_test_uplink(&event_sock, &config, uplink_count);
        }

        thread::sleep(Duration::from_secs(1));
    }
}

fn parse_args() -> String {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "-c" && i + 1 < args.len() {
            return args[i + 1].clone();
        }
        i += 1;
    }

    // Default: look for config next to the example source.
    let default = "examples/fake_concentratord.toml";
    if std::path::Path::new(default).exists() {
        return default.to_string();
    }

    eprintln!("Usage: fake_concentratord [-c CONFIG_FILE]");
    eprintln!("Default config: {}", default);
    std::process::exit(1);
}

fn command_loop(cmd_sock: &zmq::Socket, gateway_id: &str) {
    loop {
        let msg = match cmd_sock.recv_bytes(0) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("cmd recv error: {}", e);
                continue;
            }
        };

        let cmd = match gw::Command::decode(msg.as_slice()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("cmd decode error: {}", e);
                cmd_sock.send(&b""[..], 0).unwrap();
                continue;
            }
        };

        match cmd.command {
            Some(gw::command::Command::GetGatewayId(_)) => {
                println!("  -> GetGatewayId, responding: {}", gateway_id);
                let resp = gw::GetGatewayIdResponse {
                    gateway_id: gateway_id.to_string(),
                };
                cmd_sock.send(resp.encode_to_vec(), 0).unwrap();
            }
            Some(gw::command::Command::SetGatewayConfiguration(cfg)) => {
                println!(
                    "  -> SetGatewayConfiguration, channels: {}",
                    cfg.channels.len()
                );
                cmd_sock.send(&b""[..], 0).unwrap();
            }
            Some(gw::command::Command::SendDownlinkFrame(dl)) => {
                println!("  -> SendDownlinkFrame, id: {}", dl.downlink_id);
                let ack = gw::DownlinkTxAck {
                    gateway_id: gateway_id.to_string(),
                    downlink_id: dl.downlink_id,
                    items: vec![gw::DownlinkTxAckItem {
                        status: gw::TxAckStatus::Ok.into(),
                    }],
                    ..Default::default()
                };
                cmd_sock.send(ack.encode_to_vec(), 0).unwrap();
            }
            other => {
                println!("  -> Unknown command: {:?}", other);
                cmd_sock.send(&b""[..], 0).unwrap();
            }
        }
    }
}

fn publish_test_uplink(sock: &zmq::Socket, config: &Config, count: u32) {
    // Build a fake unconfirmed data up LoRaWAN frame.
    // MHDR=0x40 (unconfirmed data up), DevAddr, FCtrl, FCnt, FPort, Payload, MIC
    let mhdr: u8 = 0x40;
    let dev_addr: [u8; 4] = [0x34, 0x12, 0x01, 0x26]; // 0x26011234 in LE
    let fctrl: u8 = 0x00;
    let fcnt: [u8; 2] = (count as u16).to_le_bytes();
    let fport: u8 = 0x01;
    let payload: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
    let mic: [u8; 4] = [0x12, 0x34, 0x56, 0x78];

    let mut phy_payload = Vec::with_capacity(17);
    phy_payload.push(mhdr);
    phy_payload.extend_from_slice(&dev_addr);
    phy_payload.push(fctrl);
    phy_payload.extend_from_slice(&fcnt);
    phy_payload.push(fport);
    phy_payload.extend_from_slice(&payload);
    phy_payload.extend_from_slice(&mic);

    let uplink = gw::UplinkFrame {
        phy_payload,
        tx_info: Some(gw::UplinkTxInfo {
            frequency: config.uplink.frequency,
            modulation: Some(gw::Modulation {
                parameters: Some(gw::modulation::Parameters::Lora(gw::LoraModulationInfo {
                    bandwidth: config.uplink.bandwidth,
                    spreading_factor: config.uplink.spreading_factor,
                    code_rate: gw::CodeRate::Cr45.into(),
                    polarization_inversion: false,
                    ..Default::default()
                })),
            }),
        }),
        rx_info: Some(gw::UplinkRxInfo {
            uplink_id: count,
            context: vec![0x00, 0x01, 0x00, count as u8],
            rssi: -60,
            snr: 10.0,
            crc_status: gw::CrcStatus::CrcOk.into(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let event = gw::Event {
        event: Some(gw::event::Event::UplinkFrame(uplink)),
    };

    sock.send(event.encode_to_vec(), 0).expect("send uplink");
    println!(
        "[uplink #{}] Published: DevAddr=26011234, FCnt={}, SF{}/BW{}kHz, {} Hz",
        count,
        count,
        config.uplink.spreading_factor,
        config.uplink.bandwidth / 1000,
        config.uplink.frequency,
    );
}
