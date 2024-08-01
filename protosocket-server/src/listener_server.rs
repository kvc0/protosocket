use mio::{net::TcpListener, Events, Interest, Poll, Token};
use tokio::sync::mpsc;

use crate::{
    connection_acceptor::{ConnectionAcceptor, NewConnection},
    connection_server::ConnectionServer,
    interrupted, Result, ServerConnector,
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
    ///
    /// see `register_multithreaded_service_listener` for a configuration that has multiple IO drivers for the service.
    pub fn register_service_listener<Connector: ServerConnector>(
        &mut self,
        listener: std::net::TcpListener,
        server_state: Connector,
    ) -> Result<ConnectionServer<Connector>> {
        let inbound_connection_assignments = self
            .register_acceptor(listener, 1)?
            .pop()
            .expect("Thread count is 1");

        ConnectionServer::new(server_state, inbound_connection_assignments)
    }

    /// Configure a single tcp listener to vend connections to separate IO drivers (one driver per connection server).
    ///
    /// Each of the returned ConnectionServers can be spawned onto distinct threads, and their connections will be
    /// segregated. Connections are vended to the backing threads in round-robin fashion. There is no work stealing
    /// between the ConnectionServers - they are fully distinct.
    ///
    /// ConnectionServers can reuse the same task work pool if desired, or they can use separate async work pools.
    /// What works best for you is what you should do.
    ///
    /// listener must be configured non_blocking
    pub fn register_multithreaded_service_listener<Connector: ServerConnector + Clone>(
        &mut self,
        listener: std::net::TcpListener,
        server_state: Connector,
        thread_count: usize,
    ) -> Result<Vec<ConnectionServer<Connector>>> {
        let inbound_streams = self.register_acceptor(listener, thread_count)?;

        inbound_streams
            .into_iter()
            .map(|inbound_connection_assignments| {
                ConnectionServer::new(server_state.clone(), inbound_connection_assignments)
            })
            .collect()
    }

    fn register_acceptor(
        &mut self,
        listener: std::net::TcpListener,
        thread_count: usize,
    ) -> Result<Vec<mpsc::UnboundedReceiver<NewConnection>>> {
        let token = Token(self.services.len());
        log::trace!("new service listener index {} on {listener:?}", token.0);

        let mut listener = TcpListener::from_std(listener);
        self.poll.registry().register(
            &mut listener,
            token,
            Interest::READABLE.add(Interest::WRITABLE),
        )?;

        let (acceptor, inbound_streams) = ConnectionAcceptor::new(listener, thread_count);
        self.services.push(acceptor);
        Ok(inbound_streams)
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
