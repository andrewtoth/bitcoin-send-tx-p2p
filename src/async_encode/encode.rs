use core::mem;

use async_trait::async_trait;
use bitcoin::{
    consensus::encode::{CheckedData, Error, MAX_VEC_SIZE},
    hashes::{sha256d, Hash as _},
    p2p::{message_blockdata::Inventory, Magic},
    BlockHash, Txid, VarInt, Wtxid,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[async_trait]
pub trait AsyncEncodable: Sized {
    async fn consensus_encode<W: AsyncWrite + Send + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<usize, std::io::Error>;
}

#[async_trait]
pub trait AsyncDecodable: Sized {
    #[inline]
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        reader: &mut R,
    ) -> Result<Self, Error> {
        Self::async_consensus_decode(reader).await
    }

    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        reader: &mut R,
    ) -> Result<Self, Error> {
        Self::async_consensus_decode_from_finite_reader(reader.take(MAX_VEC_SIZE as u64).get_mut())
            .await
    }
}

// Primitive types
macro_rules! impl_int_encodable {
    ($ty:ident, $meth_dec:ident, $meth_enc:ident) => {
        #[async_trait]
        impl AsyncDecodable for $ty {
            #[inline]
            async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
                r: &mut R,
            ) -> Result<Self, Error> {
                AsyncReadExt::$meth_dec(r)
                    .await
                    .map_err(|err| bitcoin::io::Error::from(err).into())
            }
        }
        #[async_trait]
        impl AsyncEncodable for $ty {
            #[inline]
            async fn consensus_encode<W: AsyncWrite + Send + Unpin>(
                &self,
                w: &mut W,
            ) -> Result<usize, std::io::Error> {
                w.$meth_enc(*self).await?;
                Ok(mem::size_of::<$ty>())
            }
        }
    };
}

impl_int_encodable!(u8, read_u8, write_u8);
impl_int_encodable!(u16, read_u16_le, write_u16_le);
impl_int_encodable!(u32, read_u32_le, write_u32_le);
impl_int_encodable!(u64, read_u64_le, write_u64_le);
impl_int_encodable!(i8, read_i8, write_i8);
impl_int_encodable!(i16, read_i16_le, write_i16_le);
impl_int_encodable!(i32, read_i32_le, write_i32_le);
impl_int_encodable!(i64, read_i64_le, write_i64_le);

#[async_trait]
impl AsyncDecodable for VarInt {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let n = AsyncReadExt::read_u8(r)
            .await
            .map_err(bitcoin::io::Error::from)?;
        match n {
            0xFF => {
                let x = AsyncReadExt::read_u64(r)
                    .await
                    .map_err(bitcoin::io::Error::from)?;
                if x < 0x100000000 {
                    Err(self::Error::NonMinimalVarInt)
                } else {
                    Ok(VarInt(x))
                }
            }
            0xFE => {
                let x = AsyncReadExt::read_u32(r)
                    .await
                    .map_err(bitcoin::io::Error::from)?;
                if x < 0x10000 {
                    Err(self::Error::NonMinimalVarInt)
                } else {
                    Ok(VarInt(x as u64))
                }
            }
            0xFD => {
                let x = AsyncReadExt::read_u16(r)
                    .await
                    .map_err(bitcoin::io::Error::from)?;
                if x < 0xFD {
                    Err(self::Error::NonMinimalVarInt)
                } else {
                    Ok(VarInt(x as u64))
                }
            }
            n => Ok(VarInt(n as u64)),
        }
    }
}

#[async_trait]
impl AsyncEncodable for VarInt {
    #[inline]
    async fn consensus_encode<W: AsyncWrite + Send + Unpin>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        match self.0 {
            0..=0xFC => {
                (self.0 as u8).consensus_encode(w).await?;
                Ok(1)
            }
            0xFD..=0xFFFF => {
                w.write_u8(0xFD).await?;
                (self.0 as u16).consensus_encode(w).await?;
                Ok(3)
            }
            0x10000..=0xFFFFFFFF => {
                w.write_u8(0xFE).await?;
                (self.0 as u32).consensus_encode(w).await?;
                Ok(5)
            }
            _ => {
                w.write_u8(0xFF).await?;
                self.0.consensus_encode(w).await?;
                Ok(9)
            }
        }
    }
}

#[async_trait]
impl AsyncDecodable for bool {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        AsyncReadExt::read_i8(r)
            .await
            .map_err(|err| bitcoin::io::Error::from(err).into())
            .map(|bit| bit != 0)
    }
}

#[async_trait]
impl AsyncDecodable for String {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        String::from_utf8(AsyncDecodable::async_consensus_decode(r).await?)
            .map_err(|_| self::Error::ParseFailed("String was not valid UTF8"))
    }
}

macro_rules! impl_array {
    ( $size:literal ) => {
        #[async_trait]
        impl AsyncEncodable for [u8; $size] {
            #[inline]
            async fn consensus_encode<W: AsyncWrite + ?Sized + Send + Unpin>(
                &self,
                w: &mut W,
            ) -> Result<usize, std::io::Error> {
                w.write_all(&self[..]).await?;
                Ok(self.len())
            }
        }

        #[async_trait]
        impl AsyncDecodable for [u8; $size] {
            #[inline]
            async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
                r: &mut R,
            ) -> Result<Self, Error> {
                let mut ret = [0; $size];
                r.read_exact(&mut ret)
                    .await
                    .map_err(bitcoin::io::Error::from)?;
                Ok(ret)
            }
        }
    };
}

impl_array!(2);
impl_array!(4);
impl_array!(6);
impl_array!(8);
impl_array!(10);
impl_array!(12);
impl_array!(16);
impl_array!(32);
impl_array!(33);

