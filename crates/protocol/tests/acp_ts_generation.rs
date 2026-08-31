#[test]
fn generated_acp_typescript_contains_wire_discriminants_and_names() {
    let output = devo_protocol::acp_ts::generate_acp_typescript();

    assert!(output.contains("export type JsonValue"));
    assert!(output.contains("export type AcpSessionNotification"));
    assert!(output.contains("sessionId"));
    assert!(output.contains("\"sessionUpdate\": \"agent_message_chunk\""));
    assert!(output.contains("\"sessionUpdate\": \"config_option_update\""));
    assert!(output.contains("export type AcpContentBlock"));
    assert!(output.contains("\"type\": \"text\""));
    assert!(output.contains("currentValue"));
    assert!(!output.contains("current_value"));
    assert!(output.contains("export type AcpRequestPermissionParams"));
    assert!(output.contains("export type AcpRequestPermissionResponse"));
    assert!(output.contains("export type AcpFsReadTextFileParams"));
    assert!(output.contains("export type AcpFsReadTextFileResult"));
    assert!(output.contains("export type AcpFsWriteTextFileParams"));
    assert!(output.contains("export type RequestUserInputResponse"));
    assert!(output.contains("switch_mode"));
    assert!(output.contains("type: AcpSetConfigOptionValueType"));
    assert!(output.contains(
        "export type AcpMcpServer = AcpMcpServerHttp | AcpMcpServerSse | AcpMcpServerStdio"
    ));
    assert!(!output.contains("AcpUnsupportedMcpServer"));
    assert!(!output.contains("AcpAuthMethodType"));
}

#[test]
fn generated_protocol_typescript_contains_non_acp_client_method_roots() {
    let output = devo_protocol::acp_ts::generate_protocol_typescript();

    assert!(output.contains("export type SkillListParams"));
    assert!(output.contains("export type SkillListResult"));
    assert!(output.contains("export type CommandExecParams"));
    assert!(output.contains("export type SubscriptionCreateParams"));
    assert!(output.contains("export type SearchStartParams"));
    assert!(output.contains("session_id"));
    assert!(output.contains("searchId"));
}

#[test]
fn generated_protocol_schema_contains_method_bindings() {
    let output = devo_protocol::acp_ts::generate_protocol_schema_json();
    let value: serde_json::Value = serde_json::from_str(&output).expect("schema JSON");

    assert_eq!(
        value["methods"]["session/update"]["incomingNotification"],
        "AcpSessionNotification"
    );
    assert_eq!(
        value["methods"]["session/prompt"]["outgoingRequest"],
        "AcpPromptParams"
    );
    assert!(value["methods"]["goal/status"].is_null());
    assert_eq!(
        value["methods"]["skill/list"]["incomingResult"],
        "SkillListResult"
    );
    assert!(value["schemas"]["AcpSessionNotification"].is_object());
    assert!(value["schemas"]["SkillListResult"].is_object());
    assert_eq!(
        value["methods"]["session/request_permission"]["incomingRequest"],
        "AcpRequestPermissionParams"
    );
    assert_eq!(
        value["methods"]["fs/read_text_file"]["incomingRequest"],
        "AcpFsReadTextFileParams"
    );
    assert_eq!(
        value["methods"]["fs/write_text_file"]["incomingRequest"],
        "AcpFsWriteTextFileParams"
    );
    assert_eq!(
        value["methods"]["session/cancel"]["outgoingNotification"],
        "AcpCancelParams"
    );
    assert_eq!(
        value["methods"]["logout"]["incomingResult"],
        "AcpLogoutResult"
    );
    assert_eq!(
        value["methods"]["session/new"]["outgoingRequest"],
        "SessionNewParams"
    );
    assert_eq!(
        value["methods"]["session/new"]["incomingResult"],
        "SessionNewResult"
    );
    assert_eq!(
        value["methods"]["userInput/request"]["incomingRequest"],
        "Item"
    );
    assert_eq!(
        value["methods"]["userInput/request"]["outgoingResponse"],
        "UserInputRespondParams"
    );
    assert_eq!(
        value["methods"]["approval/command/request"]["outgoingResponse"],
        "ApprovalRespondParams"
    );
    assert!(value["schemas"]["AcpRequestPermissionParams"].is_object());
    assert!(value["schemas"]["AcpFsReadTextFileParams"].is_object());
    assert!(value["schemas"]["AcpFsWriteTextFileParams"].is_object());
    assert!(
        value["schemas"]["AcpNewSessionParams"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|item| item == "mcpServers"))
    );
    assert!(
        value["schemas"]["AcpLoadSessionParams"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|item| item == "mcpServers"))
    );
    assert!(
        value["schemas"]["AcpResumeSessionParams"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|item| item == "mcpServers"))
    );
    assert!(
        value["schemas"]["AcpSetConfigOptionResult"]["required"]
            .as_array()
            .is_some_and(|required| required.iter().any(|item| item == "configOptions"))
    );
}

