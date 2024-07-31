use mio::{net::TcpListener, Events, Interest, Poll, Token};

use crate::{
    connection_acceptor::ConnectionAcceptor, connection_server::ConnectionServer, interrupted,
    Result, ServerConnector,
};

pub struct Server {
    poll: Poll,
    events: Events,
    services: Vec<ConnectionAcceptor>,
}

impl Server {
    pub fn new() -> Result<Self> {
        let poll = Poll::new()?;
        let events = Events::with_capacity(1024);
        log::trace!("new server");

        Ok(Self {
            poll,
            events,
            services: Default::default(),
        })
    }

    /// listener must be configured non_blocking
    pub fn register_service_listener<Connector: ServerConnector>(
        &mut self,
        listener: std::net::TcpListener,
        server_state: Connector,
    ) -> Result<ConnectionServer<Connector>> {
        let token = Token(self.services.len());
        log::trace!("new service listener index {} on {listener:?}", token.0);

        let mut listener = TcpListener::from_std(listener);
        self.poll.registry().register(
            &mut listener,
            token,
            Interest::READABLE.add(Interest::WRITABLE),
        )?;
        let (acceptor, server) = ConnectionServer::new(server_state, listener)?;

        self.services.push(acceptor);

        Ok(server)
    }

    /// Dedicate a thread to your connection frontend.
    pub fn serve(self) -> Result<()> {
        let Self {
            mut poll,
            mut events,
            services,
        } = self;
        log::trace!("serving frontend");
        loop {
            if let Err(e) = poll.poll(&mut events, None) {
                if interrupted(&e) {
                    log::trace!("interrupted");
                    continue;
                }
                log::error!("failed {e:?}");
                return Err(e.into());
            }

            for event in events.iter() {
                let service_index = event.token().0;
                log::trace!("new connection for service {service_index}");
                services[service_index].accept()?;
            }
        }
    }
}
