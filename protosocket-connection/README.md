# protosocket-connection

`protosocket-connection` provides a flexible, asynchronous TCP connection handler. It's designed to efficiently manage bidirectional, message-oriented TCP streams with customizable serialization and deserialization.

## Key Features
- Low abstraction - no http or higher level constructs
- Asynchronous I/O using `mio` via `tokio`
- Customizable message types through `ConnectionBindings`
- Efficient buffer management and flexible error handling

## Flow Diagrams

The `poll()` function on `connection.rs` controls the lifecycle of the entire connection. You're recommended to read individual comments on the code to understand the flow, but below is a sequence diagram to get you started:

```mermaid
sequenceDiagram
    participant P as Poll
    participant IS as Inbound Socket
    participant D as Deserializer
    participant R as Reactor
    participant S as Serializer
    participant OS as Outbound Socket

    P->>IS: Read from inbound socket
    IS->>D: Raw data
    D->>P: Deserialize inbound messages
    P->>R: Submit inbound messages
    R-->>P: Process messages
    P->>S: Prepare and serialize outbound messages
    S->>P: Serialized outbound bytes
    P->>OS: Write to outbound socket
```