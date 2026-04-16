# protosocket-server

Raw server scaffolding for `protosocket` connections.

This crate sits one layer below `protosocket-rpc`. Use it when you want direct control over
the `MessageReactor` for each connection — for example, when you need a custom message
ordering or streaming protocol that doesn't fit the request/response RPC pattern.

You can implement RESP or memcache, or just about any other message-oriented protocol with
this low level of abstraction.

If you want unary or server-streaming RPC out of the box, use `protosocket-rpc` instead.


## How it works

Implement `ServerConnector` to describe your codec and how to build a per-connection
`MessageReactor`. Then pass your connector to `ProtosocketServerConfig` to bind a listener
and get a server future you can spawn or `await`, depending on where you want the connection
to run:

```rust
use protosocket_server::{ServerConnector, ProtosocketServerConfig};

struct MyConnector;

impl ServerConnector for MyConnector {
    type Codec = /* your Encoder + Decoder */;
    type Reactor = /* your MessageReactor */;
    type SocketListener = TcpSocketListener;

    fn codec(&self) -> Self::Codec { /* ... */ }

    fn new_reactor(
        &self,
        outbound: spillway::Sender</* LogicalOutbound */>,
        stream: &StreamWithAddress<TcpStream>,
    ) -> Self::Reactor {
        // Build your reactor, store the outbound sender to push replies
    }

    fn spawn_connection(&self, connection: Connection</* ... */>) {
        tokio::spawn(connection);
    }
}

let server = ProtosocketServerConfig::default()
    .max_buffer_length(16 * 1024 * 1024)
    .bind_tcp("0.0.0.0:8080".parse()?, MyConnector)?;

tokio::spawn(server);
```

## Configuration

`ProtosocketServerConfig` controls buffer sizes per connection:

|           Setting              | Default | Description |
|--------------------------------|---------|-------------|
| `max_buffer_length`            | 32 MiB  | Maximum receive buffer per connection |
| `max_queued_outbound_messages` | 128     | Outbound message queue depth |
| `buffer_allocation_increment`  | 1 MiB   | Grow the receive buffer in steps of this size |

`ProtosocketSocketConfig` controls TCP options (nodelay, reuseaddr/reuseport, keepalive, backlog).
The defaults enable nodelay and reuse with a backlog of 65536.

See `example-telnet` for the simplest full example.
