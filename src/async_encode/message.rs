use crate::async_encode::encode::AsyncDecodable;
use async_trait::async_trait;
use bitcoin::consensus::encode::{CheckedData, Error};
use bitcoin::network::constants::ServiceFlags;
use bitcoin::network::message::{CommandString, NetworkMessage, RawNetworkMessage};
use bitcoin::network::message_network::VersionMessage;
use bitcoin::network::Address;
use core::iter::FromIterator;
use std::io::Cursor;
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_MSG_SIZE: usize = 5_000_000;

#[async_trait]
impl AsyncDecodable for RawNetworkMessage {
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let magic = AsyncDecodable::async_consensus_decode_from_finite_reader(r).await?;
        let cmd = CommandString::async_consensus_decode_from_finite_reader(r).await?;
        let raw_payload = CheckedData::async_consensus_decode_from_finite_reader(r)
            .await?
            .0;
        let mut mem_d = Cursor::new(raw_payload);
        let payload = match cmd.as_ref() {
            "version" => NetworkMessage::Version(
                AsyncDecodable::async_consensus_decode_from_finite_reader(&mut mem_d).await?,
            ),
            "verack" => NetworkMessage::Verack,
            "getdata" => NetworkMessage::GetData(
                AsyncDecodable::async_consensus_decode_from_finite_reader(&mut mem_d).await?,
            ),
            "ping" => NetworkMessage::Ping(
                AsyncDecodable::async_consensus_decode_from_finite_reader(&mut mem_d).await?,
            ),
            "alert" => NetworkMessage::Alert(
                AsyncDecodable::async_consensus_decode_from_finite_reader(&mut mem_d).await?,
            ),
            "wtxidrelay" => NetworkMessage::WtxidRelay,
            "sendaddrv2" => NetworkMessage::SendAddrV2,
            _ => NetworkMessage::Unknown {
                command: cmd,
                payload: mem_d.into_inner(),
            },
        };
        Ok(RawNetworkMessage { magic, payload })
    }

    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        Self::async_consensus_decode_from_finite_reader(r.take(MAX_MSG_SIZE as u64).get_mut()).await
    }
}

#[async_trait]
impl AsyncDecodable for CommandString {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let rawbytes: [u8; 12] = AsyncDecodable::async_consensus_decode(r).await?;
        let rv: String = FromIterator::from_iter(rawbytes.iter().filter_map(|&u| {
            if u > 0 {
                Some(u as char)
            } else {
                None
            }
        }));
        Ok(CommandString::try_from(rv)
            .map_err(|_| Error::ParseFailed("Failed to parse CommandString"))?)
    }
}

macro_rules! impl_consensus_encoding {
    ($thing:ident, $($field:ident),+) => (

        #[async_trait]
        impl AsyncDecodable for $thing {

            #[inline]
            async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
                r: &mut R,
            ) -> Result<$thing, Error> {
                Ok($thing {
                    $($field: AsyncDecodable::async_consensus_decode_from_finite_reader(r).await?),+
                })
            }

            #[inline]
            async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
                r: &mut R,
            ) -> Result<$thing, Error> {
                use tokio::io::AsyncReadExt as _;
                let mut r = r.take(bitcoin::consensus::encode::MAX_VEC_SIZE as u64);
                Ok($thing {
                    $($field: AsyncDecodable::async_consensus_decode(r.get_mut()).await?),+
                })
            }
        }
    );
}

pub(crate) use impl_consensus_encoding;

impl_consensus_encoding!(
    VersionMessage,
    version,
    services,
    timestamp,
    receiver,
    sender,
    nonce,
    user_agent,
    start_height,
    relay
);

#[async_trait]
impl AsyncDecodable for ServiceFlags {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        Ok(ServiceFlags::from(u64::async_consensus_decode(r).await?))
    }
}

async fn read_be_address<R: AsyncRead + Sized + Unpin>(r: &mut R) -> Result<[u16; 8], Error> {
    let mut address = [0u16; 8];
    let mut buf = [0u8; 2];

    for word in &mut address {
        r.read_exact(&mut buf).await?;
        *word = u16::from_be_bytes(buf)
    }
    Ok(address)
}

#[async_trait]
impl AsyncDecodable for Address {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        Ok(Address {
            services: AsyncDecodable::async_consensus_decode(r).await?,
            address: read_be_address(r).await?,
            port: u16::swap_bytes(AsyncDecodable::async_consensus_decode(r).await?),
        })
    }
}
