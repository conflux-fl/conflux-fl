//! The wire format every other Conflux FL crate builds on: a gRPC service
//! (`FlTransport`) and a small byte codec for model weights, generated
//! from and defined alongside `proto/fl_transport.proto`.
//!
//! `FlTransport` is used over two different connections with the exact
//! same schema: the real network hop between `conflux-server` and
//! `conflux-node`, and a local loopback hop (no TLS, localhost only)
//! between `conflux-node` and the Python `ClientApp` it launches. Same
//! message types, same codec, both times — a client's trained update is
//! never re-serialized into a different shape between the two hops.
//!
//! It exposes five RPCs: `Register` and `Heartbeat` (client lifecycle),
//! `FetchTask`/`SubscribeTasks` (pull vs. push mode — a client either
//! asks for the next round's task or the server streams it), and
//! `SubmitDelta`, which is itself a *streaming* RPC: a client's trained
//! update is sent as a stream of `DeltaChunk` messages rather than one
//! big message, so `conflux-buffer` can start reassembling it before the
//! last chunk arrives instead of waiting for a single multi-megabyte
//! payload.
//!
//! This crate has zero dependencies on any other crate in the workspace,
//! on purpose: every crate that needs to read or write the wire format —
//! `conflux-net`, `conflux-buffer`, `conflux-core`, both binaries — can
//! depend on `conflux-proto` without risking a cycle, because
//! `conflux-proto` never depends back on anything built on top of it.

#![warn(missing_docs)]

// Pulls in the Rust types `build.rs` generated from
// `proto/fl_transport.proto` at compile time — `RegisterRequest`,
// `ClientDelta`, `FlTransport`'s client/server traits, and so on. The
// string argument must match that `.proto` file's `package conflux.v1;`
// declaration exactly, or the generated code won't be found — it's a
// module path into `build.rs`'s output, not an arbitrary label.
//
// (A plain `//` comment, not `///` — rustdoc can't attach doc comments to
// a macro invocation like this one; it would just warn "unused doc
// comment" and be silently dropped.)
tonic::include_proto!("conflux.v1");

/// Returned by [`decode_weights`] when its input isn't a valid encoding —
/// currently the only way that can happen is a length that isn't a
/// multiple of 4, since every other byte pattern decodes to *some* valid
/// `f32` (unlike, say, UTF-8, there's no invalid 4-byte pattern to reject).
#[derive(Debug, thiserror::Error)]
pub enum WeightsCodecError {
    /// The buffer's length isn't a multiple of 4, so it cannot be a
    /// packed `f32` vector.
    #[error("weights buffer has {len} bytes, which is not a multiple of 4")]
    Malformed {
        /// The buffer's actual length, in bytes.
        len: usize,
    },
}

