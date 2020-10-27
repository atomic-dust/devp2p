use crate::types::*;
use anyhow::anyhow;
use async_trait::async_trait;
use std::{collections::HashMap, net::SocketAddr};

#[cfg(feature = "discv4")]
mod discv4;

#[cfg(feature = "discv4")]
pub use self::discv4::Discv4;

#[cfg(feature = "discv5")]
mod discv5;

#[cfg(feature = "discv5")]
pub use self::discv5::Discv5;

#[cfg(feature = "dnsdisc")]
mod dnsdisc;

#[cfg(feature = "dnsdisc")]
pub use self::dnsdisc::DnsDiscovery;

#[async_trait]
pub trait Discovery: Send + 'static {
    async fn get_new_peer(&mut self) -> anyhow::Result<NodeRecord>;
}

#[async_trait]
impl<S: Send + 'static> Discovery for HashMap<SocketAddr, PeerId, S> {
    async fn get_new_peer(&mut self) -> anyhow::Result<NodeRecord> {
        Ok(self
            .iter()
            .next()
            .map(|(&addr, &id)| NodeRecord { id, addr })
            .ok_or_else(|| anyhow!("No peers in set"))?)
    }
}
