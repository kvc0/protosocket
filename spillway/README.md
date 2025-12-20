# Spillway
A high-throughput multi-producer, single-consumer (MPSC) channel.

This channel was made to relieve a performance bottleneck in the
`protosocket` ecosystem.

## Usage
```rust
tokio_test::block_on(async {
    use spillway::channel;

    let (sender_1, mut receiver) = channel();

    // Send is synchronous
    sender_1.send(1);

    // Cloning a sender results in a different cursor with a separate order
    let sender_2 = sender_1.clone();

    // interleave messages to illustrate ordering
    sender_2.send(2);
    sender_1.send(3);
    sender_2.send(4);

    // Only order per-sender is guaranteed
    assert_eq!(Some(1), receiver.next().await, "sender 1 is consumed first");
    sender_1.send(5);
    assert_eq!(Some(3), receiver.next().await, "sender 1 had 1-3-5, but 5 is in another delivery batch.");

    assert_eq!(Some(2), receiver.next().await, "sender 2 is consumed next");
    assert_eq!(Some(4), receiver.next().await);

    let sender_3 = sender_1.clone();
    sender_3.send(6);
    assert_eq!(Some(6), receiver.next().await, "yes, sender_1 sent 5 first, but lanes are serviced round-robin.");
    assert_eq!(Some(5), receiver.next().await, "and finally, we made it back around to 5");
})
```

## About Ordering
Many MPSC channel implementations guarantee ordering across senders, when
you externally guarantee happens-before. They might not claim it, but their
implementations often tend to be fancy OCC loops with final ordering
determined at `send()` time.

Spillway ordering is only determined per-sender at `send()` time. This is
how many senders can send with very little contention between them.

For each `Sender`, its messages will appear in order to the `Receiver`. Any
other `Sender`'s messages may appear before, between, or after this `Sender`'s
messages. But each `Sender`'s messages will appear only in the order the messages
were submitted to the `Sender`.

## How it works
There's no unsafe code in this channel. It's basically a `Vec<Mutex<VecDeque<T>>>`.
Each `Sender` gets an index `i` at creation time, and that `i` is valid for the outer
`Vec`. That decides which `Mutex` will block this `Sender`, and which "chute" or "shard"
or "lane" will order the messages from this `Sender`.

The `Receiver` holds a buffer of messages. When it's empty, it advances to the next
chute with messages in it, and swaps its empty buffer for the chute's messages. It
then resumes fulfilling `next()` by `pop_front()` on the current buffered `VecDeque<T>`.

## Performance
In benchmark tests with various processor architectures, the
`channel` bench on this repository shows the `tokio` mpsc channel
producing around 5-10mhz throughput. The `spillway` channel outperforms
typically by a factor of around 20x. For example, on my older m1 macbook:

||tokio|spillway|
|-|-|-|
|throughput|4.5mhz|92.7mhz|
|latency|219.4ns|10.8ns
|std deviation|16.6ns|2.5ns

