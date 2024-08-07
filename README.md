# protosocket
Tools for building message-oriented tcp streams.

A protosocket is a non-blocking, bidirectional, message streaming connection.
Providing a serializer and deserializer for your messages, you can stream to
and from tcp servers.

Protosockets avoid too many opinions - you have (get?) to choose your own
message ordering and concurrency semantics. You can make an implicitly ordered
stream, or a non-blocking out-of-order stream, or anything in between.

Tools to facilitate protocol buffers over tcp are provided in [`protosocket-prost`](./protosocket-prost/).
You can see the protocol buffers example in [`example-proto`](./example-proto/).
If you're only using rust, of course you can hand-write prost structs, but if you
want to use a protosocket server with clients in other languages you'll want to
generate from protos.
