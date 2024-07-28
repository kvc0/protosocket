use mio::{net::TcpListener, Events, Interest, Poll, Token};
use protosocket_connection::ConnectionLifecycle;

use crate::{
    connection_acceptor::ConnectionAcceptor, connection_server::ConnectionServer, interrupted,
    Result,
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

    /// address like "127.0.0.1:9000"
    pub fn register_service_listener<Lifecycle: ConnectionLifecycle>(
        &mut self,
        address: impl Into<String>,
        server_state: Lifecycle::ServerState,
    ) -> Result<ConnectionServer<Lifecycle>> {
        let addr = address.into().parse()?;
        let token = Token(self.services.len());
        log::trace!("new service listener index {} on {addr:?}", token.0);
        let mut listener = TcpListener::bind(addr)?;

        self.poll.registry().register(
            &mut listener,
            token,
            Interest::READABLE.add(Interest::WRITABLE),
        )?;
        let (acceptor, server) = ConnectionServer::new(server_state, listener);

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
