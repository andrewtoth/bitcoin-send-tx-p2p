# Bitcoin Send Tx P2P

Library for sending bitcoin transactions to nodes using the bitcoin 
peer-to-peer protocol, given only a transaction and the IP or onion address of
the node.

### Examples

```
use anyhow::Result;
use bitcoin::consensus::encode::deserialize;
use bitcoin::{Network, Transaction};
use bitcoin_send_tx_p2p::{send_tx_p2p_over_clearnet, send_tx_p2p_over_tor, Config};
use hex::FromHex;

#[tokio::main]
async fn main() -> Result<()> {
    let tx_bytes = Vec::from_hex("000000800100000000000000000000000000000000000000000000000000000000000000000000000000ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000")?;
    let tx: Transaction = deserialize(&tx_bytes)?;

    send_tx_p2p_over_clearnet("127.0.0.1:8333".parse()?, tx.clone(), None).await?;

    let mut config = Config::default();
    config.block_height = 138;
    config.network = Network::Regtest;

    send_tx_p2p_over_tor("<onion address>:8333", tx, Some(config)).await
}
```