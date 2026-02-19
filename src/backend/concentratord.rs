use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chirpstack_api::{gw, prost::Message};
use log::{error, info, trace};

use super::Backend as BackendTrait;
use crate::config::Configuration;

pub struct Backend {
    gateway_id: String,
    ctx: zmq::Context,
    cmd_url: String,
    cmd_sock: Mutex<zmq::Socket>,
}

impl Backend {
    pub async fn setup(conf: &Configuration) -> Result<Self> {
        info!("Setting up ChirpStack Concentratord backend");

        let zmq_ctx = zmq::Context::new();

        info!(
            "Connecting to Concentratord event API, event_url: {}",
            conf.backend.concentratord.event_url
        );
        let event_sock = zmq_ctx.socket(zmq::SUB)?;
        event_sock.connect(&conf.backend.concentratord.event_url)?;
        event_sock.set_subscribe("".as_bytes())?;

        info!(
            "Connecting to Concentratord command API, command_url: {}",
            conf.backend.concentratord.command_url
        );
        let cmd_sock = zmq_ctx.socket(zmq::REQ)?;
        cmd_sock.set_sndtimeo(1000)?;
        cmd_sock.connect(&conf.backend.concentratord.command_url)?;

        info!("Reading gateway id");

        // Request Gateway ID.
        let req = gw::Command {
            command: Some(gw::command::Command::GetGatewayId(
                gw::GetGatewayIdRequest {},
            )),
        };
        cmd_sock.send(req.encode_to_vec(), 0)?;

        // set poller so that we can timeout after 100ms
        let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
        zmq::poll(&mut items, 100)?;
        if !items[0].is_readable() {
            return Err(anyhow!("Could not read gateway id"));
        }

        // Read response.
        let resp = cmd_sock.recv_bytes(0)?;
        let resp = gw::GetGatewayIdResponse::decode(resp.as_slice())?;
        if resp.gateway_id.len() != 16 {
            return Err(anyhow!(
                "Invalid Gateway ID length, gateway_id: {}",
                resp.gateway_id
            ));
        }
        info!("Received gateway id, gateway_id: {}", resp.gateway_id);

        let forward_crc_ok = conf.backend.filters.forward_crc_ok;
        let forward_crc_invalid = conf.backend.filters.forward_crc_invalid;
        let forward_crc_missing = conf.backend.filters.forward_crc_missing;

        let event_sock = Arc::new(Mutex::new(event_sock));

        tokio::spawn({
            let event_sock = event_sock.clone();
            async move {

                event_loop(
                    event_sock,
                    forward_crc_ok,
                    forward_crc_invalid,
                    forward_crc_missing,
                )
                .await;
            }
        });

        Ok(Backend {
            gateway_id: resp.gateway_id,
            ctx: zmq_ctx,
            cmd_url: conf.backend.concentratord.command_url.clone(),
            cmd_sock: Mutex::new(cmd_sock),
        })
    }

    fn send_command(&self, cmd: gw::Command) -> Result<Vec<u8>> {
        let res = || -> Result<Vec<u8>> {
            let cmd_sock = self.cmd_sock.lock().unwrap();
            cmd_sock.send(cmd.encode_to_vec(), 0)?;

            // set poller so that we can timeout after 100ms
            let mut items = [cmd_sock.as_poll_item(zmq::POLLIN)];
            zmq::poll(&mut items, 100)?;
            if !items[0].is_readable() {
                return Err(anyhow!("Could not read response"));
            }

            let resp_b = cmd_sock.recv_bytes(0)?;
            Ok(resp_b)
        }();

        if res.is_err() {
            loop {
                if let Err(e) = self.reconnect_cmd_sock() {
                    error!(
                        "Re-connecting to Concentratord command API error, error: {}",
                        e
                    );
                    sleep(Duration::from_secs(1));
                    continue;
                }

                break;
            }
        }

        res
    }

    fn reconnect_cmd_sock(&self) -> Result<()> {
        info!(
            "Re-connecting to Concentratord command API, command_url: {}",
            self.cmd_url
        );
        let mut cmd_sock = self.cmd_sock.lock().unwrap();
        *cmd_sock = self.ctx.socket(zmq::REQ)?;
        cmd_sock.set_sndtimeo(1000)?;
        cmd_sock.connect(&self.cmd_url)?;
        Ok(())
    }

}

