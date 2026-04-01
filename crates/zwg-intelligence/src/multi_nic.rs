//! Multi-NIC bonding — distribute HTTP requests across Ethernet + Wi-Fi.
//!
//! Discovers active network interfaces and provides round-robin or
//! fastest-wins request distribution for external API calls (remote Ollama,
//! cloud LLM endpoints, model downloads, etc.).
//!
//! # How it works
//!
//! ```text
//! Ethernet (192.168.1.x) ──┐
//!                           ├── MultiNicDispatcher (round-robin / race)
//! Wi-Fi    (192.168.1.y) ──┘     → binds socket to local IP → connects
//! ```

use anyhow::{anyhow, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Network interface kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NicKind {
    Ethernet,
    WiFi,
    Other,
}

/// Discovered network interface with IPv4 address.
#[derive(Debug, Clone)]
pub struct NicInfo {
    pub name: String,
    pub ip: Ipv4Addr,
    pub kind: NicKind,
}

/// Discover active physical NICs with IPv4 addresses.
///
/// Uses PowerShell `Get-NetAdapter` + `Get-NetIPAddress` for reliable
/// enumeration on Windows. Only returns adapters that are Up and have
/// a valid IPv4 address.
pub fn discover_nics() -> Vec<NicInfo> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            r#"Get-NetAdapter -Physical | Where-Object {$_.Status -eq 'Up'} | ForEach-Object {
                $ip = (Get-NetIPAddress -InterfaceIndex $_.InterfaceIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue).IPAddress
                if ($ip) { "$($_.Name)|$ip|$($_.InterfaceDescription)" }
            }"#,
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.trim().split('|').collect();
            if parts.len() < 3 {
                return None;
            }
            let ip: Ipv4Addr = parts[1].parse().ok()?;
            // Skip loopback / link-local
            if ip.is_loopback() || ip.octets()[0] == 169 {
                return None;
            }
            let desc = parts[2].to_lowercase();
            let kind = if desc.contains("wi-fi")
                || desc.contains("wifi")
                || desc.contains("wireless")
                || desc.contains("wlan")
            {
                NicKind::WiFi
            } else if desc.contains("ethernet")
                || desc.contains("realtek")
                || desc.contains("intel")
                || desc.contains("killer")
            {
                NicKind::Ethernet
            } else {
                NicKind::Other
            };
            Some(NicInfo {
                name: parts[0].to_string(),
                ip,
                kind,
            })
        })
        .collect()
}

/// Round-robin dispatcher across multiple NICs.
pub struct MultiNicDispatcher {
    nics: Vec<NicInfo>,
    counter: AtomicUsize,
}

impl MultiNicDispatcher {
    /// Auto-discover NICs and create dispatcher.
    /// Returns `None` if fewer than 2 usable NICs are found.
    pub fn auto() -> Option<Self> {
        let nics = discover_nics();
        if nics.len() < 2 {
            return None;
        }
        log::info!(
            "Multi-NIC bonding: {} interfaces discovered: {}",
            nics.len(),
            nics.iter()
                .map(|n| format!("{}({})", n.name, n.ip))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Some(Self {
            nics,
            counter: AtomicUsize::new(0),
        })
    }

    /// Create from an explicit NIC list.
    pub fn with_nics(nics: Vec<NicInfo>) -> Option<Self> {
        if nics.is_empty() {
            return None;
        }
        Some(Self {
            nics,
            counter: AtomicUsize::new(0),
        })
    }

    /// Get the next NIC in round-robin order.
    pub fn next_nic(&self) -> &NicInfo {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.nics.len();
        &self.nics[idx]
    }

    /// Number of available NICs.
    pub fn nic_count(&self) -> usize {
        self.nics.len()
    }

    /// Get all NICs.
    pub fn nics(&self) -> &[NicInfo] {
        &self.nics
    }

    /// Create a TCP stream bound to the next NIC in rotation.
    pub fn connect_round_robin(&self, remote: SocketAddr) -> Result<TcpStream> {
        let nic = self.next_nic();
        connect_via(nic.ip, remote)
    }

    /// Race connections across all NICs, return the first to succeed.
    /// Useful for latency-sensitive single requests.
    pub fn connect_fastest(&self, remote: SocketAddr) -> Result<(TcpStream, NicInfo)> {
        let (tx, rx) = std::sync::mpsc::channel();

        for nic in &self.nics {
            let nic_clone = nic.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Ok(stream) = connect_via(nic_clone.ip, remote) {
                    let _ = tx.send((stream, nic_clone));
                }
            });
        }
        drop(tx);

