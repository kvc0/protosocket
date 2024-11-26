# protosocket-rpc
For making RPC servers and clients.

A protosocket rpc server consists of a couple key traits:

* `SocketService`: Your service that takes new connections and produces `ConnectionService`s.
* `ConnectionService`: Your service that manages a connection, creating new RPC futures and doing bookkeeping.
* `Message`: Your way to get protosocket metadata out of your encoded messages.

A protosocket rpc client is a little more basic, just relying on common protosocket traits and `Message`.

Protosocket rpc lets you choose any encoding and does not wrap your messages at all. The bytes you
send are the bytes which are sent. This means you need to provide a way to communicate the basic protosocket
metadata on each message: A message_id u64 and a control code u8. The `Message` trait helps to ensure you get
the needful functions wired through.
