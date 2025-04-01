use std::io;

use async_trait::async_trait;
use futures::{AsyncReadExt, AsyncWriteExt};
use libp2p::{
    futures::{AsyncRead, AsyncWrite},
    identity::Keypair,
    request_response, StreamProtocol,
};
use tracing::trace;

use crate::handshake::{envelope, envelope::Envelope, node_info, node_info::NodeInfo};

const MAXIMUM_SIZE: u64 = 1024;

impl From<envelope::Error> for io::Error {
    fn from(err: envelope::Error) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, err)
    }
}

impl From<node_info::Error> for io::Error {
    fn from(err: node_info::Error) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, err)
    }
}

/// A `Codec` that reads/writes an **`Envelope`**.
#[derive(Clone, Debug)]
pub struct Codec {
    keypair: Keypair,
}

impl Codec {
    pub fn new(keypair: Keypair) -> Self {
        Self { keypair }
    }

    async fn read<T>(&mut self, io: &mut T) -> io::Result<NodeInfo>
    where
        T: AsyncRead + Unpin + Send,
    {
        let mut msg_buf = Vec::new();
        let num_bytes_read = io.take(MAXIMUM_SIZE).read_to_end(&mut msg_buf).await?;
        trace!(?num_bytes_read, "read handshake");
        let env = Envelope::parse_and_verify(&msg_buf)?;
        let node_info = NodeInfo::unmarshal(&env.payload)?;
        Ok(node_info)
    }

    async fn write<T>(&mut self, io: &mut T, node_info: NodeInfo) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let envelope = node_info.seal(&self.keypair)?;
        let raw = envelope.encode_to_vec()?;
        io.write_all(&raw).await?;
        io.flush().await?;
        io.close().await?;
        Ok(())
    }
}

#[async_trait]
impl request_response::Codec for Codec {
    type Protocol = StreamProtocol;
    type Request = NodeInfo;
    type Response = NodeInfo;

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        trace!("reading handshake request");
        self.read(io).await
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        trace!("reading handshake response");
        self.read(io).await
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        trace!(req = ?req, "writing handshake request");
        self.write(io, req).await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        trace!("writing handshake response");
        self.write(io, res).await
    }
}
