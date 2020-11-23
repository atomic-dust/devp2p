use super::algorithm::ECIES;
use crate::{errors::ECIESError, transport::Transport, types::PeerId};
use anyhow::{bail, Context as _};
use bytes::{Bytes, BytesMut};
use futures::{ready, Sink, SinkExt};
use secp256k1::SecretKey;
use std::{
    fmt::Debug,
    io,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::stream::*;
use tokio_util::codec::*;
use tracing::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Current ECIES state of a connection
pub enum ECIESState {
    Auth,
    Ack,
    Header,
    Body,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Raw egress values for an ECIES protocol
pub enum EgressECIESValue {
    Auth,
    Ack,
    Message(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Raw ingress values for an ECIES protocol
pub enum IngressECIESValue {
    AuthReceive(PeerId),
    Ack,
    Message(Vec<u8>),
}

/// Tokio codec for ECIES
#[derive(Debug)]
pub struct ECIESCodec {
    ecies: ECIES,
    state: ECIESState,
}

impl ECIESCodec {
    /// Create a new server codec using the given secret key
    pub fn new_server(secret_key: SecretKey) -> Result<Self, ECIESError> {
        Ok(Self {
            ecies: ECIES::new_server(secret_key)?,
            state: ECIESState::Auth,
        })
    }

    /// Create a new client codec using the given secret key and the server's public id
    pub fn new_client(secret_key: SecretKey, remote_id: PeerId) -> Result<Self, ECIESError> {
        Ok(Self {
            ecies: ECIES::new_client(secret_key, remote_id)?,
            state: ECIESState::Auth,
        })
    }
}

impl Decoder for ECIESCodec {
    type Item = IngressECIESValue;
    type Error = io::Error;

    #[instrument(level = "trace", skip(self, buf), fields(peer=&*format!("{:?}", self.ecies.remote_id.map(|s| s.to_string())), state=&*format!("{:?}", self.state)))]
    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.state {
            ECIESState::Auth => {
                trace!("parsing auth");
                if buf.len() < 2 {
                    return Ok(None);
                }

                let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                let total_size = payload_size + 2;

                if buf.len() < total_size {
                    trace!("current len {}, need {}", buf.len(), total_size);
                    return Ok(None);
                }

                let data = buf.split_to(total_size);
                self.ecies.parse_auth(&data)?;

                self.state = ECIESState::Header;
                Ok(Some(IngressECIESValue::AuthReceive(self.ecies.remote_id())))
            }
            ECIESState::Ack => {
                trace!("parsing ack with len {}", buf.len());
                if buf.len() < 2 {
                    return Ok(None);
                }

                let payload_size = u16::from_be_bytes([buf[0], buf[1]]) as usize;
                let total_size = payload_size + 2;

                if buf.len() < total_size {
                    trace!("current len {}, need {}", buf.len(), total_size);
                    return Ok(None);
                }

                let data = buf.split_to(total_size);
                self.ecies.parse_ack(&data)?;

                self.state = ECIESState::Header;
                Ok(Some(IngressECIESValue::Ack))
            }
            ECIESState::Header => {
                if buf.len() < ECIES::header_len() {
                    return Ok(None);
                }

                let data = buf.split_to(ECIES::header_len());
                self.ecies.parse_header(&data)?;

                self.state = ECIESState::Body;
                Ok(None)
            }
            ECIESState::Body => {
                if buf.len() < self.ecies.body_len() {
                    return Ok(None);
                }

                let data = buf.split_to(self.ecies.body_len());
                let ret = self.ecies.parse_body(&data)?;

                self.state = ECIESState::Header;
                Ok(Some(IngressECIESValue::Message(ret)))
            }
        }
    }
}

impl Encoder<EgressECIESValue> for ECIESCodec {
    type Error = io::Error;

    #[instrument(level = "trace", skip(self, buf), fields(peer=&*format!("{:?}", self.ecies.remote_id.map(|s| s.to_string())), state=&*format!("{:?}", self.state)))]
    fn encode(&mut self, item: EgressECIESValue, buf: &mut BytesMut) -> Result<(), Self::Error> {
        match item {
            EgressECIESValue::Auth => {
                let data = self.ecies.create_auth();
                self.state = ECIESState::Ack;
                buf.extend_from_slice(&data);
                Ok(())
            }
            EgressECIESValue::Ack => {
                let data = self.ecies.create_ack();
                self.state = ECIESState::Header;
                buf.extend_from_slice(&data);
                Ok(())
            }
            EgressECIESValue::Message(data) => {
                buf.extend_from_slice(&self.ecies.create_header(data.len()));
                buf.extend_from_slice(&self.ecies.create_body(&data));
                Ok(())
            }
        }
    }
}

/// `ECIES` stream over TCP exchanging raw bytes
#[derive(Debug)]
pub struct ECIESStream<Io> {
    stream: Framed<Io, ECIESCodec>,
    remote_id: PeerId,
}

impl<Io> ECIESStream<Io>
where
    Io: Transport,
{
    /// Connect to an `ECIES` server
    #[instrument(skip(transport, secret_key), fields(peer=&*format!("{:?}", transport.remote_addr())))]
    pub async fn connect(
        transport: Io,
        secret_key: SecretKey,
        remote_id: PeerId,
    ) -> anyhow::Result<Self> {
        let ecies = ECIESCodec::new_client(secret_key, remote_id)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "invalid handshake"))?;

        let mut transport = ecies.framed(transport);

        trace!("sending ecies auth ...");
        transport.send(EgressECIESValue::Auth).await?;

        trace!("waiting for ecies ack ...");
        let ack = transport.try_next().await?;

        trace!("parsing ecies ack ...");
        if matches!(ack, Some(IngressECIESValue::Ack)) {
            Ok(Self {
                stream: transport,
                remote_id,
            })
        } else {
            bail!("invalid handshake: expected ack, got {:?} instead", ack)
        }
    }

    /// Listen on a just connected ECIES client
    #[instrument(skip(transport, secret_key), fields(peer=&*format!("{:?}", transport.remote_addr())))]
    pub async fn incoming(transport: Io, secret_key: SecretKey) -> anyhow::Result<Self> {
        let ecies = ECIESCodec::new_server(secret_key).context("handshake error")?;

        debug!("incoming ecies stream ...");
        let mut transport = ecies.framed(transport);
        let ack = transport.try_next().await?;

        debug!("receiving ecies auth");
        let remote_id = match ack {
            Some(IngressECIESValue::AuthReceive(remote_id)) => remote_id,
            other => {
                debug!("expected auth, got {:?} instead", other);
                bail!("invalid handshake");
            }
        };

        debug!("sending ecies ack ...");
        transport
            .send(EgressECIESValue::Ack)
            .await
            .context("failed to send ECIES auth")?;

        Ok(Self {
            stream: transport,
            remote_id,
        })
    }

    /// Get the remote id
    pub fn remote_id(&self) -> PeerId {
        self.remote_id
    }
}

impl<Io> Stream for ECIESStream<Io>
where
    Io: Transport,
{
    type Item = Result<Bytes, io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match ready!(Pin::new(&mut self.get_mut().stream).poll_next(cx)) {
            Some(Ok(IngressECIESValue::Message(body))) => Poll::Ready(Some(Ok(body.into()))),
            Some(other) => Poll::Ready(Some(Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "ECIES stream protocol error: expected message, received {:?}",
                    other
                ),
            )))),
            None => Poll::Ready(None),
        }
    }
}

impl<Io> Sink<Vec<u8>> for ECIESStream<Io>
where
    Io: Transport,
{
    type Error = io::Error;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_ready(cx)
    }

    fn start_send(self: Pin<&mut Self>, item: Vec<u8>) -> Result<(), Self::Error> {
        let this = self.get_mut();
        Pin::new(&mut this.stream).start_send(EgressECIESValue::Message(item))?;

        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.get_mut().stream).poll_close(cx)
    }
}
