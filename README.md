# Bitcoin Send Tx P2P

Library for sending bitcoin transactions to nodes using the bitcoin 
peer-to-peer protocol, given only a transaction and the IP or onion address of
the node.

Under the hood it creates a connection to the node and performs the version
handshake. Then it sends an `inv` message with the txid or wtxid and waits
for a `getdata` message. After transmitting a `tx` message with the full
transaction it disconnects. Note that if the receiving node already has the
transaction it will not respond with a a `getdata` message, in which case
the sending function will timeout and disconnect.

### Examples


```rust
use anyhow::Result;
use bitcoin::Transaction;

use bitcoin_send_tx_p2p::{send_tx_p2p_over_clearnet, send_tx_p2p_over_tor, Config};

async fn send_tx(tx: Transaction) -> Result<()> {
    let mut config = Config::default();
    config.block_height = 1000;
    send_tx_p2p_over_clearnet("127.0.0.1:8333".parse()?, tx, Some(config)).await
}

async fn send_tx_tor(tx: Transaction) -> Result<()> {
    send_tx_p2p_over_tor("cssusbltvosy7hhomxuhicmh5svw6e4z3eebgnyjcnslrloiy5m27pid.onion:8333", tx, None).await
}
```