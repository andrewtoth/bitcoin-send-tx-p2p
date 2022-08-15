//! Send a Bitcoin Transaction to a node via Peer-to-Peer protocol
//!
//! Supports sending via clearnet with a [SocketAddr] or via tor using a
//! [SocketAddr] or onion address with the [IntoTargetAddr] trait.
//!
//! Under the hood it creates a connection to the node and performs the version
//! handshake. Then it sends an `inv` message with the txid or wtxid and waits
//! for a `getdata` message. After transmitting a `tx` message with the full
//! transaction it disconnects. Note that if the receiving node already has the
//! transaction it will not respond with a a `getdata` message, in which case
//! the sending function will timeout and disconnect.

#![cfg_attr(docsrs, feature(doc_cfg))]

mod async_encode;
mod message_handler;

use anyhow::Result;
use async_encode::AsyncDecodable;
use bitcoin::network::address::Address;
use bitcoin::network::constants::ServiceFlags;
use bitcoin::network::message::RawNetworkMessage;
use bitcoin::network::message_network::VersionMessage;
use bitcoin::secp256k1;
use bitcoin::secp256k1::rand::Rng;
use bitcoin::{Network, Transaction};
use log::{info, trace};
use message_handler::{BroadcastState, MessageHandler};
#[cfg(feature = "tor")]
use std::fmt::Debug;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;
#[cfg(feature = "tor")]
pub use tokio_socks::IntoTargetAddr;
#[cfg(feature = "tor")]
use tokio_socks::{tcp::Socks5Stream, TargetAddr};

/// Config options for sending
pub struct Config {
    /// The user agent for the initial version message
    /// Defaults to /Satoshi:23.0.0/
    pub user_agent: String,
    /// The block height for the initial version message
    /// Defaults to 749_000
    pub block_height: i32,
    /// The network to use
    /// Defaults to Network::Bitcoin
    pub network: Network,
    /// The timeout duration for the initial connection to the node
    /// Default is 30 seconds but that might not be long enough for tor
    pub connection_timeout: Duration,
    /// The timeout duration for the handshake, sending inv, receiving getdata,
    /// and finally sending the tx message
    /// Note that if a node already has the tx then it will not respond with
    /// getdata so a timeout here does not necessarily mean the node does not
    /// have the tx
    /// Default is 30 seconds
    pub send_tx_timeout: Duration,
    /// Tor SOCKS5 proxy address to send through if using tor
    /// Defaults to 127.0.0.1:9050
    #[cfg(feature = "tor")]
    #[cfg_attr(docsrs, doc(cfg(feature = "tor")))]
    pub tor_proxy: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            user_agent: String::from("/Satoshi:23.0.0/"),
            block_height: 749_000,
            network: Network::Bitcoin,
            connection_timeout: Duration::from_secs(30),
            send_tx_timeout: Duration::from_secs(30),
            #[cfg(feature = "tor")]
            tor_proxy: "127.0.0.1:9050".parse().unwrap(),
        }
    }
}

/// Connects to a node at `address` over clearnet and attempts to send it `tx`,
/// optionally taking [`config`](Config) to specify configuration options
///
/// # Example
/// ```rust
///use anyhow::Result;
///use bitcoin::Transaction;
///use bitcoin_send_tx_p2p::{send_tx_p2p_over_clearnet, Config};
///
///async fn send_tx(tx: Transaction) -> Result<()> {
///    let mut config = Config::default();
///    config.block_height = 1000;
///    send_tx_p2p_over_clearnet("127.0.0.1:8333".parse()?, tx, Some(config)).await
///}
/// ```
pub async fn send_tx_p2p_over_clearnet(
    address: SocketAddr,
    tx: Transaction,
    config: Option<Config>,
) -> Result<()> {
    let config = config.unwrap_or_default();
    let mut stream = timeout(config.connection_timeout, TcpStream::connect(address)).await??;

    info!("Connected to node at {:?}", address);

    let version_message = build_version_message(&config, Some(address))?;
    send_tx_p2p(
        &mut stream,
        tx,
        config.network,
        config.send_tx_timeout,
        version_message,
    )
    .await
}

