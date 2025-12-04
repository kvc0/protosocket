use std::sync::{atomic::AtomicBool, Arc};

use tokio::sync::oneshot;

use super::reactor::completion_reactor::{DoNothingMessageHandler, RpcCompletionReactor};
use super::reactor::completion_registry::{Completion, CompletionGuard};
use super::reactor::{
    completion_streaming::StreamingCompletion, completion_unary::UnaryCompletion,
};
use crate::client::reactor::completion_reactor::{CompletableRpc, RpcNotification};
use crate::Message;

/// A client for sending RPCs to a protosockets rpc server.
///
/// It handles sending messages to the server and associating the responses.
/// Messages are sent and received in any order, asynchronously, and support cancellation.
/// To cancel an RPC, drop the response future.
#[derive(Debug)]
pub struct RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    submission_queue: spillway::Sender<RpcNotification<Response, Request>>,
    is_alive: Arc<AtomicBool>,
}

impl<Request, Response> Clone for RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    fn clone(&self) -> Self {
        Self {
            submission_queue: self.submission_queue.clone(),
            is_alive: self.is_alive.clone(),
        }
    }
}

impl<Request, Response> RpcClient<Request, Response>
where
    Request: Message,
    Response: Message,
{
    pub(crate) fn new(
        submission_queue: spillway::Sender<RpcNotification<Response, Request>>,
        message_reactor: &RpcCompletionReactor<
            Response,
            Request,
            DoNothingMessageHandler<Response>,
        >,
    ) -> Self {
        Self {
            submission_queue,
            is_alive: message_reactor.alive_handle(),
        }
    }

    /// Checking this before using the client does not guarantee that the client is still alive when you send  
    /// your message. It may be useful for connection pool implementations - for example, [bb8::ManageConnection](https://github.com/djc/bb8/blob/09a043c001b3c15514d9f03991cfc87f7118a000/bb8/src/api.rs#L383-L384)'s  
    /// is_valid and has_broken could be bound to this function to help the pool cycle out broken connections.  
    pub fn is_alive(&self) -> bool {
        self.is_alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Send a server-streaming rpc to the server.
    ///
    /// This function only sends the request. You must consume the completion stream to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub fn send_streaming(
        &self,
        request: Request,
    ) -> crate::Result<StreamingCompletion<Response, Request>> {
        let (sender, completion) = spillway::channel();
        let completion_guard = self.send_message(Completion::RemoteStreaming(sender), request)?;

        let completion = StreamingCompletion::new(completion, completion_guard);

        Ok(completion)
    }

    /// Send a unary rpc to the server.
    ///
    /// This function only sends the request. You must await the completion to get the response.
    #[must_use = "You must await the completion to get the response. If you drop the completion, the request will be cancelled."]
    pub fn send_unary(
        &self,
        request: Request,
    ) -> crate::Result<UnaryCompletion<Response, Request>> {
        let (completor, completion) = oneshot::channel();
        let completion_guard = self.send_message(Completion::Unary(completor), request)?;

        let completion = UnaryCompletion::new(completion, completion_guard);

        Ok(completion)
    }

    fn send_message(
        &self,
        completion: Completion<Response>,
        request: Request,
    ) -> crate::Result<CompletionGuard<Response, Request>> {
        if !self.is_alive.load(std::sync::atomic::Ordering::Relaxed) {
            // early-out if the connection is closed
            return Err(crate::Error::ConnectionIsClosed);
        }
        let message_id = request.message_id();
        let completion_guard = CompletionGuard::new(message_id, self.submission_queue.clone());

        self.submission_queue
            .send(RpcNotification::New(CompletableRpc {
                message_id,
                completion,
                request,
            }))
            .map_err(|_e| crate::Error::ConnectionIsClosed)
            .map(|_| completion_guard)
    }
}

#[cfg(test)]
mod test {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::task::Context;
    use std::task::Poll;

    use futures::task::noop_waker_ref;

