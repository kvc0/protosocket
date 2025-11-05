use std::{
    cell::RefCell,
    future::Future,
    pin::{pin, Pin},
    sync::Mutex,
    task::{Context, Poll},
};

use futures::FutureExt;
use rand::{Rng, SeedableRng};

use crate::{client::RpcClient, Message};

/// A connection strategy for protosocket rpc clients.
///
/// This is called asynchronously by the connection pool to create new connections.
pub trait ClientConnector: Clone {
    type Request: Message;
    type Response: Message;

    /// Connect to the server and return a new RpcClient. See [`crate::client::connect`] for
    /// the typical way to connect. This is called by the ConnectionPool when it needs a new
    /// connection.
    ///
    /// Your returned future needs to be `'static`, and your connector needs to be cheap to
    /// clone. One easy way to do that is to just impl ClientConnector on `Arc<YourConnectorType>`
    /// instead of directly on `YourConnectorType`.
    ///
    /// If you have rolling credentials, initialization messages, changing endpoints, or other
    /// adaptive connection logic, this is the place to do it or consult those sources of truth.
    fn connect(
        self,
    ) -> impl Future<Output = crate::Result<RpcClient<Self::Request, Self::Response>>> + Send + 'static;
}

/// A connection pool for protosocket rpc clients.
///
/// Protosocket-rpc connections are shared and multiplexed, so this vends cloned handles.
/// You can hold onto a handle from the pool for as long as you want. There is a small
/// synchronization cost to getting a handle from a pool, so caching is a good idea - but
/// if you want to load balance a lot, you can just make a pool with as many "slots" as you
/// want to dilute any contention on connection state locks. The locks are typically held for
/// the time it takes to clone an `Arc`, so it's usually nanosecond-scale synchronization,
/// per connection. So if you have several connections, you'll rarely contend.
#[derive(Debug)]
pub struct ConnectionPool<Connector: ClientConnector> {
    connector: Connector,
    connections: Vec<Mutex<ConnectionState<Connector::Request, Connector::Response>>>,
}

impl<Connector: ClientConnector> ConnectionPool<Connector> {
    /// Create a new connection pool.
    ///
    /// It will try to maintain `connection_count` healthy connections.
    pub fn new(connector: Connector, connection_count: usize) -> Self {
        Self {
            connector,
            connections: (0..connection_count)
                .map(|_| Mutex::new(ConnectionState::Disconnected))
                .collect(),
        }
    }

    /// Get a consistent connection from the pool for a given key.
    pub async fn get_connection_for_key(
        &self,
        key: usize,
    ) -> crate::Result<RpcClient<Connector::Request, Connector::Response>> {
        let slot = key % self.connections.len();

        self.get_connection_by_slot(slot).await
    }

    /// Get a connection from the pool.
    pub async fn get_connection(
        &self,
    ) -> crate::Result<RpcClient<Connector::Request, Connector::Response>> {
        thread_local! {
            static THREAD_LOCAL_SMALL_RANDOM: RefCell<rand::rngs::SmallRng> = RefCell::new(rand::rngs::SmallRng::from_os_rng());
        }

        // Safety: This is executed on a thread, in only one place. It cannot be borrowed anywhere else.
        let slot = THREAD_LOCAL_SMALL_RANDOM
            .with_borrow_mut(|rng| rng.random_range(0..self.connections.len()));

        self.get_connection_by_slot(slot).await
    }

    async fn get_connection_by_slot(
        &self,
        slot: usize,
    ) -> crate::Result<RpcClient<Connector::Request, Connector::Response>> {
        let connection_state = &self.connections[slot];

        // The connection state requires a mutex, so I need to keep await out of the scope to satisfy clippy (and for paranoia).
        let connecting_handle = loop {
            let mut state = connection_state.lock().expect("internal mutex must work");
            break match &mut *state {
                ConnectionState::Connected(shared_connection) => {
                    if shared_connection.is_alive() {
                        return Ok(shared_connection.clone());
                    } else {
                        *state = ConnectionState::Disconnected;
                        continue;
                    }
                }
                ConnectionState::Connecting(join_handle) => join_handle.clone(),
                ConnectionState::Disconnected => {
                    let connector = self.connector.clone();
                    let load = SpawnedConnect {
                        inner: tokio::task::spawn(connector.connect()),
                    }
                    .shared();
                    *state = ConnectionState::Connecting(load.clone());
                    continue;
                }
            };
        };

        match connecting_handle.await {
            Ok(client) => Ok(reconcile_client_slot(connection_state, client)),
            Err(connect_error) => {
                let mut state = connection_state.lock().expect("internal mutex must work");
                *state = ConnectionState::Disconnected;
                Err(connect_error)
            }
        }
    }
}

fn reconcile_client_slot<Request, Response>(
    connection_state: &Mutex<ConnectionState<Request, Response>>,
    client: RpcClient<Request, Response>,
) -> RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    let mut state = connection_state.lock().expect("internal mutex must work");
    match &mut *state {
        ConnectionState::Connecting(_shared) => {
            // Here we drop the shared handle. If there is another task still waiting on it, they will get notified when
            // the spawned connection task completes. When they come to reconcile with the connection slot, they will
            // favor this connection and drop their own.
            *state = ConnectionState::Connected(client.clone());
            client
        }
        ConnectionState::Connected(rpc_client) => {
            if rpc_client.is_alive() {
                // someone else beat us to it
                rpc_client.clone()
            } else {
                // well this one is broken too, so we should just replace it with our new one
                *state = ConnectionState::Connected(client.clone());
                client
            }
        }
        ConnectionState::Disconnected => {
            // we raced with a disconnect, but we have a new client, so use it
            *state = ConnectionState::Connected(client.clone());
            client
        }
    }
}

struct SpawnedConnect<Request, Response>
where
    Request: Message,
    Response: Message,
{
    inner: tokio::task::JoinHandle<crate::Result<RpcClient<Request, Response>>>,
}
impl<Request, Response> Future for SpawnedConnect<Request, Response>
where
    Request: Message,
    Response: Message,
{
    type Output = crate::Result<RpcClient<Request, Response>>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match pin!(&mut self.inner).poll(context) {
            Poll::Ready(Ok(client_result)) => Poll::Ready(client_result),
            Poll::Ready(Err(_join_err)) => Poll::Ready(Err(crate::Error::ConnectionIsClosed)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug)]
enum ConnectionState<Request, Response>
where
    Request: Message,
    Response: Message,
{
    Connecting(futures::future::Shared<SpawnedConnect<Request, Response>>),
    Connected(RpcClient<Request, Response>),
    Disconnected,
}