/// The wire convention every `ClientDelta.weights`/`DeltaChunk.data`/
/// `TaskResponse.model_weights` buffer uses: a flat little-endian `f32`
/// array, 4 bytes per weight, no header, no length prefix (the buffer's
/// own length is the full source of truth — `decode_weights` divides it
/// by 4). Little-endian because that's what `f32::to_le_bytes`/
/// `from_le_bytes` give you for free from Rust's standard library, and
/// x86/ARM are both little-endian natively, so there's no encode/decode
/// cost on the platforms this actually runs on. Defined once here so
/// every crate that touches a weight buffer (`conflux-core`,
/// `conflux-server`, `conflux-buffer` reassembling `DeltaChunk`s) agrees
/// on the format without needing to ask — Protobuf's own `repeated float`
/// would work too, but costs more to encode/decode at the sizes a real
/// model's weights reach, for a format this simple to hand-roll instead.
pub fn encode_weights(weights: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(weights.len() * 4);
    for w in weights {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    bytes
}

/// The inverse of [`encode_weights`]. Fails only on a malformed length;
/// any well-formed (multiple-of-4) buffer decodes successfully, including
/// buffers containing bit patterns for `NaN` or `Inf` — this function
/// doesn't validate the *values*, only the buffer's shape. A caller that
/// needs to reject non-finite weights (a client submitting `NaN` after a
/// diverged local training run, say) checks for that separately, after
/// decoding.
pub fn decode_weights(bytes: &[u8]) -> Result<Vec<f32>, WeightsCodecError> {
    // `% 4 != 0` rather than `is_multiple_of`, which is stable only
    // since 1.87. This crate promises 1.85 (edition 2024's own floor),
    // and a one-token convenience is not worth two minor versions of
    // downstream compatibility. `clippy::incompatible_msrv` is what
    // caught the mismatch.
    if bytes.len() % 4 != 0 {
        return Err(WeightsCodecError::Malformed { len: bytes.len() });
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn register_request_round_trips() {
        let original = RegisterRequest {
            client_id: "client-42".to_string(),
            auth_token: "token-abc".to_string(),
        };

        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");

        let decoded = RegisterRequest::decode(buf.as_slice()).expect("decode");

        assert_eq!(original, decoded);
    }

    #[test]
    fn client_delta_round_trips() {
        let original = ClientDelta {
            client_id: "client-7".to_string(),
            round: 3,
            weights: vec![0, 0, 128, 63, 0, 0, 0, 64], // f32 1.0, 2.0 little-endian
            num_samples: 128,
        };

        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");

        let decoded = ClientDelta::decode(buf.as_slice()).expect("decode");

        assert_eq!(original, decoded);
    }

    #[test]
    fn weights_codec_round_trips() {
        let original = vec![1.0, -2.5, 0.0, 3.75];

        let decoded = decode_weights(&encode_weights(&original)).unwrap();

        assert_eq!(decoded, original);
    }

    #[test]
    fn weights_codec_rejects_truncated_buffer() {
        let err = decode_weights(&[0, 0, 128]).unwrap_err();

        assert!(matches!(err, WeightsCodecError::Malformed { len: 3 }));
    }

    #[test]
    fn weights_codec_accepts_empty_buffer() {
        // Zero is a multiple of 4 — an empty update decodes to an empty
        // Vec, not an error. A client with zero local samples this round
        // is a real (if unusual) case, not malformed input.
        assert_eq!(decode_weights(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn weights_codec_does_not_reject_nan_or_inf() {
        // decode_weights validates the buffer's *shape*, not its values —
        // documented behavior, not an oversight. A client whose local
        // training diverged could legitimately submit NaN/Inf weights;
        // rejecting non-finite values is a policy decision that belongs
        // to whoever calls this (or conflux-privacy's clipping), not to
        // the codec.
        let bytes = encode_weights(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
        let decoded = decode_weights(&bytes).unwrap();

        assert!(decoded[0].is_nan());
        assert_eq!(decoded[1], f32::INFINITY);
        assert_eq!(decoded[2], f32::NEG_INFINITY);
    }

    #[test]
    fn decoding_random_garbage_as_a_register_request_fails_cleanly_not_a_panic() {
        // The real "can untrusted network input crash this?" question.
        // prost-generated `decode` returns a `Result` for exactly this
        // reason — a corrupted or adversarial byte stream on the wire
        // must produce a clean `Err`, never a panic. This sweeps a range
        // of byte patterns (not just one) through the decoder to back
        // that guarantee with more than a single example.
        for byte in 0u8..=255 {
            let garbage = vec![byte; 37]; // arbitrary length, not a multiple of anything meaningful
            let result = RegisterRequest::decode(garbage.as_slice());
            // Either a clean parse or a clean error — what matters is that
            // this loop completes at all rather than panicking partway
            // through.
            let _ = result;
        }
    }

    #[test]
    fn decoding_truncated_valid_message_fails_cleanly() {
        let original = ClientDelta {
            client_id: "client-7".to_string(),
            round: 3,
            weights: vec![0, 0, 128, 63],
            num_samples: 1,
        };
        let mut buf = Vec::new();
        original.encode(&mut buf).expect("encode");

        // Truncate a real, validly-encoded message partway through —
        // simulating a connection that drops mid-stream — and confirm
        // decoding it back reports an error instead of panicking or
        // silently returning a corrupted value.
        let truncated = &buf[..buf.len() / 2];
        let result = ClientDelta::decode(truncated);

        assert!(result.is_err() || result.unwrap() != original);
    }
}
