use devo_protocol::AcpMeta;
use devo_protocol::DEVO_EXTENSION_META;
use devo_protocol::DEVO_PROTOCOL_META;
use devo_protocol::DEVO_PROTOCOL_NATIVE;
use devo_protocol::devo_native_protocol_opted_in;
use devo_protocol::native::methods::method_names;
use devo_protocol::native::rpc_session::SessionInterruptParams;
use devo_protocol::native::rpc_session::SessionInterruptScope;

#[test]
fn native_protocol_marker_selects_native_surface() {
    assert_eq!(DEVO_PROTOCOL_NATIVE, "native");

    let meta = AcpMeta::from_iter([(
        DEVO_EXTENSION_META.to_string(),
        serde_json::json!({ DEVO_PROTOCOL_META: "native" }),
    )]);

    assert!(devo_native_protocol_opted_in(Some(&meta)));
}

#[test]
fn former_protocol_marker_does_not_select_native_surface() {
    let meta = AcpMeta::from_iter([(
        DEVO_EXTENSION_META.to_string(),
        serde_json::json!({ DEVO_PROTOCOL_META: "canonical" }),
    )]);

    assert!(!devo_native_protocol_opted_in(Some(&meta)));
}

#[test]
fn session_interrupt_scopes_round_trip_without_a_turn_id() {
    let cases = [
        SessionInterruptScope::Session {
            session_id: devo_protocol::native::ids::SessionId::new(),
        },
        SessionInterruptScope::Task {
            item_id: devo_protocol::native::ids::ItemId::from_string("task-1".to_string()),
        },
        SessionInterruptScope::Command {
            process_id: "user-shell-1".to_string(),
        },
    ];

    for scope in cases {
        let params = SessionInterruptParams { scope };
        let json = serde_json::to_value(&params).expect("serialize session interrupt params");
        assert!(
            json["scope"].get("sessionId").is_some()
                || json["scope"].get("itemId").is_some()
                || json["scope"].get("processId").is_some()
        );
        let restored: SessionInterruptParams =
            serde_json::from_value(json).expect("deserialize session interrupt params");
        assert_eq!(restored, params);
    }
}

#[test]
fn native_interrupt_replaces_removed_turn_interrupt() {
    let methods = method_names();
    assert!(methods.contains(&"session/interrupt"));
    assert!(!methods.contains(&"turn/interrupt"));
}
