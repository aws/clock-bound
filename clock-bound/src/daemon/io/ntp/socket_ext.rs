//! Socket command extensions

pub trait SocketExt {
    /// Clear the buffer of the socket
    ///
    /// # Errors
    /// Returns an error if the underlying socket returns any error
    /// other than `WouldBlock`
    fn clear(&self) -> Result<(), std::io::Error>;
}

impl SocketExt for tokio::net::UdpSocket {
    fn clear(&self) -> Result<(), std::io::Error> {
        // size doesn't matter if not actually reading
        // udp packets will be truncated and excess dropped
        let mut buf = [0u8; 48];
        loop {
            match self.try_recv(&mut buf) {
                Ok(_) => (),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn clear() {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let tx_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let buffer = [0xFFu8; 48];
        tx_socket
            .send_to(&buffer, format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        socket.clear().unwrap();
        let mut buf = [0u8; 48];
        let err = socket.try_recv(&mut buf).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock);
    }
}
