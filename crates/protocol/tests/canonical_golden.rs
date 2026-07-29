//! Golden JSON fixtures for the canonical wire contract (P0 gate, 01 §10.4):
//! the patch tri-state, legacy bare-UUID IDs and the unknown-item fallback
//! must keep stable JSON representations.

use devo_protocol::canonical::ids::SessionId;
use devo_protocol::canonical::item::Item;
use devo_protocol::canonical::item::ItemEnvelope;
use devo_protocol::canonical::item::ItemOrUnknown;
use devo_protocol::canonical::patch::PatchField;
use devo_protocol::canonical::rpc_session::SessionMetadataUpdateParams;
use pretty_assertions::assert_eq;

fn read_golden(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(path).expect("read golden fixture");
    serde_json::from_str(&text).expect("golden fixture is valid JSON")
}

#[test]
fn user_message_envelope_matches_golden_and_round_trips() {
    let golden = read_golden("item_user_message.json");
    let envelope: ItemEnvelope =
        serde_json::from_value(golden.clone()).expect("golden userMessage parses");
    // A legacy bare UUID session id round-trips unchanged.
    assert_eq!(
        envelope.session_id.as_str(),
        "019b1c2d-3e4f-7890-abcd-ef1234567890"
    );
    assert_eq!(
        serde_json::to_value(&envelope).expect("serialize"),
        golden,
        "serialization must be stable against the golden fixture"
    );
}

#[test]
fn approval_waiting_state_matches_golden_and_round_trips() {
    let golden = read_golden("item_approval_waiting.json");
    let envelope: ItemEnvelope =
        serde_json::from_value(golden.clone()).expect("golden approval parses");
    let Item::Approval { decision, .. } = &envelope.item else {
        panic!("expected approval item");
    };
    assert_eq!(*decision, None, "waiting state has no decision");
    assert_eq!(serde_json::to_value(&envelope).expect("serialize"), golden);
}

#[test]
fn unknown_future_variant_degrades_with_raw_preserved() {
    let golden = read_golden("item_unknown_future_variant.json");
    let decoded: ItemOrUnknown =
        serde_json::from_value(golden.clone()).expect("unknown item decodes");
    let ItemOrUnknown::Unknown(raw) = &decoded else {
        panic!("future variant must degrade to Unknown, got {decoded:?}");
    };
    assert_eq!(*raw, golden, "raw JSON is preserved verbatim");
    assert_eq!(decoded.raw(), golden);
}

#[test]
fn known_item_does_not_fall_into_unknown() {
    let golden = read_golden("item_user_message.json");
    let item_json = golden.get("item").cloned().expect("item payload");
    let decoded: ItemOrUnknown = serde_json::from_value(item_json).expect("decode");
    assert!(matches!(decoded, ItemOrUnknown::Known(_)));
}

#[test]
fn patch_field_null_is_explicit_clear() {
    let golden = read_golden("patch_title_null.json");
    let params: SessionMetadataUpdateParams = serde_json::from_value(golden).expect("params parse");
    assert_eq!(params.title, PatchField::Null);
}

#[test]
fn legacy_bare_uuid_id_round_trips() {
    let id: SessionId =
        serde_json::from_value(serde_json::json!("019b1c2d-3e4f-7890-abcd-ef1234567890"))
            .expect("legacy id parses");
    assert_eq!(id.as_str(), "019b1c2d-3e4f-7890-abcd-ef1234567890");
    assert_eq!(
        serde_json::to_value(&id).expect("serialize"),
        serde_json::json!("019b1c2d-3e4f-7890-abcd-ef1234567890")
    );
}
