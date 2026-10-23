//! Opt-in local UDP discovery compatible with Jellyfin's v12.3 request and reply.
//! This does not enable Jellyfin login or TV streaming by itself.
use std::{io, net::{IpAddr, Ipv4Addr, SocketAddr}};

use serde::Serialize;
use tokio::net::UdpSocket;

pub const DISCOVERY_PORT: u16 = 7359;
const MAX_REQUEST_BYTES: usize = 2048;

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct DiscoveryResponse<'a> {
    address: &'a str,
    id: &'a str,
    name: &'static str,
    endpoint_address: Option<&'a str>,
}

/// Requires the documented discovery phrase. Arbitrary UDP payloads do not get a reply.
pub fn matches_request(packet: &[u8]) -> bool {
    std::str::from_utf8(packet)
        .is_ok_and(|text| text.to_ascii_lowercase().contains("who is jellyfinserver?"))
}

fn lan_peer(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(ip) => ip.is_private() || ip.is_link_local() || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

fn response(url: &str, id: &str) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&DiscoveryResponse {
        address: url,
        id,
        name: "Pelagos",
        endpoint_address: None,
    })
    .map_err(io::Error::other)
}

/// Obtain the server's outgoing LAN interface address as seen from the requester.
/// Never advertise 0.0.0.0, an unreachable loopback address or a guessed interface.
async fn url_for_peer(peer: SocketAddr, http_port: u16) -> io::Result<String> {
    let bind = match peer {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => "[::]:0",
    };
    let probe = UdpSocket::bind(bind).await?;
    probe.connect(peer).await?;
    let ip = probe.local_addr()?.ip();
    match ip {
        IpAddr::V4(address) => Ok(format!("http://{address}:{http_port}")),
        IpAddr::V6(address) => Ok(format!("http://[{address}]:{http_port}")),
    }
}

/// Runs until the caller cancels the task or drops its async runtime.
/// The socket is supplied by the caller so a binding conflict with Jellyfin is
/// detected before the HTTP server starts.
pub async fn serve(socket: UdpSocket, http_port: u16, server_id: String) -> io::Result<()> {
    let mut buffer = [0u8; MAX_REQUEST_BYTES];
    loop {
        let (count, peer) = socket.recv_from(&mut buffer).await?;
        if !lan_peer(peer.ip()) || !matches_request(&buffer[..count]) {
            continue;
        }
        match url_for_peer(peer, http_port).await {
            Ok(url) => {
                let packet = response(&url, &server_id)?;
                socket.send_to(&packet, peer).await?;
            }
            Err(error) => {
                tracing::warn!(%peer, %error, "cannot choose interface for discovery reply");
            }
        }
    }
}

pub async fn bind_default() -> io::Result<UdpSocket> {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tokio::time::{timeout, Duration};

    #[test]
    fn matches_real_discovery_phrase_only() {
        assert!(matches_request(b"who is JellyfinServer?"));
        assert!(matches_request(b"WHO IS JELLYFINSERVER?"));
        assert!(!matches_request(b"who is Pelagos?"));
        assert!(!matches_request(&[0xff, 0xfe, 0xfd]));
    }

    #[test]
    fn payload_matches_jellyfin_discovery_properties() {
        let payload = response("http://192.168.1.25:8097", "sample-id").unwrap();
        let json: Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["Address"], "http://192.168.1.25:8097");
        assert_eq!(json["Id"], "sample-id");
        assert_eq!(json["Name"], "Pelagos");
        assert!(json["EndpointAddress"].is_null());
    }

    #[tokio::test]
    async fn loopback_udp_discovery_roundtrip() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let job = tokio::spawn(serve(server, 8097, "synthetic-id".to_string()));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"who is JellyfinServer?", address).await.unwrap();
        let mut buf = [0u8; 2048];
        let (count, _) = timeout(Duration::from_secs(2), client.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let json: Value = serde_json::from_slice(&buf[..count]).unwrap();
        assert_eq!(json["Address"], "http://127.0.0.1:8097");
        assert_eq!(json["Id"], "synthetic-id");
        job.abort();
    }

    #[test]
    fn external_public_ipv4_cannot_request() {
        assert!(!lan_peer("8.8.8.8".parse().unwrap()));
        assert!(lan_peer("192.168.1.15".parse().unwrap()));
    }
}
