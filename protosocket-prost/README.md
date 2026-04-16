# protosocket-prost

Protocol Buffers codec adapters for `protosocket` via [`prost`](https://github.com/tokio-rs/prost).

Provides `ProstSerializer` and `ProstDecoder`, which implement the `protosocket::Serialize` and
`protosocket::Decoder` traits for prost-generated types. Use them as the codec for any
`protosocket::Connection` or `protosocket-rpc` service.

## Wire format

Messages are framed using prost's length-delimited encoding: a varint byte-count prefix
followed by the raw protobuf bytes.

## Usage

```rust
use protosocket_prost::{ProstSerializer, ProstDecoder};
use protosocket::PooledEncoder;

// Encoder: wrap in PooledEncoder to reuse send buffers
type Encoder = PooledEncoder<ProstSerializer<MyRequest>>;
// Decoder
type Decoder = ProstDecoder<MyResponse>;

// Combine into a codec tuple for a protosocket Connection
let codec = (Encoder::default(), Decoder::default());
```

`T` must implement `prost::Message` (+ `Default` for the decoder).

See `example-proto` for a complete client and server, and `example-proto-tls` for TLS.