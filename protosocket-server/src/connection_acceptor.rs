use mio::net::TcpListener;
use tokio::sync::mpsc;

use crate::{would_block, Error};

#[derive(Debug)]
pub(crate) struct NewConnection {
    pub stream: mio::net::TcpStream,
    pub address: std::net::SocketAddr,
}

pub(crate) struct ConnectionAcceptor {
    listener: TcpListener,
    inbound_streams: mpsc::UnboundedSender<NewConnection>,
}

impl ConnectionAcceptor {
    pub fn new(
        listener: TcpListener,
        inbound_streams: mpsc::UnboundedSender<NewConnection>,
    ) -> Self {
        Self {
            listener,
            inbound_streams,
        }
    }

    pub fn accept(&self) -> crate::Result<()> {
        let connection = match self.listener.accept() {
            Ok((stream, address)) => NewConnection { stream, address },
            Err(e) if would_block(&e) => {
                // I guess there isn't anything to accept after all
                return Ok(());
            }
            Err(e) => {
                // If it was any other kind of error, something went
                // wrong and we terminate with an error.
                return Err(e.into());
            }
        };
        if let Err(e) = connection.stream.set_nodelay(true) {
            log::warn!("could not set nodelay: {e:?}");
        }
        log::debug!("accepted connection {connection:?}");
        self.inbound_streams
            .send(connection)
            .map_err(|_e| Error::Dead("connection server"))
    }
}
