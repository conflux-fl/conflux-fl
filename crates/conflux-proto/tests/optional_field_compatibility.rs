//! ADR 0012's two new `ClientDelta`/`DeltaChunk` fields, and the wire
//! compatibility claim that justified adding them.
//!
//! The ADR's Consequences section asserts that "no existing `ClientDelta`
//! producer needs to change — both new fields default to absent". That is
//! a claim about *bytes*, and it is the one worth testing: a deployed
//! `conflux-node` built before this change must keep interoperating with
//! a server built after it, in both directions, without either side
//! knowing the other's vintage.
//!
//! (It is not a claim about Rust source. Adding a field to a `prost`
//! struct breaks every literal that names its fields exhaustively, which
//! is why 75 of them across this workspace now end in
//! `..Default::default()`. Worth stating plainly, because the ADR's
//! wording does not distinguish the two and the distinction cost a
//! workspace-wide edit.
//!
//! That idiom was paid for and then tested: adding a *third* optional
//! field — `local_loss`, for q-FedAvg — broke exactly one literal in the
//! whole workspace, the one below that deliberately names every field
//! because its whole job is to notice when the schema grows.)

use conflux_proto::{ClientDelta, DeltaChunk, decode_weights, encode_weights};
use prost::Message;

/// A `ClientDelta` exactly as a pre-ADR-0012 client would build it.
fn legacy_delta() -> ClientDelta {
    ClientDelta {
        client_id: "legacy-node".to_string(),
        round: 3,
        weights: encode_weights(&[1.0, 2.0, 3.0]),
        num_samples: 128,
        ..Default::default()
    }
}

#[test]
fn a_delta_with_neither_new_field_encodes_to_the_pre_adr_bytes() {
    // The compatibility claim, stated as bytes. proto3 `optional` fields
    // that are absent emit nothing at all — no tag, no length, no zero —
    // so a message carrying neither is byte-for-byte what the old schema
    // produced. If this ever fails, every deployed client predating the
    // change is talking a different protocol than it thinks.
    let encoded = legacy_delta().encode_to_vec();

    // Hand-built from the old schema: field 1 (client_id, string),
    // field 2 (round, varint), field 3 (weights, bytes), field 4
    // (num_samples, varint). Spelled out rather than derived from the
    // current type, because deriving it from the thing under test would
    // make this assertion vacuous.
    let mut expected = Vec::new();
    expected.push(0x0a); // field 1, wire type 2 (length-delimited)
    expected.push(11); // "legacy-node".len()
    expected.extend_from_slice(b"legacy-node");
    expected.push(0x10); // field 2, wire type 0 (varint)
    expected.push(3); // round = 3
    expected.push(0x1a); // field 3, wire type 2
    expected.push(12); // 3 f32s
    expected.extend_from_slice(&encode_weights(&[1.0, 2.0, 3.0]));
    expected.push(0x20); // field 4, wire type 0
    expected.push(128); // num_samples = 128, varint continuation
    expected.push(1);

    assert_eq!(
        encoded, expected,
        "a delta with both new fields absent must be byte-identical to \
         what the pre-ADR-0012 schema produced"
    );
}

#[test]
fn absent_is_distinguishable_from_zero_and_empty() {
    // The whole reason both fields are `optional` rather than plain
    // scalars. "This client is not running FedNova" and "this client took
    // zero local steps" are different facts, and a plain `uint32` cannot
    // tell them apart — proto3 would encode both as nothing.
    let absent = ClientDelta {
        local_steps: None,
        control_variate: None,
        ..legacy_delta()
    };
    let zero = ClientDelta {
        local_steps: Some(0),
        control_variate: Some(Vec::new()),
        ..legacy_delta()
    };

    assert_ne!(
        absent.encode_to_vec(),
        zero.encode_to_vec(),
        "explicit presence must survive the wire, or the distinction the \
         fields were made optional for does not exist"
    );

    let decoded = ClientDelta::decode(zero.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.local_steps, Some(0));
    assert_eq!(decoded.control_variate, Some(Vec::new()));
}

