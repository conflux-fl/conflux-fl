//! Shared protobuf schema — network AND local IPC.
//!
//! See `docs/spec/conflux-spec-v1.md` §2–§3.

tonic::include_proto!("conflux.v1");

/// The wire convention every `ClientDelta.weights`/`TaskResponse.model_weights`
/// buffer uses: a flat little-endian `f32` array, no header. Shared here so
/// every crate that touches these bytes (`conflux-core`, `conflux-server`)
/// calls one implementation instead of each carrying its own copy.
#[derive(Debug, thiserror::Error)]
pub enum WeightsCodecError {
    #[error("weights buffer has {len} bytes, which is not a multiple of 4")]
    Malformed { len: usize },
}

pub fn encode_weights(weights: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(weights.len() * 4);
    for w in weights {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    bytes
}

pub fn decode_weights(bytes: &[u8]) -> Result<Vec<f32>, WeightsCodecError> {
    if !bytes.len().is_multiple_of(4) {
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
}
