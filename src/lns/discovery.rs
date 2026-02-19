use std::sync::Arc;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use log::debug;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::tungstenite::Message;

use super::messages::RouterInfoResponse;

/// Format a gateway ID (16 hex chars) as BasicStation ID6 format.
/// ID6 mimics IPv6: four groups of 4 hex chars separated by colons.
/// Example: "0016C001FF10A235" → "0016:c001:ff10:a235"
pub fn gateway_id_to_id6(gateway_id: &str) -> Result<String> {
    if gateway_id.len() != 16 {
        return Err(anyhow!(
            "Invalid gateway ID length: {}, expected 16 hex chars",
            gateway_id.len()
        ));
    }

    let id = gateway_id.to_lowercase();
    Ok(format!(
        "{}:{}:{}:{}",
        &id[0..4],
        &id[4..8],
        &id[8..12],
        &id[12..16]
    ))
}

/// Perform BasicStation router discovery via WebSocket.
///
/// Opens a WebSocket connection to <endpoint>/router-info, sends the gateway's
/// router ID as a JSON message, and receives the MUXS WebSocket URI in response.
pub async fn discover(
    endpoint: &str,
    gateway_id: &str,
    auth_headers: &[(String, String)],
    tls_connector: Arc<rustls::ClientConfig>,
) -> Result<String> {
    let id6 = gateway_id_to_id6(gateway_id)?;
    let url = format!("{}/router-info", endpoint.trim_end_matches('/'));

    debug!(
        "Performing router discovery, url: {}, router: {}",
        url, id6
    );

    let mut request = url.into_client_request()?;
    for (name, value) in auth_headers {
        request.headers_mut().insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value)?,
        );
    }

    let (mut ws_stream, _resp) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(tls_connector)),
    )
    .await?;

    // Send router info request.
    let body = serde_json::json!({ "router": id6 });
    ws_stream
        .send(Message::Text(body.to_string().into()))
        .await?;

    // Read response.
    let response = ws_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("Router discovery: no response received"))??;

    // Close the WebSocket.
    let _ = ws_stream.close(None).await;

    let text = match response {
        Message::Text(t) => t,
        other => return Err(anyhow!("Router discovery: unexpected message type: {:?}", other)),
    };

    let info: RouterInfoResponse = serde_json::from_str(&text)?;

    if let Some(error) = &info.error {
        return Err(anyhow!("Router discovery error: {}", error));
    }

    info.uri
        .ok_or_else(|| anyhow!("Router discovery response missing 'uri' field"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_id_to_id6() {
        assert_eq!(
            gateway_id_to_id6("0016C001FF10A235").unwrap(),
            "0016:c001:ff10:a235"
        );
        assert_eq!(
            gateway_id_to_id6("AA555A0000000000").unwrap(),
            "aa55:5a00:0000:0000"
        );
    }

    #[test]
    fn test_gateway_id_to_id6_invalid() {
        assert!(gateway_id_to_id6("0016C001").is_err());
        assert!(gateway_id_to_id6("").is_err());
    }
}
