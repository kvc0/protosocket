use mio::{net::TcpListener, Events, Interest, Poll, Token};

use crate::{connection_server::{ConnectionAcceptor, ConnectionServer}, interrupted, Result};

pub struct Server {
    poll: Poll,
    events: Events,
    services: Vec<ConnectionAcceptor>,
}

impl Server {
    pub fn new() -> Result<Self> {
        let poll = Poll::new()?;
        let events = Events::with_capacity(1024);

        Ok(
            Self {
                poll,
                events,
                services: Default::default(),
            }
        )
    }

    /// address like "127.0.0.1:9000"
    pub fn register_service_listener(&mut self, address: impl Into<String>) -> Result<ConnectionServer> {
        let addr = address.into().parse()?;
        let mut listener = TcpListener::bind(addr)?;

        let token = Token(self.services.len());

        self.poll.registry()
            .register(&mut listener, token, Interest::READABLE)?;
        let (acceptor, server) = ConnectionServer::new(listener);

        self.services.push(acceptor);

        Ok(server)
    }

    /// Dedicate a thread to your connection frontend.
    pub fn serve(self) -> Result<()> {
        let Self { mut poll, mut events, services } = self;
        loop {
            if let Err(e) = poll.poll(&mut events, None) {
                if interrupted(&e) {
                    continue;
                }
                return Err(e.into())
            }

            for event in events.iter() {
                let service_index = event.token().0;
                services[service_index].accept()?;
            }
        }
    }
}
