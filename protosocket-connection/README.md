# protosocket

`protosocket` provides a bidirectional, asynchronous connection
handler. It works with Tokio's strengths to efficiently manage
message-oriented streams.

```
Your application sends outbound messages here.
 │
 │
 │         ┌─────── Connection ────────┐
 ▼         │                           │
Sender ────┼──▶  encode ───▶ writev ───┼──▶ remote
(channel)  │                           │
           │                           │
Reactor ◀──┼───  decode ◀───  read  ◀──┼─── remote
(callback) │                           │
 ▲         └───────────────────────────┘
 │
 │
Your application implements (or uses a) Reactor,
and receives messages from the remote here.
```

## Key Features
- Low latency via low abstraction - no http or higher level constructs
- Asynchronous I/O using `mio` via `tokio`
- Flexible message types
- Flexible buffer management

## Concepts
The `Connection` is a Future: You spawn it onto a Tokio runtime (or a
JoinSet or whatever).

Its I/O is a little different from many other frameworks you're used to.
Where many of these systems use a simple `Channel` receiver for input, and
sender for output, `Connection`s only use a `Channel` sender for output.
For the input side, Protosocket provides the notion of a `Reactor`.

`Reactor`s are how you handle inbound messages. They can have state, and
receive `&mut self` with each message. This is particularly useful to
low-latency and low-overhead use cases.

On a server `Connection`, a typical `Reactor` will directly handle or spawn
handlers for each inbound message.

On a client `Connection`, a typical `Reactor` will complete RPC calls that were
previously sent.

Of course, the `Connection` simply gives you a `Sender` half of a channel, and
calls your `Reactor` on inbound messages. So what you want to do with your
messages, whether ordering, request/response, broadcast, or anything else is
entirely up to you.

If you want to use a `Connection` with RPC semantics like multiplexing,
cancellation, streaming, and request/response pairing, you should look at
`protosocket-rpc`. That project implements these common requirements so you
can focus on your messages and business logic more than your network, and
still get the benefits of a low-abstraction, Tokio-friendly transport.
