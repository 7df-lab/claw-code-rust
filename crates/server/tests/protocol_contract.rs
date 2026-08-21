use std::collections::VecDeque;

use chrono::Utc;
use devo_core::{
    ItemId, SessionId, SessionRecord, SessionTitleFinalSource, SessionTitleState, TurnId,
    TurnRecord, TurnStatus,
};
use devo_protocol::{AcpClientCapabilities, AcpInitializeParams};
use devo_server::{
    ActiveTurnSteeringState, ApprovalDecisionValue, ApprovalRequestPayload, ApprovalResponseParams,
    ApprovalScopeValue, ClientRequest, DefaultProjection, EventContext, InputItem, ItemDeltaKind,
    ItemDeltaPayload, PendingServerRequestContext, ProtocolError, ProtocolErrorCode, ServerEvent,
    ServerRequestKind, SessionMetadata, SessionProjector, SessionRuntimeStatus, SteerInputRecord,
    TurnKind, TurnProjector,
};
use pretty_assertions::assert_eq;

#[test]
fn acp_initialize_params_accept_documented_minimal_shape() {
    let params: AcpInitializeParams =
        serde_json::from_value(serde_json::json!({ "protocolVersion": 1 }))
            .expect("deserialize ACP initialize params");

    assert_eq!(
        params,
        AcpInitializeParams {
            protocol_version: 1,
            client_capabilities: AcpClientCapabilities::default(),
            client_info: None,
            meta: None,
        }
    );
}

#[test]
fn approval_response_roundtrip() {
    let payload = ApprovalResponseParams {
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        approval_id: "approval-1".into(),
        decision: ApprovalDecisionValue::Approve,
        scope: ApprovalScopeValue::Session,
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: ApprovalResponseParams = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(payload, restored);
}

#[test]
fn event_context_keeps_correlation_ids() {
    let context = EventContext {
        session_id: SessionId::new(),
        turn_id: Some(TurnId::new()),
        item_id: None,
        seq: 7,
        item_seq: None,
    };

    assert_eq!(context.seq, 7);
    assert!(context.turn_id.is_some());
}

#[test]
fn input_item_serializes_tagged_shape() {
    let input = InputItem::Skill {
        name: "rust-docs".into(),
        path: std::path::PathBuf::from("/skills/rust/SKILL.md"),
    };

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"skill\""));
}