#[async_trait]
impl AsyncDecodable for [u16; 8] {
    #[inline]
    async fn async_consensus_decode<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let mut res = [0; 8];
        for item in &mut res {
            *item = AsyncDecodable::async_consensus_decode(r).await?;
        }
        Ok(res)
    }
}

macro_rules! impl_vec {
    ($type: ty) => {
        #[async_trait]
        impl AsyncDecodable for Vec<$type> {
            #[inline]
            async fn async_consensus_decode_from_finite_reader<
                R: AsyncRead + Sized + Send + Unpin,
            >(
                r: &mut R,
            ) -> Result<Self, Error> {
                let len = VarInt::async_consensus_decode_from_finite_reader(r)
                    .await?
                    .0;
                // Do not allocate upfront more items than if the sequnce of type
                // occupied roughly quarter a block. This should never be the case
                // for normal data, but even if that's not true - `push` will just
                // reallocate.
                // Note: OOM protection relies on reader eventually running out of
                // data to feed us.
                let max_capacity = MAX_VEC_SIZE / 4 / mem::size_of::<$type>();
                let mut ret = Vec::with_capacity(core::cmp::min(len as usize, max_capacity));
                for _ in 0..len {
                    ret.push(AsyncDecodable::async_consensus_decode_from_finite_reader(r).await?);
                }
                Ok(ret)
            }
        }
    };
}

impl_vec!(Vec<u8>);
impl_vec!(u64);
impl_vec!(VarInt);
impl_vec!(Inventory);
impl_vec!(u32);
impl_vec!(String);

#[async_trait]
impl AsyncDecodable for Inventory {
    #[inline]
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let inv_type: u32 = AsyncDecodable::async_consensus_decode(r).await?;
        Ok(match inv_type {
            0 => Inventory::Error,
            1 => Inventory::Transaction(AsyncDecodable::async_consensus_decode(r).await?),
            2 => Inventory::Block(AsyncDecodable::async_consensus_decode(r).await?),
            5 => Inventory::WTx(AsyncDecodable::async_consensus_decode(r).await?),
            0x40000001 => {
                Inventory::WitnessTransaction(AsyncDecodable::async_consensus_decode(r).await?)
            }
            0x40000002 => Inventory::WitnessBlock(AsyncDecodable::async_consensus_decode(r).await?),
            tp => Inventory::Unknown {
                inv_type: tp,
                hash: AsyncDecodable::async_consensus_decode(r).await?,
            },
        })
    }
}

struct ReadBytesFromFiniteReaderOpts {
    len: usize,
    chunk_size: usize,
}

#[inline]
async fn read_bytes_from_finite_reader<D: AsyncRead + Sized + Send + Unpin>(
    mut d: D,
    mut opts: ReadBytesFromFiniteReaderOpts,
) -> Result<Vec<u8>, Error> {
    let mut ret = vec![];

    assert_ne!(opts.chunk_size, 0);

    while opts.len > 0 {
        let chunk_start = ret.len();
        let chunk_size = core::cmp::min(opts.len, opts.chunk_size);
        let chunk_end = chunk_start + chunk_size;
        ret.resize(chunk_end, 0u8);
        d.read_exact(&mut ret[chunk_start..chunk_end])
            .await
            .map_err(bitcoin::io::Error::from)?;
        opts.len -= chunk_size;
    }

    Ok(ret)
}

#[async_trait]
impl AsyncDecodable for Vec<u8> {
    #[inline]
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let len = VarInt::async_consensus_decode(r).await?.0 as usize;
        // most real-world vec of bytes data, wouldn't be larger than 128KiB
        read_bytes_from_finite_reader(
            r,
            ReadBytesFromFiniteReaderOpts {
                len,
                chunk_size: 128 * 1024,
            },
        )
        .await
    }
}

fn sha2_checksum(data: &[u8]) -> [u8; 4] {
    let checksum = sha256d::Hash::hash(data);
    [checksum[0], checksum[1], checksum[2], checksum[3]]
}

#[async_trait]
impl AsyncDecodable for CheckedData {
    #[inline]
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        let len = u32::async_consensus_decode_from_finite_reader(r).await? as usize;

        let checksum = <[u8; 4]>::async_consensus_decode_from_finite_reader(r).await?;
        let ret = read_bytes_from_finite_reader(
            r,
            ReadBytesFromFiniteReaderOpts {
                len,
                chunk_size: MAX_VEC_SIZE,
            },
        )
        .await?;
        let expected_checksum = sha2_checksum(&ret);
        if expected_checksum != checksum {
            Err(self::Error::InvalidChecksum {
                expected: expected_checksum,
                actual: checksum,
            })
        } else {
            Ok(CheckedData::new(ret))
        }
    }
}

macro_rules! impl_sha256d {
    ($type: ty) => {
        #[async_trait]
        impl AsyncDecodable for $type {
            #[inline]
            async fn async_consensus_decode_from_finite_reader<
                R: AsyncRead + Sized + Send + Unpin,
            >(
                r: &mut R,
            ) -> Result<Self, Error> {
                Ok(Self::from_byte_array(
                    <[u8; 32]>::async_consensus_decode(r).await?,
                ))
            }
        }
    };
}

impl_sha256d!(Txid);
impl_sha256d!(BlockHash);
impl_sha256d!(Wtxid);

#[async_trait]
impl AsyncDecodable for Magic {
    #[inline]
    async fn async_consensus_decode_from_finite_reader<R: AsyncRead + Sized + Send + Unpin>(
        r: &mut R,
    ) -> Result<Self, Error> {
        Ok(Self::from_bytes(
            <[u8; 4]>::async_consensus_decode_from_finite_reader(r).await?,
        ))
    }
}