        rx.recv()
            .map_err(|_| anyhow!("all NIC connections failed"))
    }

    /// HTTP POST through the next NIC (for Ollama / LLM API calls).
    ///
    /// This is a raw HTTP/1.1 POST — no TLS. Suitable for local-network
    /// or VPN endpoints (remote Ollama, etc.).
    pub fn http_post_json(
        &self,
        host: &str,
        port: u16,
        path: &str,
        body: &[u8],
    ) -> Result<Vec<u8>> {
        let remote = resolve_addr(host, port)?;
        let mut stream = self.connect_round_robin(remote)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;

        let header = format!(
            "POST {path} HTTP/1.1\r\n\
             Host: {host}:{port}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes())?;
        stream.write_all(body)?;
        stream.flush()?;

        let mut reader = BufReader::new(&stream);
        let mut content_length: Option<usize> = None;

        // Parse HTTP response headers
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("Content-Length:") {
                content_length = rest.trim().parse().ok();
            } else if let Some(rest) = trimmed.strip_prefix("content-length:") {
                content_length = rest.trim().parse().ok();
            }
        }

        if let Some(len) = content_length {
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            Ok(buf)
        } else {
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf)?;
            Ok(buf)
        }
    }

    /// Download a large resource using parallel chunk requests across NICs.
    ///
    /// Splits the download into `chunk_count` ranges and fetches them
    /// through different NICs simultaneously. Requires the server to
    /// support HTTP Range requests.
    pub fn parallel_download(
        &self,
        host: &str,
        port: u16,
        path: &str,
        total_size: usize,
    ) -> Result<Vec<u8>> {
        let chunk_count = self.nics.len();
        let chunk_size = (total_size + chunk_count - 1) / chunk_count;
        let mut handles = Vec::new();

        for i in 0..chunk_count {
            let start = i * chunk_size;
            let end = std::cmp::min(start + chunk_size - 1, total_size - 1);
            let host = host.to_string();
            let path = path.to_string();
            let nic_ip = self.nics[i % self.nics.len()].ip;

            handles.push(std::thread::spawn(move || -> Result<(usize, Vec<u8>)> {
                let remote = resolve_addr(&host, port)?;
                let mut stream = connect_via(nic_ip, remote)?;
                stream.set_read_timeout(Some(Duration::from_secs(60)))?;

                let header = format!(
                    "GET {path} HTTP/1.1\r\n\
                     Host: {host}:{port}\r\n\
                     Range: bytes={start}-{end}\r\n\
                     Connection: close\r\n\r\n"
                );
                stream.write_all(header.as_bytes())?;
                stream.flush()?;

                let mut reader = BufReader::new(&stream);
                // Skip headers
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line)?;
                    if line.trim().is_empty() {
                        break;
                    }
                }

                let mut buf = Vec::new();
                reader.read_to_end(&mut buf)?;
                Ok((start, buf))
            }));
        }

        let mut result = vec![0u8; total_size];
        for handle in handles {
            let (offset, data) = handle
                .join()
                .map_err(|_| anyhow!("download thread panicked"))??;
            let end = std::cmp::min(offset + data.len(), total_size);
            result[offset..end].copy_from_slice(&data[..end - offset]);
        }

        Ok(result)
    }
}

/// Create a TCP connection bound to a specific local IPv4 address.
fn connect_via(local_ip: Ipv4Addr, remote: SocketAddr) -> Result<TcpStream> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    // Bind to the specific NIC (port 0 = OS picks ephemeral port)
    let local = SocketAddrV4::new(local_ip, 0);
    socket.bind(&socket2::SockAddr::from(local))?;
    socket.connect(&socket2::SockAddr::from(remote))?;
    Ok(TcpStream::from(socket))
}

/// Resolve hostname:port to a SocketAddr.
fn resolve_addr(host: &str, port: u16) -> Result<SocketAddr> {
    format!("{host}:{port}")
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("failed to resolve {host}:{port}"))
}

impl std::fmt::Display for NicKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ethernet => write!(f, "Ethernet"),
            Self::WiFi => write!(f, "Wi-Fi"),
            Self::Other => write!(f, "Other"),
        }
    }
}

impl std::fmt::Display for NicInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}, {})", self.name, self.ip, self.kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_nics_no_panic() {
        let nics = discover_nics();
        for nic in &nics {
            assert!(!nic.name.is_empty());
            assert_ne!(nic.ip, Ipv4Addr::UNSPECIFIED);
        }
    }

    #[test]
    fn nic_kind_display() {
        assert_eq!(NicKind::Ethernet.to_string(), "Ethernet");
        assert_eq!(NicKind::WiFi.to_string(), "Wi-Fi");
        assert_eq!(NicKind::Other.to_string(), "Other");
    }

    #[test]
    fn round_robin_cycles() {
        let nics = vec![
            NicInfo {
                name: "Ethernet".into(),
                ip: Ipv4Addr::new(192, 168, 1, 100),
                kind: NicKind::Ethernet,
            },
            NicInfo {
                name: "Wi-Fi".into(),
                ip: Ipv4Addr::new(192, 168, 1, 101),
                kind: NicKind::WiFi,
            },
        ];
        let d = MultiNicDispatcher::with_nics(nics).unwrap();
        let a = d.next_nic().ip;
        let b = d.next_nic().ip;
        let c = d.next_nic().ip;
        assert_ne!(a, b);
        assert_eq!(a, c); // wraps around
    }

    #[test]
    fn dispatcher_none_for_empty() {
        assert!(MultiNicDispatcher::with_nics(vec![]).is_none());
    }

    #[test]
    fn resolve_localhost() {
        let addr = resolve_addr("127.0.0.1", 11434).unwrap();
        assert_eq!(addr.port(), 11434);
    }
}
