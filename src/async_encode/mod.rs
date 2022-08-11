// Translated Encodable and Decodable to use AsyncRead and AsyncWrite from
// https://github.com/rust-bitcoin/rust-bitcoin

mod encode;
mod message;

pub use self::encode::AsyncDecodable;
