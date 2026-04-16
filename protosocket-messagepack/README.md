# protosocket-messagepack

MessagePack codec adapters for `protosocket`.

Provides `MessagePackSerializer` and `MessagePackDecoder`.
`MessagePackSerializer` implements `protosocket::Serialize` for use with the `PooledEncoder`.
`MessagePackDecoder` implements `protosocket::Decoder`. Wire these up as the codec for any
`protosocket::Connection` or `protosocket-rpc` service.

## Wire format

Each message is framed as a 5-byte MessagePack-encoded `u32` length prefix followed by the
MessagePack-encoded payload. The decoder is stateful and handles messages that arrive split
across multiple reads.

## Usage

```rust
use protosocket_messagepack::{MessagePackSerializer, MessagePackDecoder};
use protosocket::PooledEncoder;

// Encoder: wrap in PooledEncoder to reuse send buffers
type Encoder = PooledEncoder<MessagePackSerializer<MyRequest>>;
// Decoder
type Decoder = MessagePackDecoder<MyResponse>;

// Combine into a codec tuple for protosocket
let codec = (Encoder::default(), Decoder::default());
```

Your message type must implement `serde::Serialize` for the serializer and
`serde::de::DeserializeOwned` for the decoder.

See `example-messagepack` for a complete working client and server.
