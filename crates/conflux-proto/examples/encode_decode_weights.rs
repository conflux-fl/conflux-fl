//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-proto`](https://confluxfl.dev/crate-deep-dives/conflux-proto/).
//!
//! Run with:
//!   cargo run --example encode_decode_weights -p conflux-proto
//!
//! Shows both halves of what this crate owns: the hand-rolled byte codec
//! for a raw `Vec<f32>`, and the generated Protobuf types from
//! `proto/fl_transport.proto` (via `tonic::include_proto!`).

use conflux_proto::{ClientDelta, decode_weights, encode_weights};
use prost::Message;

fn main() {
    // 1. The weight codec: Vec<f32> -> Vec<u8> -> Vec<f32>.
    let weights = vec![1.0, -2.5, 0.001, 42.0];
    let bytes = encode_weights(&weights);
    let round_tripped = decode_weights(&bytes).expect("well-formed buffer");
    assert_eq!(weights, round_tripped);
    println!(
        "encode_weights: {} floats -> {} bytes -> decoded back to the same {} floats",
        weights.len(),
        bytes.len(),
        round_tripped.len()
    );

    // 2. A real generated type (ClientDelta), built and encoded/decoded
    // the way conflux-net actually does it — the weights *inside* the
    // message are the same little-endian bytes from step 1.
    let delta = ClientDelta {
        client_id: "client-7".to_string(),
        round: 3,
        weights: bytes,
        num_samples: 128,
    };

    let mut buf = Vec::new();
    delta.encode(&mut buf).expect("encode ClientDelta");
    println!(
        "ClientDelta for round {} from {:?}: {} bytes on the wire ({} of which are the weights payload)",
        delta.round,
        delta.client_id,
        buf.len(),
        delta.weights.len()
    );

    let decoded = ClientDelta::decode(buf.as_slice()).expect("decode ClientDelta");
    assert_eq!(decoded, delta);
    let recovered_weights = decode_weights(&decoded.weights).expect("well-formed buffer");
    println!(
        "round-tripped through Protobuf encode/decode -> recovered weights: {recovered_weights:?}"
    );
}
