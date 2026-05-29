//! Loopback TCP → SOCKS5 forwarder.
//!
//! mysql_async 0.34 has no hook to inject a pre-dialed stream (`Endpoint` enum
//! is closed, `OptsBuilder` exposes only direct TCP knobs). So to route MySQL
//! through a SOCKS5 proxy we bind an ephemeral loopback listener, point
//! mysql_async at it, and tunnel each accepted connection through the proxy
//! via `tokio-socks` + `copy_bidirectional`.
//!
//! The forwarder must outlive the MySQL connection — store the returned handle
//! on the connection struct and let Drop abort the accept task.

use crate::config::Socks5Config;
use crate::error::{VeloError, VeloResult};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_socks::tcp::Socks5Stream;

pub struct Socks5Forwarder {
    pub local_addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Socks5Forwarder {
    /// Bind an ephemeral loopback port and start forwarding every accepted
    /// connection through the SOCKS5 proxy to `target_host:target_port`.
    pub async fn spawn(
        socks5: &Socks5Config,
        target_host: String,
        target_port: u16,
    ) -> VeloResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
            VeloError::connection_with_source(
                "Failed to bind SOCKS5 forwarder loopback listener".to_string(),
                e,
            )
        })?;
        let local_addr = listener.local_addr().map_err(|e| {
            VeloError::connection_with_source(
                "Failed to read SOCKS5 forwarder local addr".to_string(),
                e,
            )
        })?;

        let s5 = socks5.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut inbound, _peer) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                let s5 = s5.clone();
                let host = target_host.clone();
                let port = target_port;
                tokio::spawn(async move {
                    let proxy_addr = format!("{}:{}", s5.host, s5.port);
                    let mut outbound = match Socks5Stream::connect_with_password(
                        proxy_addr.as_str(),
                        (host.as_str(), port),
                        &s5.user,
                        &s5.pass,
                    )
                    .await
                    {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::warn!(
                                target = "velo::socks5",
                                "SOCKS5 dial to {host}:{port} via {proxy_addr} failed: {e}"
                            );
                            return;
                        }
                    };
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                });
            }
        });

        Ok(Socks5Forwarder { local_addr, task })
    }
}

impl Drop for Socks5Forwarder {
    fn drop(&mut self) {
        self.task.abort();
    }
}