#[test]
fn protocol_error_uses_spec_code_strings() {
    let payload = ProtocolError {
        code: ProtocolErrorCode::NotInitialized,
        message: "handshake incomplete".into(),
        data: serde_json::json!({}),
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    assert!(json.contains("NotInitialized"));
}

#[test]
fn server_request_payload_roundtrip() {
    let payload = ApprovalRequestPayload {
        request: PendingServerRequestContext {
            request_id: "req-1".into(),
            request_kind: ServerRequestKind::ItemPermissionsRequestApproval,
            session_id: SessionId::new(),
            turn_id: Some(TurnId::new()),
            item_id: None,
        },
        approval_id: "approval-1".into(),
        action_summary: "run shell command".into(),
        justification: "writes files".into(),
        resource: Some("ShellExec".into()),
        available_scopes: vec!["once".into(), "turn".into(), "session".into()],
        path: None,
        host: None,
        target: Some("echo hi".into()),
        command_pattern: Some(vec!["echo".into(), "*".into()]),
        command_prefix: None,
    };

    let json = serde_json::to_string(&payload).expect("serialize");
    let restored: ApprovalRequestPayload = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(payload, restored);
}

#[test]
fn steering_state_preserves_queue_order() {
    let first = SteerInputRecord {
        item_id: ItemId::new(),
        received_at: Utc::now(),
        input: vec![InputItem::Text {
            text: "first".into(),
        }],
    };
    let second = SteerInputRecord {
        item_id: ItemId::new(),
        received_at: Utc::now(),
        input: vec![InputItem::Text {
            text: "second".into(),
        }],
    };

    let state = ActiveTurnSteeringState {
        turn_id: TurnId::new(),
        turn_kind: TurnKind::Regular,
        pending_inputs: VecDeque::from([first.clone(), second.clone()]),
    };

    assert_eq!(state.pending_inputs[0], first);
    assert_eq!(state.pending_inputs[1], second);
}

#[test]
fn session_projection_maps_core_record() {
    let projection = DefaultProjection;
    let session = SessionRecord {
        id: SessionId::new(),
        rollout_path: "rollout.jsonl".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_activity_at: Some(Utc::now()),
        source: "api".into(),
        agent_nickname: None,
        agent_role: None,
        agent_path: None,
        model_provider: "anthropic".into(),
        model: Some("claude-sonnet".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        cwd: ".".into(),
        additional_directories: Vec::new(),
        cli_version: "0.1.0".into(),
        title: Some("Test".into()),
        title_state: SessionTitleState::Final(SessionTitleFinalSource::ExplicitCreate),
        sandbox_policy: "workspace-write".into(),
        approval_mode: "never".into(),
        effective_context_window: None,
        permission_preset: None,
        tokens_used: 0,
        first_user_message: None,
        archived_at: None,
        git_sha: None,
        git_branch: None,
        git_origin_url: None,
        parent_session_id: None,
        session_context: None,
        latest_turn_context: None,
        collaboration_mode: None,
        schema_version: 2,
    };

    let projected = projection.project_session(&session, false, SessionRuntimeStatus::Idle);
    assert_eq!(projected.session_id, session.id);
    assert_eq!(projected.model, session.model);
}

#[test]
fn turn_projection_preserves_turn_status_vocabulary() {
    let projection = DefaultProjection;
    let turn = TurnRecord {
        id: TurnId::new(),
        session_id: SessionId::new(),
        sequence: 1,
        started_at: Utc::now(),
        completed_at: None,
        status: TurnStatus::Running,
        kind: devo_core::TurnKind::Regular,
        model: "claude-sonnet".into(),
        model_binding_id: None,
        reasoning_effort_selection: None,
        request_model: "claude-sonnet".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: None,
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 2,
    };

    let projected = projection.project_turn(&turn);
    assert_eq!(projected.status, TurnStatus::Running);
}

#[test]
fn event_enum_carries_delta_kind() {
    let event = ServerEvent::ItemDelta {
        delta_kind: ItemDeltaKind::AgentMessageDelta,
        payload: ItemDeltaPayload {
            context: EventContext {
                session_id: SessionId::new(),
                turn_id: Some(TurnId::new()),
                item_id: Some(ItemId::new()),
                seq: 5,
                item_seq: None,
            },
            delta: "hi".into(),
            stream_index: None,
            channel: None,
            chunk_index: None,
        },
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("agent_message_delta"));
}

#[test]
fn request_envelope_keeps_method_and_id() {
    let request = ClientRequest {
        id: serde_json::json!(1),
        method: "session/start".into(),
        params: serde_json::json!({"cwd":"C:/repo"}),
    };

    let json = serde_json::to_string(&request).expect("serialize");
    assert!(json.contains("\"method\":\"session/start\""));
    assert!(json.contains("\"id\":1"));
}

#[test]
fn session_title_updated_event_serializes_expected_kind() {
    let event = ServerEvent::SessionTitleUpdated(devo_server::SessionEventPayload {
        session: SessionMetadata {
            session_id: SessionId::new(),
            cwd: ".".into(),
            additional_directories: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_activity_at: Utc::now(),
            title: Some("Renamed session".into()),
            title_state: SessionTitleState::Final(SessionTitleFinalSource::UserRename),
            parent_session_id: None,
            agent_path: None,
            agent_nickname: None,
            agent_role: None,
            ephemeral: false,
            model: Some("claude-sonnet".into()),
            model_binding_id: None,
            reasoning_effort_selection: None,
            reasoning_effort: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_tokens: 0,
            total_cache_creation_tokens: 0,
            total_cache_read_tokens: 0,
            prompt_token_estimate: 0,
            last_query_usage: None,
            last_query_total_tokens: 0,
            last_context_occupancy: None,
            status: SessionRuntimeStatus::Idle,
            collaboration_mode: Default::default(),
            effective_context_window: None,
            permission_preset: None,
        },
    });

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("session_title_updated"));
}

#[test]
fn session_compaction_events_serialize_expected_kinds() {
    let metadata = SessionMetadata {
        session_id: SessionId::new(),
        cwd: ".".into(),
        additional_directories: Vec::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_activity_at: Utc::now(),
        title: Some("Compacting session".into()),
        title_state: SessionTitleState::Unset,
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: false,
        model: Some("claude-sonnet".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        last_context_occupancy: None,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
        effective_context_window: None,
        permission_preset: None,
    };

    let started =
        ServerEvent::SessionCompactionStarted(devo_server::SessionCompactionStartedPayload {
            session: metadata.clone(),
            turn_id: TurnId::new(),
            trigger: devo_protocol::native::item::CompactionTrigger::Manual,
        });
    let completed =
        ServerEvent::SessionCompactionCompleted(devo_server::SessionCompactionCompletedPayload {
            session: metadata,
            turn_id: TurnId::new(),
            item_id: None,
        });
    let failed =
        ServerEvent::SessionCompactionFailed(devo_server::SessionCompactionFailedPayload {
            session_id: SessionId::new(),
            message: "boom".into(),
        });

    assert!(
        serde_json::to_string(&started)
            .expect("serialize")
            .contains("session_compaction_started")
    );
    assert!(
        serde_json::to_string(&completed)
            .expect("serialize")
            .contains("session_compaction_completed")
    );
    assert!(
        serde_json::to_string(&failed)
            .expect("serialize")
            .contains("session_compaction_failed")
    );
}

/// Trace: L2-DES-APP-009
/// Verifies: emit-site-enriched compaction lifecycle events project to
/// canonical context/compactionStarted and context/compactionCompleted
/// (item-linked), while a completion without a persisted item stays legacy.
#[test]
fn compaction_lifecycle_events_project_to_native_notifications() {
    let metadata = SessionMetadata {
        session_id: SessionId::new(),
        cwd: ".".into(),
        additional_directories: Vec::new(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_activity_at: Utc::now(),
        title: Some("Compacting session".into()),
        title_state: SessionTitleState::Unset,
        parent_session_id: None,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
        ephemeral: false,
        model: Some("claude-sonnet".into()),
        model_binding_id: None,
        reasoning_effort_selection: None,
        reasoning_effort: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_tokens: 0,
        total_cache_creation_tokens: 0,
        total_cache_read_tokens: 0,
        prompt_token_estimate: 0,
        last_query_usage: None,
        last_query_total_tokens: 0,
        last_context_occupancy: None,
        status: SessionRuntimeStatus::Idle,
        collaboration_mode: Default::default(),
        effective_context_window: None,
        permission_preset: None,
    };

    let turn_id = TurnId::new();
    let (method, value) =
        devo_protocol::native::wire_projector::typed_item_notification_from_server_event(
            &ServerEvent::SessionCompactionStarted(devo_server::SessionCompactionStartedPayload {
                session: metadata.clone(),
                turn_id,
                trigger: devo_protocol::native::item::CompactionTrigger::Manual,
            }),
        )
        .expect("compaction started projects");
    assert_eq!(method, "context/compactionStarted");
    assert_eq!(value["trigger"].as_str(), Some("manual"));
    assert_eq!(value["turnId"].as_str(), Some(turn_id.to_string().as_str()));

    let item_id = ItemId::new();
    let (method, value) =
        devo_protocol::native::wire_projector::typed_item_notification_from_server_event(
            &ServerEvent::SessionCompactionCompleted(
                devo_server::SessionCompactionCompletedPayload {
                    session: metadata.clone(),
                    turn_id,
                    item_id: Some(item_id),
                },
            ),
        )
        .expect("compaction completed with item projects");
    assert_eq!(method, "context/compactionCompleted");
    assert_eq!(value["itemId"].as_str(), Some(item_id.to_string().as_str()));

    assert!(
        devo_protocol::native::wire_projector::typed_item_notification_from_server_event(
            &ServerEvent::SessionCompactionCompleted(
                devo_server::SessionCompactionCompletedPayload {
                    session: metadata,
                    turn_id,
                    item_id: None,
                },
            ),
        )
        .is_none(),
        "completion without a persisted item must stay on the legacy path"
    );
}

/// Trace: L2-DES-APP-008, L2-DES-CONV-002
/// Verifies: the canonical session/metadata/update contract shape (patch
/// payload with SessionSettings + expectedVersion) round-trips on the wire.
#[test]
fn native_session_metadata_update_params_roundtrip() {
    use devo_protocol::native::model::PermissionProfile;
    use devo_protocol::native::rpc_session::SessionMetadataUpdateParams;
    use devo_protocol::native::session::SessionSettings;

    let params: SessionMetadataUpdateParams = serde_json::from_value(serde_json::json!({
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "expectedVersion": 3,
        "settings": {
            "permissionProfile": "fullAccess",
            "sandboxProfile": "workspace",
            "reasoningEffort": "high"
        }
    }))
    .expect("deserialize canonical params");
    assert_eq!(params.expected_version, 3);
    let settings = params.settings.clone().expect("settings present");
    assert_eq!(
        settings.permission_profile,
        Some(PermissionProfile::FullAccess)
    );
    assert_eq!(settings.sandbox_profile.as_deref(), Some("workspace"));
    assert_eq!(settings.reasoning_effort, Some("high".to_string()));
    let roundtripped: SessionMetadataUpdateParams =
        serde_json::from_value(serde_json::to_value(&params).expect("serialize canonical params"))
            .expect("re-deserialize canonical params");
    assert_eq!(roundtripped, params);

    // A minimal settings object defaults the unset fields.
    let minimal: SessionSettings = serde_json::from_value(serde_json::json!({
        "permissionProfile": "default"
    }))
    .expect("minimal settings");
    assert_eq!(minimal.permission_profile, PermissionProfile::Default);
    assert_eq!(minimal.sandbox_profile, None);
    assert_eq!(minimal.reasoning_effort, None);
    assert_eq!(minimal.mode, None);
    assert_eq!(minimal.effective_context_window, None);
}

/// Trace: L2-DES-CONV-002, L2-DES-APP-008
/// Verifies: the settings patch is partial (only present fields change) and
/// `expectedVersion: 0` is the documented no-precondition escape.
#[test]
fn native_session_settings_patch_is_partial() {
    use devo_protocol::native::rpc_session::SessionSettingsPatch;

    let patch: SessionSettingsPatch =
        serde_json::from_value(serde_json::json!({ "sandboxProfile": "strict" }))
            .expect("partial patch deserializes");
    assert_eq!(patch.permission_profile, None);
    assert_eq!(patch.sandbox_profile.as_deref(), Some("strict"));
    assert_eq!(patch.reasoning_effort, None);
    assert_eq!(patch.mode, None);
    assert_eq!(patch.effective_context_window, None);
    assert_eq!(
        serde_json::to_value(&patch).expect("serialize"),
        serde_json::json!({ "sandboxProfile": "strict" }),
        "absent fields stay absent on the wire"
    );
}

/// Trace: L2-DES-APP-008
/// Verifies: the canonical task domain wire shapes (task/start kind-tagged
/// params, task verb params) serialize as specified by DD-7.
#[test]
fn native_task_start_params_kind_tagged_wire_shape() {
    use devo_protocol::native::ids::SessionId;
    use devo_protocol::native::rpc_turn::TaskStartParams;

    let process = TaskStartParams::Process {
        session_id: SessionId::from_string("00000000-0000-0000-0000-000000000001".into()),
        command: "ls".into(),
        cwd: None,
        idempotency_key: "k-1".into(),
    };
    let value = serde_json::to_value(&process).expect("serialize process params");
    assert_eq!(
        value,
        serde_json::json!({
            "kind": "process",
            "sessionId": "00000000-0000-0000-0000-000000000001",
            "command": "ls",
            "idempotencyKey": "k-1"
        })
    );
    let roundtripped: TaskStartParams =
        serde_json::from_value(value).expect("deserialize process params");
    assert_eq!(roundtripped, process);

    let agent: TaskStartParams = serde_json::from_value(serde_json::json!({
        "kind": "agent",
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "input": [{ "type": "text", "text": "hi" }],
        "idempotencyKey": "k-2"
    }))
    .expect("deserialize agent params");
    assert!(matches!(agent, TaskStartParams::Agent { .. }));
}

/// Trace: L2-DES-APP-008
/// Verifies: the canonical goal domain wire shapes (goal/set with ifExists,
/// goal transition params) round-trip.
#[test]
fn native_goal_params_wire_shapes() {
    use devo_protocol::native::rpc_session::{
        GoalIfExists, SessionGoalSetParams, SessionGoalTransitionParams,
    };

    let set = SessionGoalSetParams {
        session_id: devo_protocol::native::ids::SessionId::from_string(
            "00000000-0000-0000-0000-000000000001".into(),
        ),
        objective: "ship it".into(),
        token_budget: Some(1000),
        if_exists: GoalIfExists::Replace,
        idempotency_key: "g-1".into(),
    };
    let value = serde_json::to_value(&set).expect("serialize goal/set params");
    assert_eq!(value["ifExists"], serde_json::json!("replace"));
    let roundtripped: SessionGoalSetParams =
        serde_json::from_value(value).expect("deserialize goal/set params");
    assert_eq!(roundtripped, set);

    let transition: SessionGoalTransitionParams = serde_json::from_value(serde_json::json!({
        "sessionId": "00000000-0000-0000-0000-000000000001",
        "expectedGoalId": "goal_00000000-0000-0000-0000-000000000002"
    }))
    .expect("deserialize transition params");
    assert_eq!(
        transition.expected_goal_id.as_str(),
        "goal_00000000-0000-0000-0000-000000000002"
    );
}
