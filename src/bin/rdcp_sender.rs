use crc::crc32;
use crc::Hasher32;
use rdtp::rdcp_socket::RDTPSocket;
use std::error::Error;
use tokio::net::UdpSocket;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let mut send_socket = RDTPSocket::bind("127.0.0.1:8001", "127.0.0.1:8002").await?;
    send_socket
        .connect("127.0.0.1:8003", "127.0.0.1:8004")
        .await;
    let data = vec![0u8; (1 << 18) + 128];
    send_socket.send(&data).await;
    Ok(())
}