#[async_trait]
impl BackendTrait for Backend {
    async fn get_gateway_id(&self) -> Result<String> {
        Ok(self.gateway_id.clone())
    }

    async fn send_downlink_frame(&self, pl: gw::DownlinkFrame) -> Result<gw::DownlinkTxAck> {
        info!("Sending downlink frame, downlink_id: {}", pl.downlink_id);
        let downlink_id = pl.downlink_id;

        let cmd = gw::Command {
            command: Some(gw::command::Command::SendDownlinkFrame(pl)),
        };
        let resp_b = self.send_command(cmd)?;
        let tx_ack = gw::DownlinkTxAck::decode(resp_b.as_slice())?;

        let ack_items: Vec<String> = tx_ack
            .items
            .iter()
            .map(|i| i.status().as_str_name().to_string())
            .collect();

        info!(
            "Received ack, items: {:?}, downlink_id: {}",
            ack_items, downlink_id
        );

        Ok(tx_ack)
    }

    async fn send_configuration_command(&self, pl: gw::GatewayConfiguration) -> Result<()> {
        info!("Sending configuration command");

        let cmd = gw::Command {
            command: Some(gw::command::Command::SetGatewayConfiguration(pl)),
        };
        let _ = self.send_command(cmd);
        
        info!("Concentratord reconfigured, event socket reconnected");
        Ok(())
    }
}

async fn event_loop(
    event_sock: Arc<Mutex<zmq::Socket>>,
    forward_crc_ok: bool,
    forward_crc_invalid: bool,
    forward_crc_missing: bool,
) {
    trace!("Starting event loop");

    loop {
        let event = tokio::task::spawn_blocking({
            let event_sock = event_sock.clone();

            move || -> Result<Option<gw::Event>> {
                let event_sock = event_sock.lock().unwrap();

                // set poller so that we can timeout after 100ms
                let mut items = [event_sock.as_poll_item(zmq::POLLIN)];
                zmq::poll(&mut items, 100)?;
                if !items[0].is_readable() {
                    return Ok(None);
                }

                let msg = event_sock.recv_bytes(0)?;
                Ok(Some(gw::Event::decode(msg.as_slice())?))
            }
        })
        .await;

        let event = match event {
            Ok(v) => v,
            Err(e) => {
                error!("Task error: {}", e);
                continue;
            }
        };

        match event {
            Ok(Some(v)) => {
                if let Err(err) = handle_event_msg(
                    v,
                    forward_crc_ok,
                    forward_crc_invalid,
                    forward_crc_missing,
                )
                .await
                {
                    error!("Handle event error: {}", err);
                    continue;
                }
            }
            Ok(None) => continue,
            Err(e) => {
                error!("Error reading event, error: {}", e);
                continue;
            }
        }
    }
}

async fn handle_event_msg(
    event: gw::Event,
    forward_crc_ok: bool,
    forward_crc_invalid: bool,
    forward_crc_missing: bool,
) -> Result<()> {
    match event.event {
        Some(gw::event::Event::UplinkFrame(v)) => {
            if let Some(rx_info) = &v.rx_info
                && !((rx_info.crc_status() == gw::CrcStatus::CrcOk && forward_crc_ok)
                    || (rx_info.crc_status() == gw::CrcStatus::BadCrc && forward_crc_invalid)
                    || (rx_info.crc_status() == gw::CrcStatus::NoCrc && forward_crc_missing))
            {
                trace!(
                    "Ignoring uplink frame because of forward_crc_ flags, uplink_id: {}",
                    v.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
                );
                return Ok(());
            }

            info!(
                "Received uplink frame, uplink_id: {}",
                v.rx_info.as_ref().map(|v| v.uplink_id).unwrap_or_default(),
            );
            crate::lns::send_uplink(&v).await?;
        }
        Some(gw::event::Event::GatewayStats(_v)) => {
            // BasicStation protocol does not have a stats message.
            // Stats are tracked internally by the LNS.
            trace!("Received gateway stats (not forwarded in BasicStation protocol)");
        }
        Some(gw::event::Event::Mesh(_v)) => {
            trace!("Received mesh event (not supported in BasicStation protocol)");
        }
        None => {}
    }

    Ok(())
}
