//! TCP side-channel used to bootstrap a fabric connection.
//!
//! libfabric reliable-datagram endpoints have no `connect`/`accept`: before two
//! peers can exchange fabric messages they must each learn the other's raw
//! fabric address and insert it into their address vector. We do that exchange
//! over an ordinary TCP connection (the same approach NCCL and MPI use to
//! bootstrap).
//!
//! The wire format is a single fixed header followed by the raw address bytes,
//! sent by each side:
//!
//! ```text
//! magic:    4 bytes  b"PSEF"
//! version:  1 byte   PROTOCOL_VERSION
//! addr_len: 4 bytes  little-endian u32
//! addr:     addr_len bytes
//! ```

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::error::EfaError;

const MAGIC: [u8; 4] = *b"PSEF";
const PROTOCOL_VERSION: u8 = 1;
const HEADER_LEN: usize = 4 + 1 + 4;
/// Reject absurd address lengths to avoid trusting a peer-supplied size.
const MAX_ADDR_LEN: usize = 4096;

/// Encode a `hello` frame announcing our raw fabric `address`.
fn encode_hello(address: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(HEADER_LEN + address.len());
    frame.extend_from_slice(&MAGIC);
    frame.push(PROTOCOL_VERSION);
    frame.extend_from_slice(&(address.len() as u32).to_le_bytes());
    frame.extend_from_slice(address);
    frame
}

/// Parse the fixed header, returning the announced address length.
fn decode_header(header: &[u8]) -> Result<usize, EfaError> {
    if header.len() < HEADER_LEN {
        return Err(EfaError::Bootstrap(
            "handshake header too short".to_string(),
        ));
    }
    if header[0..4] != MAGIC {
        return Err(EfaError::Bootstrap("handshake magic mismatch".to_string()));
    }
    if header[4] != PROTOCOL_VERSION {
        return Err(EfaError::Bootstrap(format!(
            "unsupported handshake version {}",
            header[4]
        )));
    }
    let addr_len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
    if addr_len == 0 || addr_len > MAX_ADDR_LEN {
        return Err(EfaError::Bootstrap(format!(
            "implausible fabric address length {addr_len}"
        )));
    }
    Ok(addr_len)
}

/// Exchange raw fabric addresses with the peer over `stream`.
///
/// Sends our `local_address` and returns the peer's. Both sides call this; the
/// exchange is symmetric so it does not deadlock (each writes fully before it
/// needs the other's bytes only as far as TCP buffering allows, and addresses
/// are small).
pub async fn exchange(stream: &mut TcpStream, local_address: &[u8]) -> Result<Vec<u8>, EfaError> {
    stream.set_nodelay(true).ok();

    let frame = encode_hello(local_address);
    stream.write_all(&frame).await?;
    stream.flush().await?;

    let mut header = [0u8; HEADER_LEN];
    stream.read_exact(&mut header).await?;
    let addr_len = decode_header(&header)?;

    let mut address = vec![0u8; addr_len];
    stream.read_exact(&mut address).await?;
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let address = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let frame = encode_hello(&address);
        let addr_len = decode_header(&frame[..HEADER_LEN]).expect("valid header");
        assert_eq!(addr_len, address.len());
        assert_eq!(&frame[HEADER_LEN..], &address[..]);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut frame = encode_hello(&[9, 9, 9]);
        frame[0] = b'X';
        assert!(decode_header(&frame[..HEADER_LEN]).is_err());
    }

    #[test]
    fn rejects_bad_version() {
        let mut frame = encode_hello(&[9, 9, 9]);
        frame[4] = 99;
        assert!(decode_header(&frame[..HEADER_LEN]).is_err());
    }

    #[test]
    fn rejects_zero_and_oversized_length() {
        let zero = {
            let mut f = encode_hello(&[1]);
            f[5..9].copy_from_slice(&0u32.to_le_bytes());
            f
        };
        assert!(decode_header(&zero[..HEADER_LEN]).is_err());

        let huge = {
            let mut f = encode_hello(&[1]);
            f[5..9].copy_from_slice(&(MAX_ADDR_LEN as u32 + 1).to_le_bytes());
            f
        };
        assert!(decode_header(&huge[..HEADER_LEN]).is_err());
    }

    #[test]
    fn rejects_short_header() {
        assert!(decode_header(&[0u8; HEADER_LEN - 1]).is_err());
    }
}