#[test]
fn a_new_server_reads_an_old_clients_bytes() {
    // Forward direction: old client -> new server.
    let on_the_wire = legacy_delta().encode_to_vec();
    let decoded = ClientDelta::decode(on_the_wire.as_slice()).expect("old bytes must decode");

    assert_eq!(decoded.client_id, "legacy-node");
    assert_eq!(decoded.num_samples, 128);
    assert_eq!(
        decode_weights(&decoded.weights).unwrap(),
        vec![1.0, 2.0, 3.0]
    );
    // Absent, not defaulted to something that looks like a real answer.
    assert_eq!(decoded.local_steps, None);
    assert_eq!(decoded.control_variate, None);
}

#[test]
fn an_old_client_reads_a_new_servers_bytes() {
    // Reverse direction, which is the one that actually bites in a
    // rolling deployment: a server upgraded first sends a message
    // carrying fields the old client has never heard of. proto3 requires
    // unknown fields to be skipped, not to be an error.
    //
    // Simulated by decoding a fully-populated message and confirming the
    // fields the old schema *does* know survive intact — an old decoder
    // reads exactly those tags and steps over 7 and 8.
    let modern = ClientDelta {
        local_steps: Some(42),
        control_variate: Some(encode_weights(&[0.1, 0.2, 0.3])),
        ..legacy_delta()
    };

    let bytes = modern.encode_to_vec();
    assert!(
        bytes.len() > legacy_delta().encode_to_vec().len(),
        "the populated message should carry more than the legacy one"
    );

    let decoded = ClientDelta::decode(bytes.as_slice()).unwrap();
    assert_eq!(decoded.client_id, "legacy-node");
    assert_eq!(decoded.round, 3);
    assert_eq!(decoded.num_samples, 128);
    assert_eq!(
        decode_weights(&decoded.weights).unwrap(),
        vec![1.0, 2.0, 3.0]
    );
}

#[test]
fn the_control_variate_uses_the_same_codec_as_weights() {
    // ADR 0012: "same encoding as `weights`", so no second codec exists.
    // Asserting it because a control variate that needed its own encoder
    // would be a quietly different design than the one decided.
    let variate = [0.5_f32, -1.5, 2.25];
    let delta = ClientDelta {
        control_variate: Some(encode_weights(&variate)),
        ..legacy_delta()
    };

    let decoded = ClientDelta::decode(delta.encode_to_vec().as_slice()).unwrap();
    let recovered = decode_weights(&decoded.control_variate.unwrap()).unwrap();
    assert_eq!(recovered, variate);
}

#[test]
fn delta_chunk_carries_both_fields_too() {
    // The correction to ADR 0012's own snippet: it adds the fields to
    // `ClientDelta` only, which is the one message that never travels.
    // Without them here, no client could populate either field at all.
    let chunk = DeltaChunk {
        client_id: "node-1".to_string(),
        round: 1,
        chunk_index: 0,
        total_chunks: 1,
        data: encode_weights(&[1.0, 2.0]),
        num_samples: 10,
        local_steps: Some(7),
        control_variate: Some(encode_weights(&[0.1, 0.2])),
        local_loss: Some(0.42),
    };

    let decoded = DeltaChunk::decode(chunk.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.local_steps, Some(7));
    assert_eq!(
        decode_weights(&decoded.control_variate.unwrap()).unwrap(),
        vec![0.1, 0.2]
    );

    // And a chunk from an old client still round-trips unchanged.
    let legacy_chunk = DeltaChunk {
        client_id: "node-1".to_string(),
        round: 1,
        chunk_index: 0,
        total_chunks: 1,
        data: encode_weights(&[1.0, 2.0]),
        num_samples: 10,
        ..Default::default()
    };
    let decoded = DeltaChunk::decode(legacy_chunk.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.local_steps, None);
    assert_eq!(decoded.control_variate, None);
}
