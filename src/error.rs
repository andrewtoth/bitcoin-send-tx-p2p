use std::borrow::Cow;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("protocol error: {0}")]
    Protocol(Cow<'static, str>),
    #[error("bitcoin decoding error")]
    Bitcoin(#[from] bitcoin::consensus::encode::Error),
    #[error("system time error")]
    SystemTime(#[from] std::time::SystemTimeError),
    #[error("timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[cfg(feature = "tor")]
    #[error("socks5 error")]
    Socks5Error(#[from] tokio_socks::Error),
}
