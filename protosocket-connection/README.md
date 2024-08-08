# protosocket-connection

`protosocket-connection` is a Rust library crate that provides a flexible, asynchronous TCP connection handler. It's designed to efficiently manage bidirectional, message-oriented TCP streams with customizable serialization and deserialization.

## Key Features
- Asynchronous I/O using Tokio
- Customizable message types through `ConnectionBindings`
- Efficient buffer management and flexible error handling

## Overview

The recommended order to read this library crate is

- `types.rs`
- `connection.rs`

### Flow Diagrams

This repository has the core components for a protosocket Connection. It encapsulates all the required types for the 
Connection within `ConnectionBindings` allowing you to dynamically provide the required `Message` type (like `String`) backing your connection. Below is a simple component diagram for the `Connection`:

```mermaid
classDiagram
    class Connection {
        -TcpStream stream
        -Bindings::Deserializer deserializer
        -Bindings::Serializer serializer
        -Bindings::Reactor reactor
        +poll(context: &mut Context) Poll<()>
    }

    class ConnectionBindings {
        <<trait>>
        +type Deserializer
        +type Serializer
        +type Reactor
    }

    class Serializer {
        <<trait>>
        +type Message
        +encode(message: Self::Message, buffer: &mut impl BufMut)
    }

    class Deserializer {
        <<trait>>
        +type Message
        +decode(buffer: impl Buf) Result<(usize, Self::Message), DeserializeError>
    }

    class MessageReactor {
        <<trait>>
        +type Inbound
        +on_inbound_messages(messages: impl IntoIterator<Item = Self::Inbound>) ReactorStatus
    }

    class DeserializeError {
        <<enum>>
        IncompleteBuffer
        InvalidBuffer
        SkipMessage
    }

    class ReactorStatus {
        <<enum>>
        Continue
        Disconnect
    }

    Connection "1" --> "1" ConnectionBindings : uses
    ConnectionBindings --> Deserializer : associates
    ConnectionBindings --> Serializer : associates
    ConnectionBindings --> MessageReactor : associates
    Deserializer --> DeserializeError : uses
    MessageReactor --> ReactorStatus : returns
    Connection ..> Deserializer : uses
    Connection ..> Serializer : uses
    Connection ..> MessageReactor : uses
```

The `poll()` function on `connection.rs` controls the lifecycle of the entire connection. You're recommended to read individual comments on the code to understand the flow, but below is a sequence diagram to get you started:

```mermaid
sequenceDiagram
    participant P as poll
    participant S as TcpStream
    participant RB as receive_buffer
    participant D as Deserializer
    participant IQ as inbound_messages
    participant R as MessageReactor
    participant OQ as outbound_messages
    participant Ser as Serializer
    participant SB as send_buffer

    P->>S: poll_read_ready
    alt Stream is ready
        S->>P: Ready
        P->>S: read_from_stream
        S->>RB: Raw bytes
        P->>D: decode
        D->>IQ: Deserialized messages
        P->>R: on_inbound_messages
        R-->>P: ReactorStatus
        alt ReactorStatus::Disconnect
            P->>P: Return Poll::Ready(())
        end
    else Stream is not ready
        S->>P: Pending
    end

    P->>OQ: Check for outbound messages
    alt Outbound messages exist
        OQ->>Ser: Messages to serialize
        Ser->>SB: Serialized data
        P->>S: poll_write_ready
        alt Stream is ready for writing
            S->>P: Ready
            P->>S: writev_buffers
            SB->>S: Send data
        else Stream is not ready for writing
            S->>P: Pending
        end
    end

    P->>P: Return Poll::Pending (wake me later)
```