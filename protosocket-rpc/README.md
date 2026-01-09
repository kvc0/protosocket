# protosocket-rpc
For making RPC servers and clients.

This crate provides an rpc-style client and server for protosocket connections.
If you have very special requirements, like an ordered response requirement,
you might need to use `protosocket` directly.

* See `example-proto` for an example of how to use this crate with protocol buffers.
* See `example-messagepack` for an example of how to use this crate with messagepack.

# Main features
* It does as little as it can
  * What you encode is what is on the wire
  * You do not have to `spawn` if you don't have to
  * You can output messages with zero-copy serialization with zero copies
* It does not require `Send`
* RPC cancellation
* Response streaming
* Unopinionated about serialization format: You can do RPC with any format you want.

# About
You can use whatever encoding you want, but you must provide both
an `Encoder` and a `Decoder` for your messages. If you use `prost`, you can use
the `protosocket-prost` crate to provide these implementations. There's also
a `protosocket-messagepack` available, and other encodings are similarly
straightforward to add.

Messages must provide a `Message` implementation, which includes a `message_id`
and a `control_code`. The `message_id` is used to correlate requests and responses,
while the `control_code` is used to provide special handling for RPC messages, like
cancellation.

This RPC layer is medium-low level wrapper around the low level protosocket crate. You
are expected to write a wrapper with the functions that make sense for your application,
and use this client as the transport layer.

Clients and servers need to agree about the request and response semantics. While it is
supported to have dynamic streaming/unary response types, it is recommended to instead
use separate rpc-initiating request messages for streaming and unary responses.

Protosocket rpc lets you choose any encoding and does not wrap your messages at all. The
bytes you encode are the bytes which are sent.
