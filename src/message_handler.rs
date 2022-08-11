use anyhow::{anyhow, Result};
use bitcoin::consensus::encode::serialize;
use bitcoin::network::constants;
use bitcoin::network::constants::ServiceFlags;
use bitcoin::network::message::{NetworkMessage, RawNetworkMessage};
use bitcoin::network::message_blockdata::Inventory;
use bitcoin::network::message_network::VersionMessage;
use bitcoin::Transaction;
use log::trace;
use tokio::io::{AsyncWrite, AsyncWriteExt};

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
    magic: u32,
    use_wtxid: bool,
    tx: Transaction,
    state: BroadcastState,
}

impl<W: AsyncWrite + Unpin> MessageHandler<W> {
    pub fn new(writer: W, magic: u32, tx: Transaction) -> Self {
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

    pub async fn send_version_msg(&mut self, msg: VersionMessage) -> Result<()> {
        let message = RawNetworkMessage {
            magic: self.magic,
            payload: NetworkMessage::Version(msg),
        };
        let _ = self.writer.write(&serialize(&message)).await?;
        trace!("Sent version message");
        self.state = BroadcastState::AwaitingVersion;
        Ok(())
    }

    pub async fn handle_message(&mut self, msg: NetworkMessage) -> Result<()> {
        match msg {
            NetworkMessage::Version(msg) => {
                if self.state != BroadcastState::AwaitingVersion {
                    return Err(anyhow!("Received version msg out of order"));
                }
                self.handle_version_msg(msg).await?;
                self.state = BroadcastState::AwaitingVerack;
            }
            NetworkMessage::Verack => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(anyhow!("Received verack msg out of order"));
                }
                self.handle_verack_msg().await?;
                self.state = BroadcastState::AwaitingGetData;
            }
            NetworkMessage::WtxidRelay => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(anyhow!("Received wtxidrelay msg out of order"));
                }
                self.handle_wtxid_relay_msg().await?;
            }
            NetworkMessage::SendAddrV2 => {
                if self.state != BroadcastState::AwaitingVerack {
                    return Err(anyhow!("Received sendaddrv2 msg out of order"));
                }
                trace!("Received sendaddrv2 message");
            }
            NetworkMessage::GetData(inv) => {
                if self.state != BroadcastState::AwaitingGetData {
                    return Err(anyhow!("Received getdata msg out of order"));
                }
                self.handle_get_data(inv).await?;
                trace!("Transaction broadcast successfully");
                self.state = BroadcastState::Done;
            }
            NetworkMessage::Ping(nonce) => {
                if self.state != BroadcastState::AwaitingGetData
                    && self.state != BroadcastState::Done
                {
                    return Err(anyhow!("Received ping msg out of order"));
                }
                self.handle_ping_msg(nonce).await?;
            }
            NetworkMessage::Unknown { command, .. } => {
                let command = command.to_string();
                if self.state != BroadcastState::AwaitingGetData
                    && self.state != BroadcastState::Done
                {
                    return Err(anyhow!(
                        "Received {} msg out of order {:#?}",
                        command,
                        self.state
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

    async fn handle_version_msg(&mut self, version_msg: VersionMessage) -> Result<()> {
        trace!("Received version message");

        if !version_msg.relay {
            return Err(anyhow!("Node does not relay transactions"));
        }

        if !version_msg.services.has(ServiceFlags::WITNESS) {
            return Err(anyhow!("Node does not support segwit"));
        }

        if version_msg.version == constants::PROTOCOL_VERSION {
            let msg = RawNetworkMessage {
                magic: self.magic,
                payload: NetworkMessage::WtxidRelay,
            };
            let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
            trace!("Sent wtxid message");
            let msg = RawNetworkMessage {
                magic: self.magic,
                payload: NetworkMessage::SendAddrV2,
            };
            let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
            trace!("Sent sendaddrv2 message");
        }

        let msg = RawNetworkMessage {
            magic: self.magic,
            payload: NetworkMessage::Verack,
        };
        let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent verack message");
        Ok(())
    }

    async fn handle_verack_msg(&mut self) -> Result<()> {
        trace!("Received verack message");
        let inv = if self.use_wtxid {
            Inventory::WTx(self.tx.wtxid())
        } else {
            Inventory::Transaction(self.tx.txid())
        };
        let msg = RawNetworkMessage {
            magic: self.magic,
            payload: NetworkMessage::Inv(vec![inv]),
        };
        let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent inv message");
        Ok(())
    }

    async fn handle_wtxid_relay_msg(&mut self) -> Result<()> {
        trace!("Received wtxid message");
        self.use_wtxid = true;
        Ok(())
    }

    async fn handle_get_data(&mut self, inv: Vec<Inventory>) -> Result<()> {
        trace!("Received getdata message");
        if self.use_wtxid && !inv.contains(&Inventory::WTx(self.tx.wtxid())) {
            return Err(anyhow!("Getdata message does not contain our wtxid"));
        } else if !self.use_wtxid
            && !inv.contains(&Inventory::Transaction(self.tx.txid()))
            && !inv.contains(&Inventory::WitnessTransaction(self.tx.txid()))
        {
            return Err(anyhow!("Getdata message does not contain our txid"));
        }
        let msg = RawNetworkMessage {
            magic: self.magic,
            payload: NetworkMessage::Tx(self.tx.clone()),
        };
        let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent tx message");
        Ok(())
    }

    async fn handle_ping_msg(&mut self, nonce: u64) -> Result<()> {
        trace!("Received ping message");
        let msg = RawNetworkMessage {
            magic: self.magic,
            payload: NetworkMessage::Pong(nonce),
        };
        let _ = self.writer.write_all(serialize(&msg).as_slice()).await?;
        trace!("Sent pong message");
        Ok(())
    }
}