    use crate::client::connection_pool::ClientConnector;
    use crate::client::connection_pool::ConnectionPool;
    use crate::client::reactor::completion_reactor::CompletableRpc;
    use crate::client::reactor::completion_reactor::DoNothingMessageHandler;
    use crate::client::reactor::completion_reactor::RpcCompletionReactor;
    use crate::client::reactor::completion_reactor::RpcNotification;
    use crate::Message;

    use super::RpcClient;

    impl Message for u64 {
        fn message_id(&self) -> u64 {
            *self & 0xffffffff
        }

        fn control_code(&self) -> crate::ProtosocketControlCode {
            match *self >> 32 {
                0 => crate::ProtosocketControlCode::Normal,
                1 => crate::ProtosocketControlCode::Cancel,
                2 => crate::ProtosocketControlCode::End,
                _ => unreachable!("invalid control code"),
            }
        }

        fn set_message_id(&mut self, message_id: u64) {
            *self = (*self & 0xf00000000) | message_id;
        }

        fn cancelled(message_id: u64) -> Self {
            (1_u64 << 32) | message_id
        }

        fn ended(message_id: u64) -> Self {
            (2 << 32) | message_id
        }
    }

    fn drive_future<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        loop {
            let next = f.as_mut().poll(&mut Context::from_waker(noop_waker_ref()));
            if let Poll::Ready(result) = next {
                break result;
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn get_client() -> (
        spillway::Receiver<RpcNotification<u64, u64>>,
        RpcClient<u64, u64>,
        RpcCompletionReactor<u64, u64, DoNothingMessageHandler<u64>>,
    ) {
        let (sender, remote_end) = spillway::channel();
        let rpc_reactor =
            RpcCompletionReactor::<u64, u64, _>::new(DoNothingMessageHandler::default());
        let client = RpcClient::new(sender, &rpc_reactor);
        (remote_end, client, rpc_reactor)
    }

    #[test]
    fn unary_drop_cancel() {
        let (mut remote_end, client, _reactor) = get_client();

        let response = client.send_unary(4).expect("can send");

        let notification = drive_future(remote_end.next()).expect("a request is sent");
        assert!(matches!(
            notification,
            RpcNotification::New(CompletableRpc { message_id: 4, .. })
        ));

        drop(response);

        let cancellation = drive_future(remote_end.next()).expect("a cancel is sent");
        assert!(matches!(cancellation, RpcNotification::Cancel(4)));
    }

    #[test]
    fn streaming_drop_cancel() {
        let (mut remote_end, client, _reactor) = get_client();

        let response = client.send_streaming(4).expect("can send");

        let notification = drive_future(remote_end.next()).expect("a request is sent");
        assert!(matches!(
            notification,
            RpcNotification::New(CompletableRpc { message_id: 4, .. })
        ));

        drop(response);

        let cancellation = drive_future(remote_end.next()).expect("a cancel is sent");
        assert!(matches!(cancellation, RpcNotification::Cancel(4)));
    }

    #[allow(clippy::type_complexity)]
    #[derive(Default)]
    struct TestConnector {
        clients: Mutex<
            Vec<(
                spillway::Receiver<RpcNotification<u64, u64>>,
                RpcClient<u64, u64>,
                RpcCompletionReactor<u64, u64, DoNothingMessageHandler<u64>>,
            )>,
        >,
        fail_connections: AtomicBool,
    }
    impl ClientConnector for Arc<TestConnector> {
        type Request = u64;
        type Response = u64;

        async fn connect(self) -> crate::Result<RpcClient<Self::Request, Self::Response>> {
            if self
                .fail_connections
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(crate::Error::IoFailure(Arc::new(std::io::Error::other(
                    "simulated connection failure",
                ))));
            }
            // normally I'd just call `protosocket_rpc::client::connect` in here
            let (remote_end, client, reactor) = get_client();
            self.clients
                .lock()
                .expect("mutex works")
                .push((remote_end, client.clone(), reactor));

            Ok(client)
        }
    }

    // have to use tokio::test for the connection pool because it uses tokio::spawn
    #[tokio::test]
    async fn connection_pool() {
        let connector = Arc::new(TestConnector::default());
        let pool = ConnectionPool::new(connector.clone(), 1);

        let rpc_client_a = pool
            .get_connection()
            .await
            .expect("can get a connection from the pool");
        assert_eq!(
            1,
            connector.clients.lock().expect("mutex works").len(),
            "one connection created"
        );

        let rpc_client_b = pool
            .get_connection()
            .await
            .expect("can get a connection from the pool");
        assert_eq!(
            1,
            connector.clients.lock().expect("mutex works").len(),
            "still one connection created"
        );

        assert!(
            Arc::ptr_eq(&rpc_client_a.is_alive, &rpc_client_b.is_alive),
            "same connection shared"
        );

        let _reply_a = rpc_client_a.send_unary(42).expect("can send");
        let _reply_b = rpc_client_b.send_unary(43).expect("can send");

        let (mut remote_end, _client, _reactor) = {
            let mut clients = connector.clients.lock().expect("mutex works");
            clients.pop().expect("one client exists")
        };
        assert!(matches!(
            remote_end.next().await.expect("request a is received"),
            RpcNotification::New(CompletableRpc { message_id: 42, .. })
        ));
        assert!(matches!(
            remote_end.next().await.expect("request b is received"),
            RpcNotification::New(CompletableRpc { message_id: 43, .. })
        ));
    }

    #[tokio::test]
    async fn connection_pool_reconnect() {
        let connector = Arc::new(TestConnector::default());
        let pool = ConnectionPool::new(connector.clone(), 1);

        let rpc_client_a = pool
            .get_connection()
            .await
            .expect("can get a connection from the pool");
        assert_eq!(
            1,
            connector.clients.lock().expect("mutex works").len(),
            "one connection created"
        );

        rpc_client_a
            .is_alive
            .store(false, std::sync::atomic::Ordering::Relaxed);

        let rpc_client_b = pool.get_connection().await.expect("can get a connection from the pool even when the previous connection is dead, as long as the connection attempt succeeds");
        assert_eq!(
            2,
            connector.clients.lock().expect("mutex works").len(),
            "a new connection was created, so the connector was asked to make a new connection"
        );
        // Note that the connection pool holds a plain Vec of individual clients, so it cannot create more connections than it started with.

        assert!(
            !Arc::ptr_eq(&rpc_client_a.is_alive, &rpc_client_b.is_alive),
            "new connection created"
        );
    }

    #[tokio::test]
    async fn connection_pool_failure() {
        let connector = Arc::new(TestConnector::default());
        let pool = ConnectionPool::new(connector.clone(), 1);
        connector
            .fail_connections
            .store(true, std::sync::atomic::Ordering::Relaxed);

        pool.get_connection().await.expect_err("connection attempt fails, and the calling code gets the error. It does not try forever without surfacing errors.");
    }

    #[tokio::test]
    async fn connection_pool_reconnect_failure_recovery() {
        let connector = Arc::new(TestConnector::default());
        let pool = ConnectionPool::new(connector.clone(), 1);
        let rpc_client_a = pool
            .get_connection()
            .await
            .expect("can get a connection from the pool");

        rpc_client_a
            .is_alive
            .store(false, std::sync::atomic::Ordering::Relaxed);
        connector
            .fail_connections
            .store(true, std::sync::atomic::Ordering::Relaxed);

        pool.get_connection().await.expect_err("connection attempt fails, and the calling code gets the error. It does not try forever without surfacing errors.");

        connector
            .fail_connections
            .store(false, std::sync::atomic::Ordering::Relaxed);

        let rpc_client_b = pool
            .get_connection()
            .await
            .expect("can get a connection from the pool now");
        assert!(
            !Arc::ptr_eq(&rpc_client_a.is_alive, &rpc_client_b.is_alive),
            "new connection created"
        );
    }
}