#[test]
fn generated_protocol_schema_covers_the_complete_native_registry() {
    let output = devo_protocol::acp_ts::generate_protocol_schema_json();
    let value: serde_json::Value = serde_json::from_str(&output).expect("schema JSON");

    for spec in devo_protocol::native::methods::NATIVE_METHODS {
        assert!(
            value["methods"][spec.name].is_object(),
            "missing Native method binding for {}",
            spec.name
        );
    }
    for spec in devo_protocol::native::methods::REVERSE_METHODS {
        assert_eq!(
            value["methods"][spec.name]["incomingRequest"], "Item",
            "wrong reverse request schema for {}",
            spec.name
        );
        assert!(
            value["methods"][spec.name]["outgoingResponse"].is_string(),
            "missing reverse response schema for {}",
            spec.name
        );
    }
    for method in ["session/created", "turn/completed", "item/started"] {
        assert!(
            value["methods"][method]["incomingNotification"].is_string(),
            "missing Native notification binding for {method}"
        );
    }
    let item_started_schema = value["methods"]["item/started"]["incomingNotification"]
        .as_str()
        .expect("item started schema name");
    assert!(value["schemas"][item_started_schema]["properties"]["item"].is_object());
}

#[test]
fn embedded_server_notification_fields_match_wire_casing() {
    let output = devo_protocol::acp_ts::generate_protocol_schema_json();
    let value: serde_json::Value = serde_json::from_str(&output).expect("schema JSON");

    // `ServerNotification` serializes variant fields with
    // `rename_all_fields = "camelCase"`, but schemars 0.8 ignores that
    // attribute. The generator must rewrite the definition embedded in
    // SubscriptionCreateResult (used to validate subscription replay), or
    // every multi-word variant field fails validation against snake_case.
    let branches =
        value["schemas"]["SubscriptionCreateResult"]["definitions"]["ServerNotification"]["oneOf"]
            .as_array()
            .expect("embedded ServerNotification oneOf");
    assert!(!branches.is_empty());
    for branch in branches {
        let params = &branch["properties"]["params"];
        let required = params["required"].as_array();
        for field in required.into_iter().flatten() {
            let field = field.as_str().expect("required field name");
            assert!(
                !field.contains('_'),
                "snake_case required field {field:?} in embedded ServerNotification"
            );
        }
        for field in params["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, _)| name)
        {
            assert!(
                !field.contains('_'),
                "snake_case property {field:?} in embedded ServerNotification"
            );
        }
    }

    let restore_started = branches
        .iter()
        .find(|branch| branch["properties"]["method"]["enum"][0] == "workspace/restoreStarted")
        .expect("workspace/restoreStarted branch");
    assert_eq!(
        restore_started["properties"]["params"]["required"],
        serde_json::json!(["restorePlanId", "sessionId"])
    );
}
