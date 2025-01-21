use bitcoin::{
    consensus::encode::serialize,
    p2p::{
        message::{NetworkMessage, RawNetworkMessage},
        message_blockdata::Inventory,
        message_network::VersionMessage,
        Magic, ServiceFlags, PROTOCOL_VERSION,
    },
    Transaction,
};
use log::trace;
use tokio::io::{self, AsyncWrite, AsyncWriteExt};

use crate::Error;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum BroadcastState {
    Connected,
    AwaitingVersion,
    AwaitingVerack,
    AwaitingGetData,
    Done,
}

pub struct MessageHandler<W: AsyncWrite + Unpin> {
    writer: W,
    magic: Magic,
    use_wtxid: bool,
    tx: Transaction,
    state: BroadcastState,
}

impl<W: AsyncWrite + Unpin> MessageHandler<W> {
    pub fn new(writer: W, magic: Magic, tx: Transaction) -> Self {
        MessageHandler {
            writer,
            magic,
            use_wtxid: false,
            tx,
            state: BroadcastState::Connected,
        }
    }

    pub fn state(&self) -> BroadcastState {
        self.state
    }

    pub async fn send_version_msg(&mut self, msg: VersionMessage) -> io::Result<()> {
        let message = RawNetworkMessage::new(self.magic, NetworkMessage::Version(msg));
        let _ = self.writer.write(&serialize(&message)).await?;
        trace!("Sent version message");
        self.state = BroadcastState::AwaitingVersion;
        Ok(())
    }

    pub async fn handle_message(&mut self, msg: NetworkMessage) -> Result<(), Error> {
        match msg {
            NetworkMessage::Version(msg) => {
                if self.state != BroadcastState::AwaitingVersion {
                    return Err(Error::Protocol("Received version msg out of order".into()));
                }
                self.handle_version_msg(msg).await?;
                self.state = BroadcastState::AwaitingVerack;
            }
            NetworkMessage::Verack => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(Error::Protocol("Received verack msg out of order".into()));
                }
                self.handle_verack_msg().await?;
                self.state = BroadcastState::AwaitingGetData;
            }
            NetworkMessage::WtxidRelay => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(Error::Protocol(
                        "Received wtxidrelay msg out of order".into(),
                    ));
                }
                self.handle_wtxid_relay_msg().await?;
            }
            NetworkMessage::SendAddrV2 => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(Error::Protocol(
                        "Received sendaddrv2 msg out of order".into(),
                    ));
                }
                trace!("Received sendaddrv2 message");
            }
            NetworkMessage::GetData(inv) => {
                if self.state != BroadcastState::AwaitingGetData {
                    return Err(Error::Protocol("Received getdata msg out of order".into()));
                }
                self.handle_get_data(inv).await?;
                trace!("Transaction broadcast successfully");
                self.state = BroadcastState::Done;
            }
            NetworkMessage::Ping(nonce) => {
                if self.state != BroadcastState::AwaitingGetData
                    && self.state != BroadcastState::Done
                {
                    return Err(Error::Protocol("Received ping msg out of order".into()));
                }
                self.handle_ping_msg(nonce).await?;
            }
            NetworkMessage::Unknown { command, .. } => {
                let command = command.to_string();
                if self.state != BroadcastState::AwaitingGetData
                    && self.state != BroadcastState::Done
                {
                    return Err(Error::Protocol(
                        format!("Received {} msg out of order {:#?}", command, self.state,).into(),
                    ));
                }
                trace!("Received message: {}", command);
            }
            _ => {
                trace!("Received unknown message: {:?}", msg);
            }
        }
        Ok(())
    }

    async fn handle_version_msg(&mut self, version_msg: VersionMessage) -> Result<(), Error> {
        trace!("Received version message");

        if !version_msg.relay {
            return Err(Error::Protocol("Node does not relay transactions".into()));
        }

        if !version_msg.services.has(ServiceFlags::WITNESS) {
            return Err(Error::Protocol("Node does not support segwit".into()));
        }

        if version_msg.version == PROTOCOL_VERSION {
            let msg = RawNetworkMessage::new(self.magic, NetworkMessage::WtxidRelay);
            self.writer.write_all(serialize(&msg).as_slice()).await?;
            trace!("Sent wtxid message");
            let msg = RawNetworkMessage::new(self.magic, NetworkMessage::SendAddrV2);
            self.writer.write_all(serialize(&msg).as_slice()).await?;
            trace!("Sent sendaddrv2 message");
        }

        let msg = RawNetworkMessage::new(self.magic, NetworkMessage::Verack);
        self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent verack message");
        Ok(())
    }

    async fn handle_verack_msg(&mut self) -> io::Result<()> {
        trace!("Received verack message");
        let inv = if self.use_wtxid {
            Inventory::WTx(self.tx.compute_wtxid())
        } else {
            Inventory::Transaction(self.tx.compute_txid())
        };
        let msg = RawNetworkMessage::new(self.magic, NetworkMessage::Inv(vec![inv]));
        self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent inv message");
        Ok(())
    }

    async fn handle_wtxid_relay_msg(&mut self) -> io::Result<()> {
        trace!("Received wtxid message");
        self.use_wtxid = true;
        Ok(())
    }

    async fn handle_get_data(&mut self, inv: Vec<Inventory>) -> Result<(), Error> {
        trace!("Received getdata message");
        if self.use_wtxid && !inv.contains(&Inventory::WTx(self.tx.compute_wtxid())) {
            return Err(Error::Protocol(
                "Getdata message does not contain our wtxid".into(),
            ));
        } else if !self.use_wtxid {
            let txid = self.tx.compute_txid();
            if !inv.contains(&Inventory::Transaction(txid))
                && !inv.contains(&Inventory::WitnessTransaction(txid))
            {
                return Err(Error::Protocol(
                    "Getdata message does not contain our txid".into(),
                ));
            }
        }
        let msg = RawNetworkMessage::new(self.magic, NetworkMessage::Tx(self.tx.clone()));
        self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent tx message");
        Ok(())
    }

    async fn handle_ping_msg(&mut self, nonce: u64) -> io::Result<()> {
        trace!("Received ping message");
        let msg = RawNetworkMessage::new(self.magic, NetworkMessage::Pong(nonce));
        self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent pong message");
        Ok(())
    }
}