/// Connects to a node at `address` over tor and attempts to send it `tx`,
/// optionally taking [`config`](Config) to specify configuration options
///
/// # Example
/// ```rust
///use anyhow::Result;
///use bitcoin::Transaction;
///use bitcoin_send_tx_p2p::send_tx_p2p_over_tor;
///
///async fn send_tx(tx: Transaction) -> Result<()> {
///    send_tx_p2p_over_tor("cssusbltvosy7hhomxuhicmh5svw6e4z3eebgnyjcnslrloiy5m27pid.onion:8333", tx, None).await
///}
/// ```
#[cfg(feature = "tor")]
#[cfg_attr(docsrs, doc(cfg(feature = "tor")))]
pub async fn send_tx_p2p_over_tor<'t, T>(
    address: T,
    tx: Transaction,
    config: Option<Config>,
) -> Result<()>
where
    T: IntoTargetAddr<'t> + Clone + Debug,
{
    let config = config.unwrap_or_default();
    let mut stream = timeout(
        config.connection_timeout,
        Socks5Stream::connect(config.tor_proxy, address.clone()),
    )
    .await??;

    info!("Connected to node at {:?}", address);

    let address = match address.into_target_addr()? {
        TargetAddr::Ip(address) => Some(address),
        _ => None,
    };
    let version_message = build_version_message(&config, address)?;
    send_tx_p2p(
        &mut stream,
        tx,
        config.network,
        config.send_tx_timeout,
        version_message,
    )
    .await
}

async fn send_tx_p2p(
    stream: &mut TcpStream,
    tx: Transaction,
    network: Network,
    send_tx_timeout: Duration,
    version_message: VersionMessage,
) -> Result<()> {
    let (read_stream, write_stream) = stream.split();

    let mut message_handler = MessageHandler::new(write_stream, network.magic(), tx);
    message_handler.send_version_msg(version_message).await?;

    let result = timeout(
        send_tx_timeout,
        message_loop(read_stream, &mut message_handler),
    )
    .await;

    if let Ok(_) = result {
        info!("Sent tx successfully");
    }

    trace!("Disconnecting");
    // Ignore error on shutdown, since we might have already broadcasted successfully
    let _ = stream.shutdown().await;

    result?
}

async fn message_loop<W: AsyncWrite + Unpin, R: AsyncRead + Unpin + Send>(
    read_stream: R,
    message_handler: &mut MessageHandler<W>,
) -> Result<()> {
    let mut reader = BufReader::new(read_stream);
    loop {
        let reply = RawNetworkMessage::async_consensus_decode(&mut reader).await?;
        message_handler.handle_message(reply.payload).await?;
        if message_handler.state() == BroadcastState::Done {
            break;
        }
    }
    Ok(())
}

fn build_version_message(config: &Config, address: Option<SocketAddr>) -> Result<VersionMessage> {
    let empty_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);

    let services = ServiceFlags::WITNESS;
    let addr_recv = match address {
        Some(addr) => Address::new(&addr, services),
        None => Address::new(&empty_address, services),
    };
    let addr_from = Address::new(&empty_address, services);
    let nonce: u64 = secp256k1::rand::thread_rng().gen();
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

    let msg = VersionMessage::new(
        services,
        timestamp.try_into().unwrap(),
        addr_recv,
        addr_from,
        nonce,
        config.user_agent.clone(),
        config.block_height,
    );
    Ok(msg)
}

#[cfg(test)]
mod tests {

    use super::send_tx_p2p_over_clearnet;
    #[cfg(feature = "tor")]
    use super::send_tx_p2p_over_tor;
    use anyhow::Result;
    use bitcoin::consensus::encode::deserialize;
    use hex::FromHex;

    use env_logger;

    #[tokio::test]
    #[cfg(feature = "tor")]
    async fn test_tor() -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let tx_bytes = Vec::from_hex("000000800100000000000000000000000000000000000000000000000000000000000000000000000000ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000")?;
        let tx = deserialize(&tx_bytes)?;
        let result = send_tx_p2p_over_tor(
            "<find ip or onion address to add here bitnodes.io>:8333",
            tx,
            None,
        )
        .await;
        tokio_test::assert_ok!(result, "Send over tor failed");
        Ok(())
    }

    #[tokio::test]
    async fn test_clearnet() -> Result<()> {
        let _ = env_logger::builder().is_test(true).try_init();

        let tx_bytes = Vec::from_hex("000000800100000000000000000000000000000000000000000000000000000000000000000000000000ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000")?;
        let tx = deserialize(&tx_bytes)?;
        let result = send_tx_p2p_over_clearnet(
            "<find ip address to add here bitnodes.io>:8333".parse()?,
            tx,
            None,
        )
        .await;
        tokio_test::assert_ok!(result, "Send over clearnet failed");
        Ok(())
    }
}
