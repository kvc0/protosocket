# protosocket-rpc
For making RPC servers and clients.

This crate provides an rpc-style client and server for `protosocket` connections.

If you want to make requests to a server and get replies, this is the tool for you.

If you have very special requirements, like ordered responses without message ids
(RESP, memcached wire protocols) or messages that have nothing to do with request &
response semantics, you might need to use `protosocket` directly.

* See `example-proto` for an example of how to make a protocol buffer client/server.
* See `example-messagepack` for an example of how to make a messagepack client/server.

# Some notes
* It does as little as it can.
  * What you encode is what is on the wire. `0` bits are snuck in outside of your control.
  * You do not have to `spawn` if you don't have to. You can though, and it works great.
  * You can output messages with zero-copy serialization.
* It does not require `Send` (u_ring may be possible in the future)
* RPC cancellation is provided.
* Response streaming is provided.
* Unopinionated about wire format: You can do RPC with any encoding you want.

# Details & guidance
You can use whatever encoding you want, but you must provide both
an `Encoder` and a `Decoder` for your messages. If you use `prost`, you can use
the `protosocket-prost` crate to provide these implementations. There's also
a `protosocket-messagepack` available, and other encodings are similarly
straightforward to add.

Messages must provide a `Message` implementation, which includes a `message_id`
and a `control_code`. The `message_id` is used to correlate requests and responses,
while the `control_code` is used to provide special handling for RPC messages, like
cancellation. Your messages have to be able to convey these somehow, but you're free
to do that however works best for you.

This RPC layer is medium-low level wrapper around the low level protosocket crate. You
are expected to write a client wrapper with functions that make sense for your
application, and use the protosocket-rpc client internally as the transport layer.

Clients and servers need to agree about the request and response semantics. While it is
supported to have dynamic streaming/unary response types, it is recommended to instead
use separate rpc-initiating request messages for streaming and unary responses.

Protosocket rpc lets you choose any encoding and does not wrap your messages at all. The
bytes you encode are the bytes which are sent.

## A diagram
Each RPC is its own interaction: You can have thousands of concurrent RPCs on a single
connection, all streaming, completing, or cancelling as each wants to.
```
Client                      wire                      Server

RpcClient                                         SocketRpcServer
    │                                                    │
    │                   UNARY                            │
    │                                                    │
    ├── send_unary(req) ──── request (id=N) ────────────▶│ new_rpc(req, responder)
    │                                                    │      │
    │   UnaryCompletion ◀─── response (id=N) ────────────┤   responder.unary(future)
    │   .await                                           │   (you get the response)
    │                                                    │
    │                  STREAMING                         │
    │                                                    │
    ├── send_streaming(req) ─ request (id=M) ───────────▶│ new_rpc(req, responder)
    │                                                    │      │
    │   StreamingCompletion ◀ response (id=M) ───────────┤   responder.stream(stream)
    │   .next().await     ◀── response (id=M) ───────────┤   (you get the next message for response M)
    │   .next().await     ◀── End (id=M) ────────────────┤   (stream ended)
    │                                                    │
    │                 CANCELLATION                       │
    │                                                    │
    ├── send_unary(req) ──── request (id=L) ────────────▶│ new_rpc(req, responder)
    │                                                    │      │
    ├── drop(completion) ──── Cancel (id) ──────────────▶│   (task aborted)
```

