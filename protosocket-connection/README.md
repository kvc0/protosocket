# protosocket

`protosocket` provides a flexible, asynchronous connection
handler. It's designed to efficiently manage bidirectional,
message-oriented streams with custom serialization and
deserialization.

## Key Features
- Low abstraction - no http or higher level constructs
- Asynchronous I/O using `mio` via `tokio`
- Flexible custom message types
- Efficient, flexible buffer management

## Flow Diagrams

The `poll()` function on `connection.rs` controls the lifecycle of the entire connection. You're recommended to read individual comments on the code to understand the flow, but below is a sequence diagram to get you started:

```mermaid
sequenceDiagram
    participant P as Poll
    participant IS as Inbound Socket
    participant D as Decoder
    participant R as Reactor
    participant S as Encoder
    participant OS as Outbound Socket

    P->>IS: Read from inbound socket
    IS->>D: Connection read buffer
    D->>P: Deserialize inbound messages
    P->>R: Submit inbound messages
    R-->>P: Create response messages (asynchronous)
    P->>S: Prepare and serialize outbound messages
    S->>P: Serialized outbound buffer (bytes or other serialized view of a message)
    P->>OS: Write to outbound socket
```