//! Length-delimited JSON codec for QUIC streams.
//!
//! Wire format: 4-byte big-endian length prefix + serde JSON payload.

use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

use crate::messages::Message;
use crate::ProtocolError;

/// Maximum message size: 16 MB (generous for batch fetches with large blobs).
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Length prefix size in bytes.
const LENGTH_PREFIX_SIZE: usize = 4;

/// Codec for framing Message values over a byte stream.
pub struct MessageCodec;

impl Decoder for MessageCodec {
    type Item = Message;
    type Error = ProtocolError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // Need at least the length prefix
        if src.len() < LENGTH_PREFIX_SIZE {
            return Ok(None);
        }

        // Peek at the length
        let length = u32::from_be_bytes([src[0], src[1], src[2], src[3]]) as usize;

        if length > MAX_MESSAGE_SIZE {
            return Err(ProtocolError::MessageTooLarge {
                size: length,
                max: MAX_MESSAGE_SIZE,
            });
        }

        // Check if we have the full message
        let total = LENGTH_PREFIX_SIZE + length;
        if src.len() < total {
            // Reserve space for the rest
            src.reserve(total - src.len());
            return Ok(None);
        }

        // Consume the length prefix
        src.advance(LENGTH_PREFIX_SIZE);

        // Take the message bytes
        let msg_bytes = src.split_to(length);

        // Deserialize
        let message: Message = serde_json::from_slice(&msg_bytes)?;
        Ok(Some(message))
    }
}

impl Encoder<Message> for MessageCodec {
    type Error = ProtocolError;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let payload = serde_json::to_vec(&item)?;

        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(ProtocolError::MessageTooLarge {
                size: payload.len(),
                max: MAX_MESSAGE_SIZE,
            });
        }

        // Write length prefix + payload
        dst.reserve(LENGTH_PREFIX_SIZE + payload.len());
        dst.put_u32(payload.len() as u32);
        dst.extend_from_slice(&payload);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::Ping;

    #[test]
    fn test_encode_decode_roundtrip() {
        let mut codec = MessageCodec;
        let msg = Message::Ping(Ping {
            seq: 42,
            sent_at_ns: 1234567890,
        });

        let mut buf = BytesMut::new();
        codec.encode(msg.clone(), &mut buf).unwrap();

        // Should have length prefix + JSON
        assert!(buf.len() > 4);

        let decoded = codec.decode(&mut buf).unwrap().unwrap();
        match decoded {
            Message::Ping(p) => {
                assert_eq!(p.seq, 42);
                assert_eq!(p.sent_at_ns, 1234567890);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_partial_message() {
        let mut codec = MessageCodec;
        let msg = Message::Ping(Ping {
            seq: 1,
            sent_at_ns: 0,
        });

        let mut buf = BytesMut::new();
        codec.encode(msg, &mut buf).unwrap();

        // Give only half the bytes
        let half = buf.len() / 2;
        let mut partial = buf.split_to(half);

        assert!(codec.decode(&mut partial).unwrap().is_none());
    }

    #[test]
    fn test_multiple_messages() {
        let mut codec = MessageCodec;
        let mut buf = BytesMut::new();

        for i in 0..5u64 {
            let msg = Message::Ping(Ping {
                seq: i,
                sent_at_ns: i * 100,
            });
            codec.encode(msg, &mut buf).unwrap();
        }

        for i in 0..5u64 {
            let decoded = codec.decode(&mut buf).unwrap().unwrap();
            match decoded {
                Message::Ping(p) => assert_eq!(p.seq, i),
                _ => panic!("wrong variant"),
            }
        }

        assert!(codec.decode(&mut buf).unwrap().is_none());
    }

    #[test]
    fn test_oversized_message_rejected() {
        let mut codec = MessageCodec;
        let mut buf = BytesMut::new();

        // Write a length prefix claiming a huge message
        buf.put_u32((MAX_MESSAGE_SIZE + 1) as u32);
        buf.extend_from_slice(&[0u8; 100]);

        let result = codec.decode(&mut buf);
        assert!(result.is_err());
    }
}
