use std::sync::atomic::AtomicUsize;

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
    inbound_streams: Vec<mpsc::UnboundedSender<NewConnection>>,
    clock: AtomicUsize,
}

impl ConnectionAcceptor {
    pub fn new(
        listener: TcpListener,
        thread_count: usize,
    ) -> (Self, Vec<mpsc::UnboundedReceiver<NewConnection>>) {
        let (inbound_streams, new_streams): (
            Vec<mpsc::UnboundedSender<NewConnection>>,
            Vec<mpsc::UnboundedReceiver<NewConnection>>,
        ) = (0..thread_count)
            .map(|_| mpsc::unbounded_channel())
            .collect();
        (
            Self {
                listener,
                inbound_streams,
                clock: Default::default(),
            },
            new_streams,
        )
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
        let round_robin_connection_thread_assignment = self
            .clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.inbound_streams.len();
        log::debug!("submitting connection to delegated connection server for accept {round_robin_connection_thread_assignment} -> {connection:?}");
        self.inbound_streams[round_robin_connection_thread_assignment]
            .send(connection)
            .map_err(|_e| Error::Dead("connection server"))
    }
}
