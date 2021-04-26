use rdtp::rdcp_socket;
use rdtp::rdcp_socket::RDTPSocket;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};
use tokio::time::Instant;



#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let mut send_socket = RDTPSocket::bind("127.0.0.1:8003", "127.0.0.1:8004").await?;
    send_socket
        .connect("127.0.0.1:8001", "127.0.0.1:8002")
        .await;
    let mut buf = vec![0u8; 1 << 19];
    send_socket.recv(&mut buf).await;
    Ok(())
}
