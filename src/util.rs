use async_trait::async_trait;
use ethereum_types::{H256, H512};
use futures::{future::abortable, Stream};
use hmac::{Hmac, Mac, NewMac};
use libsecp256k1::{self, PublicKey};
use log::*;
use parking_lot::Mutex;
use sha2::Sha256;
use sha3::{Digest, Keccak256};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::prelude::*;

pub struct TaskHandle<T>(Pin<Box<dyn Future<Output = Result<T, Shutdown>> + Send + 'static>>);

impl<T> Future for TaskHandle<T>
where
    T: Send + 'static,
{
    type Output = Result<T, Shutdown>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

#[derive(Clone, Debug)]
pub struct Shutdown;

/// Common trait that various runtimes should implement.
#[async_trait]
pub trait Runtime {
    /// Runtime's TCP stream type.
    type TcpStream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type TcpServer: Stream<Item = Self::TcpStream> + Unpin + Send + 'static;

    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>);
    async fn sleep(&self, duration: Duration);
    async fn connect_tcp(&self, target: String) -> io::Result<Self::TcpStream>;
    async fn tcp_server(&self, addr: String) -> io::Result<Self::TcpServer>;
}

#[derive(Default, Debug)]
pub struct TaskGroup(Mutex<Vec<futures::future::AbortHandle>>);

impl TaskGroup {
    pub fn spawn<Fut, T>(&self, future: Fut) -> TaskHandle<T>
    where
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let mut group = self.0.lock();
        let (t, handle) = abortable(future);
        group.push(handle);
        let spawned_handle = tokio::spawn(t);
        TaskHandle(Box::pin(async move {
            Ok(spawned_handle
                .await
                .map_err(|_| {
                    trace!("Runtime shutdown");
                    Shutdown
                })?
                .map_err(|_| {
                    trace!("Task group shutdown");
                    Shutdown
                })?)
        }))
    }
}

impl Drop for TaskGroup {
    fn drop(&mut self) {
        for handle in &*self.0.lock() {
            handle.abort();
        }
    }
}

pub fn keccak256(data: &[u8]) -> H256 {
    let mut hasher = Keccak256::new();
    hasher.update(data);
    let out = hasher.finalize();
    H256::from(out.as_ref())
}

pub fn sha256(data: &[u8]) -> H256 {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let out = hasher.finalize();
    H256::from(out.as_ref())
}

pub fn hmac_sha256(key: &[u8], input: &[u8]) -> H256 {
    let mut hmac = Hmac::<Sha256>::new_varkey(key).unwrap();
    hmac.update(input);
    H256::from_slice(&*hmac.finalize().into_bytes())
}

pub fn pk2id(pk: &PublicKey) -> H512 {
    H512::from_slice(&pk.serialize()[1..])
}

pub fn id2pk(id: H512) -> Result<PublicKey, libsecp256k1::Error> {
    let s: [u8; 64] = id.into();
    let mut sp: Vec<u8> = s.as_ref().into();
    let mut r = vec![0x04_u8];
    r.append(&mut sp);
    PublicKey::parse_slice(r.as_ref(), None)
}

#[cfg(test)]
mod tests {
    use crate::util::*;
    use libsecp256k1::{PublicKey, SecretKey};
    use rand::rngs::OsRng;

    #[test]
    fn pk2id2pk() {
        let prikey = SecretKey::random(&mut OsRng);
        let pubkey = PublicKey::from_secret_key(&prikey);
        assert_eq!(pubkey, id2pk(pk2id(&pubkey)).unwrap());
    }
}