//! Integration tests for the v1→v2 rollout migration: fixture files in the
//! frozen legacy format are read through the dual-format dispatch and
//! converted with `LegacyProjector`.
//!
//! The fixtures under `tests/fixtures/rollout_v1/` are generated from real
//! legacy `RolloutLine` values built in Rust (the builders below) and kept
//! permanently in the frozen legacy format (devo-api-design/05 §2.5). The
//! first run writes any missing file; afterwards the file must match the
//! builder byte-for-byte, so a drift failure means the legacy schema changed
//! (which it must never do) — regenerate by deleting the file.

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use devo_core::{
    ApprovalDecisionItem, ApprovalRequestItem, CollaborationMode, CommandExecutionItem,
    CompactionSnapshotLine, ContentPart, EditId, EditState, EnvironmentContext, ItemId, ItemLine,
    ItemRecord, LanguageContext, LegacyProjector, MessageEditRecordedLine,
    MessageEditRecordedRecord, Model, ParsedRolloutLine, Persona, RolloutLine, RolloutLineV2,
    SessionContext, SessionContextUpdatedLine, SessionId, SessionMetaLine, SessionRecord,
    SessionRollbackLine, SessionTitleFinalSource, SessionTitleState, SessionTitleUpdatedLine,
    SystemPromptMode, TextItem, ToolCallItem, ToolProgressItem, ToolResultItem, TurnContext,
    TurnError, TurnId, TurnItem, TurnKind, TurnLine, TurnRecord, TurnStatus, TurnUsage,
    WorkspaceRestorePolicy, parse_rollout_line,
};
use devo_protocol::native::ids::ItemId as CanonicalItemId;
use devo_protocol::native::item::{
    ApprovalDecisionKind, ApprovalScope, ApprovalTarget, ExecOrigin, ExecutionMode, Item,
    ItemState, ToolSource, UserInput, UserMessageEntry,
};
use devo_protocol::native::model::PermissionProfile;
use devo_protocol::native::session::SessionParent;
use devo_protocol::native::turn::{
    TurnKind as CanonicalTurnKind, TurnStatus as CanonicalTurnStatus,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;

// ── Deterministic fixture data ──────────────────────────────────────────

fn ts(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, second).unwrap()
}

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn session_id(n: u128) -> SessionId {
    SessionId::from(uuid(n))
}

fn turn_id(n: u128) -> TurnId {
    TurnId::from(uuid(n))
}

fn item_id(n: u128) -> ItemId {
    ItemId::from(uuid(n))
}

fn session_record(n: u128) -> SessionRecord {
    SessionRecord {
        id: session_id(n),
        rollout_path: "rollout.jsonl".into(),
        created_at: ts(0),
        updated_at: ts(1),
        last_activity_at: Some(ts(1)),
        source: "cli".into(),
        agent_nickname: None,
        agent_role: None,
        agent_path: None,
        model_provider: "openai".into(),
        model: Some("gpt-5.2".into()),
        model_binding_id: Some("binding-1".into()),
        reasoning_effort_selection: Some("high".into()),
        cwd: "/tmp/legacy-project".into(),
        additional_directories: vec!["/tmp/legacy-extra".into()],
        cli_version: "0.1.33".into(),
        title: Some("Legacy Session".into()),
        title_state: SessionTitleState::Final(SessionTitleFinalSource::ModelGenerated),
        sandbox_policy: "workspace-write".into(),
        approval_mode: "on-request".into(),
        effective_context_window: None,
        tokens_used: 12345,
        first_user_message: Some("Fix the flaky test".into()),
        archived_at: None,
        git_sha: Some("abc123".into()),
        git_branch: Some("main".into()),
        git_origin_url: Some("git@github.com:example/repo.git".into()),
        parent_session_id: None,
        session_context: None,
        latest_turn_context: None,
        collaboration_mode: None,
        permission_preset: None,
        schema_version: 2,
    }
}

fn turn_record(n: u128, session: u128) -> TurnRecord {
    TurnRecord {
        id: turn_id(n),
        session_id: session_id(session),
        sequence: 1,
        started_at: ts(2),
        completed_at: Some(ts(3)),
        status: TurnStatus::Completed,
        kind: TurnKind::Regular,
        model: "gpt-5.2".into(),
        model_binding_id: Some("binding-1".into()),
        reasoning_effort_selection: Some("high".into()),
        request_model: "gpt-5.2-codex".into(),
        request_thinking: None,
        input_token_estimate: None,
        usage: Some(TurnUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: Some(10),
            cache_read_input_tokens: Some(20),
            reasoning_output_tokens: Some(5),
            total_tokens: Some(150),
        }),
        latest_query_usage: None,
        context_occupancy: None,
        stop_reason: None,
        failure_reason: None,
        error: None,
        session_context: None,
        turn_context: None,
        schema_version: 4,
    }
}

fn item_record(n: u128, session: u128, turn: u128, seq: u64) -> ItemRecord {
    ItemRecord {
        id: item_id(n),
        session_id: session_id(session),
        turn_id: turn_id(turn),
        seq,
        timestamp: ts(10 + seq as u32),
        attempt_placement: None,
        turn_status: Some(TurnStatus::Running),
        sibling_turn_ids: Vec::new(),
        input_items: Vec::new(),
        output_items: Vec::new(),
        worklog: None,
        error: None,
        schema_version: 1,
    }
}

fn item_line(record: ItemRecord) -> RolloutLine {
    RolloutLine::Item(ItemLine {
        timestamp: record.timestamp,
        item: record,
    })
}

fn sample_session_context() -> SessionContext {
    SessionContext {
        base_instructions: "base".into(),
        available_skills: None,
        workspace_instructions: None,
        locked_agents_snapshot: None,
        environment: EnvironmentContext {
            cwd: ".".into(),
            shell: "bash".into(),
            current_date: "2026-07-01".into(),
            timezone: "UTC".into(),
        },
        language: LanguageContext::default(),
        persona: Persona::Default,
        model: Model {
            slug: "gpt-5.2".into(),
            ..Model::default()
        },
        reasoning_effort_selection: None,
        reasoning_effort: None,
        system_prompt_mode: SystemPromptMode::CodingAgent,
    }
}

// ── Fixture builders ────────────────────────────────────────────────────

/// A typical session: packed multi-payload records, tool call/result pair,
/// command execution, approval request+decision fold, steer input, web
/// search, plan, compaction, title update, snapshot, rollback.
fn basic_session_lines() -> Vec<RolloutLine> {
    let session = 0xb1;
    let turn = 0xb2;
    let mut conversation = item_record(0xb3, session, turn, 1);
    conversation.input_items = vec![TurnItem::UserMessage(TextItem {
        text: "Fix the flaky test".into(),
    })];
    conversation.output_items = vec![
        TurnItem::AgentMessage(TextItem {
            text: "On it.".into(),
        }),
        TurnItem::Plan(TextItem {
            text: "1. reproduce\n2. fix".into(),
        }),
    ];

    let mut tool_pair = item_record(0xb4, session, turn, 2);
    tool_pair.output_items = vec![
        TurnItem::ToolCall(ToolCallItem {
            tool_call_id: "call-1".into(),
            tool_name: "read_file".into(),
            input: serde_json::json!({"path": "src/lib.rs"}),
        }),
        TurnItem::ToolResult(ToolResultItem {
            tool_call_id: "call-1".into(),
            tool_name: Some("read_file".into()),
            output: serde_json::json!({"content": "fn main() {}"}),
            display_content: Some("fn main() {}".into()),
            is_error: false,
        }),
    ];

    let mut command = item_record(0xb5, session, turn, 3);
    command.output_items = vec![TurnItem::CommandExecution(CommandExecutionItem {
        tool_call_id: "call-2".into(),
        tool_name: "exec_command".into(),
        command: "cargo test".into(),
        input: serde_json::json!({"command": "cargo test"}),
        output: serde_json::json!({"stdout": "ok"}),
        is_error: false,
    })];

    let mut approval_request = item_record(0xb6, session, turn, 4);
    approval_request.output_items = vec![TurnItem::ApprovalRequest(ApprovalRequestItem {
        approval_id: "appr-1".into(),
        action_summary: "Run cargo test".into(),
        justification: "Need to verify the fix".into(),
        resource: Some("ShellExec".into()),
        available_scopes: vec!["Once".into(), "Session".into()],
        path: None,
        host: None,
        target: Some("cargo test".into()),
    })];

    let mut approval_decision = item_record(0xb7, session, turn, 5);
    approval_decision.output_items = vec![TurnItem::ApprovalDecision(ApprovalDecisionItem {
        approval_id: "appr-1".into(),
        // Pascal-case "Allow" appears in historical files (see records.rs
        // tests); it must still map to Approved.
        decision: "Allow".into(),
        scope: "Once".into(),
        decision_source: None,
    })];

    let mut steer = item_record(0xb8, session, turn, 6);
    steer.input_items = vec![TurnItem::SteerInput(TextItem {
        text: "also run clippy".into(),
    })];
    steer.output_items = vec![
        TurnItem::WebSearch(TextItem {
            text: "search results".into(),
        }),
        TurnItem::Reasoning(TextItem {
            text: "thinking...".into(),
        }),
    ];

    let mut compaction = item_record(0xb9, session, turn, 7);
    compaction.output_items = vec![TurnItem::ContextCompaction(TextItem {
        text: "compacted summary".into(),
    })];

    vec![
        RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: ts(0),
            session: session_record(session),
        })),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: ts(2),
            turn: turn_record(turn, session),
        })),
        item_line(conversation),
        item_line(tool_pair),
        item_line(command),
        item_line(approval_request),
        item_line(approval_decision),
        item_line(steer),
        item_line(compaction),
        RolloutLine::SessionTitleUpdated(SessionTitleUpdatedLine {
            timestamp: ts(30),
            session_id: session_id(session),
            title: "Fix flaky test".into(),
            title_state: SessionTitleState::Final(SessionTitleFinalSource::UserRename),
            previous_title: Some("Legacy Session".into()),
        }),
        RolloutLine::CompactionSnapshot(Box::new(CompactionSnapshotLine {
            timestamp: ts(31),
            session_id: session_id(session),
            turn_id: turn_id(turn),
            summary_item_id: item_id(0xb9),
            preserved_item_ids: vec![item_id(0xb3), item_id(0xb4)],
            context_occupancy: None,
        })),
        RolloutLine::SessionRollback(Box::new(SessionRollbackLine {
            timestamp: ts(32),
            session_id: session_id(session),
            retained_turn_ids: vec![turn_id(turn)],
            retained_item_ids: vec![item_id(0xb3)],
            latest_turn_id: Some(turn_id(turn)),
            schema_version: 1,
        })),
    ]
}

/// Internal (non-item) payloads, a failed compaction turn on a sub-agent
/// session, a message edit, and a session-context update.
fn internal_lines() -> Vec<RolloutLine> {
    let session = 0xc1;
    let turn = 0xc2;
    let mut record = session_record(session);
    record.parent_session_id = Some(session_id(0xc0));
    record.agent_role = Some("explorer".into());
    record.agent_nickname = Some("scout".into());
    record.approval_mode = "full-access".into();
    record.model = None;
    record.git_sha = None;
    record.git_branch = None;
    record.git_origin_url = None;
    record.first_user_message = None;
    record.last_activity_at = None;
    record.session_context = Some(sample_session_context());

    let mut turn = turn_record(turn, session);
    turn.kind = TurnKind::ManualCompaction;
    turn.status = TurnStatus::Failed;
    turn.completed_at = None;
    turn.usage = None;
    turn.error = Some(TurnError {
        code: "PROVIDER_SERVER_ERROR".into(),
        message: "provider request failed".into(),
        recovery_hint: Some("retry later".into()),
    });
    turn.session_context = Some(sample_session_context());
    turn.turn_context = Some(TurnContext {
        environment: EnvironmentContext {
            cwd: "/tmp/legacy-project".into(),
            shell: "bash".into(),
            current_date: "2026-07-01".into(),
            timezone: "UTC".into(),
        },
        persona: Persona::Default,
        model: Model {
            slug: "gpt-5.2".into(),
            ..Model::default()
        },
        reasoning_effort_selection: None,
        reasoning_effort: None,
        observed_agents_snapshot: None,
        collaboration_mode: CollaborationMode::default(),
    });
    turn.request_thinking = Some("enabled".into());
    turn.input_token_estimate = Some(42);
    turn.latest_query_usage = Some(TurnUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        reasoning_output_tokens: None,
        total_tokens: Some(15),
    });

    let mut internals = item_record(0xc3, 0xc1, 0xc2, 1);
    internals.input_items = vec![TurnItem::HookPrompt(TextItem {
        text: "hook text".into(),
    })];
    internals.output_items = vec![
        TurnItem::ToolProgress(ToolProgressItem {
            tool_call_id: "call-9".into(),
            message: "working".into(),
        }),
        TurnItem::TurnSummary(TextItem { text: "3".into() }),
    ];

    vec![
        RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: ts(0),
            session: record,
        })),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: ts(2),
            turn,
        })),
        item_line(internals),
        RolloutLine::MessageEditRecorded(Box::new(MessageEditRecordedLine {
            timestamp: ts(40),
            record: MessageEditRecordedRecord {
                schema_version: 1,
                session_id: session_id(0xc1),
                edit_id: EditId(uuid(0xc4)),
                target_message_id: item_id(0xc5),
                replacement_message_id: item_id(0xc6),
                target_turn_id: Some(turn_id(0xc2)),
                replacement_turn_id: None,
                queue_item_id: None,
                edited_content_parts: vec![ContentPart::Text("edited".into())],
                edited_mentions: Vec::new(),
                workspace_restore_policy: WorkspaceRestorePolicy::Skip,
                edit_state: EditState::Accepted,
                requested_by_client_id: None,
                created_at: ts(40),
            },
        })),
        RolloutLine::SessionContextUpdated(Box::new(SessionContextUpdatedLine {
            timestamp: ts(41),
            session_id: session_id(0xc1),
            session_context: sample_session_context(),
            schema_version: 1,
        })),
    ]
}

/// An orphan approval decision (no request in the file), a hosted image
/// generation, and a rollback that retains nothing.
fn orphan_decision_lines() -> Vec<RolloutLine> {
    let session = 0xd1;
    let turn = 0xd2;
    let mut record = session_record(session);
    record.approval_mode = "untrusted".into();

    let mut turn = turn_record(turn, session);
    turn.status = TurnStatus::WaitingApproval;
    turn.completed_at = None;

    let mut orphan = item_record(0xd3, 0xd1, 0xd2, 1);
    orphan.output_items = vec![TurnItem::ApprovalDecision(ApprovalDecisionItem {
        approval_id: "appr-orphan".into(),
        decision: "approve".into(),
        scope: "session".into(),
        decision_source: None,
    })];

    let mut image = item_record(0xd4, 0xd1, 0xd2, 2);
    image.output_items = vec![TurnItem::ImageGeneration(TextItem {
        text: "image result".into(),
    })];

    vec![
        RolloutLine::SessionMeta(Box::new(SessionMetaLine {
            timestamp: ts(0),
            session: record,
        })),
        RolloutLine::Turn(Box::new(TurnLine {
            timestamp: ts(2),
            turn,
        })),
        item_line(orphan),
        item_line(image),
        RolloutLine::SessionRollback(Box::new(SessionRollbackLine {
            timestamp: ts(50),
            session_id: session_id(0xd1),
            retained_turn_ids: Vec::new(),
            retained_item_ids: Vec::new(),
            latest_turn_id: None,
            schema_version: 1,
        })),
    ]
}

// ── Fixture loading + projection driver ─────────────────────────────────

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rollout_v1")
}

/// Returns the fixture content, writing it on first run and asserting the
/// on-disk file still matches the builder afterwards.
fn fixture_content(name: &str, lines: &[RolloutLine]) -> String {
    let path = fixture_dir().join(name);
    let mut expected = lines
        .iter()
        .map(|line| serde_json::to_string(line).expect("serialize legacy line"))
        .collect::<Vec<_>>()
        .join("\n");
    expected.push('\n');
    match fs::read_to_string(&path) {
        Ok(existing) => {
            // Fixtures are stored in the repo and may be checked out with
            // Windows CRLF line endings. Normalize before comparison.
            let existing = existing.replace("\r\n", "\n");
            assert_eq!(
                existing, expected,
                "fixture {name} drifted from its builder; the legacy schema must not change"
            );
            existing
        }
        Err(_) => {
            fs::create_dir_all(path.parent().expect("fixture dir parent")).expect("create dir");
            fs::write(&path, &expected).expect("write fixture");
            expected
        }
    }
}

/// Reads one fixture through the v-version dispatch and projects every line;
/// asserts that every line converts without error.
fn project_fixture(name: &str, lines: &[RolloutLine]) -> Vec<RolloutLineV2> {
    let content = fixture_content(name, lines);
    let mut projector = LegacyProjector::new();
    let mut out = Vec::new();
    for raw_line in content.lines() {
        match parse_rollout_line(raw_line).expect("fixture line parses") {
            ParsedRolloutLine::Legacy(line) => {
                out.extend(
                    projector
                        .project_line(&line)
                        .expect("legacy line projects without error"),
                );
            }
            ParsedRolloutLine::V2(_) => panic!("fixture {name} must contain only legacy lines"),
        }
    }
    out
}

/// The full v2 JSONL output re-parses as `RolloutLineV2` and deep-compares
/// equal to the projected values.
fn assert_v2_roundtrip(projected: &[RolloutLineV2]) {
    for line in projected {
        let raw = serde_json::to_string(line).expect("serialize v2 line");
        match parse_rollout_line(&raw).expect("v2 line re-parses") {
            ParsedRolloutLine::V2(parsed) => assert_eq!(parsed.as_ref(), line),
            ParsedRolloutLine::Legacy(_) => panic!("v2 line parsed as legacy"),
        }
    }
}

fn item_envelopes(lines: &[RolloutLineV2]) -> Vec<&devo_protocol::native::item::ItemEnvelope> {
    lines
        .iter()
        .filter_map(|line| match line {
            RolloutLineV2::Item { item, .. } => Some(item),
            _ => None,
        })
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────────

#[test]
fn basic_session_projects_all_lines_in_order() {
    let projected = project_fixture("basic_session.jsonl", &basic_session_lines());

    // 1 SessionMeta + 1 Turn + 12 items (3+2+1+1+1+3+1 payloads, approval
    // decision reuses the request's seq) + title + snapshot + rollback.
    assert_eq!(projected.len(), 17);

    assert!(
        matches!(&projected[0], RolloutLineV2::SessionMeta { session, .. }
            if session.cwd.as_os_str() == "/tmp/legacy-project")
    );
    assert!(matches!(&projected[1], RolloutLineV2::Turn { turn, .. }
            if turn.kind == CanonicalTurnKind::Regular
                && turn.status == CanonicalTurnStatus::Completed));

    let envelopes = item_envelopes(&projected);
    assert_eq!(envelopes.len(), 12);
    // First-appearance seqs are assigned in payload order; the folded
    // decision repeats the request's seq (7) with revision 2.
    let seqs: Vec<u64> = envelopes.iter().map(|envelope| envelope.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7, 7, 8, 9, 10, 11]);

    // The first payload of the packed conversation record keeps the legacy
    // record id; the sibling payloads get fresh bare-UUID ids (prefixed ids
    // could not round-trip into legacy UUID newtypes).
    assert_eq!(envelopes[0].id.as_str(), uuid(0xb3).to_string());
    assert!(uuid::Uuid::parse_str(envelopes[1].id.as_str()).is_ok());
    assert!(uuid::Uuid::parse_str(envelopes[2].id.as_str()).is_ok());

    assert!(
        matches!(&envelopes[0].item, Item::UserMessage { content, entry: UserMessageEntry::TurnStart, .. }
            if matches!(content.as_slice(), [UserInput::Text { text }] if text == "Fix the flaky test"))
    );
    assert!(
        matches!(&envelopes[1].item, Item::AssistantMessage { text, phase: None } if text == "On it.")
    );
    assert!(
        matches!(&envelopes[3].item, Item::ToolCall { call_id, tool_name, source: ToolSource::Builtin, .. }
            if call_id == "call-1" && tool_name == "read_file")
    );
    assert!(
        matches!(&envelopes[4].item, Item::ToolResult { call_id, is_error: false, truncated: false, .. }
            if call_id == "call-1")
    );
    // Command execution picks up the session cwd learned from SessionMeta.
    assert!(
        matches!(&envelopes[5].item, Item::CommandExecution { command, cwd, execution_mode: ExecutionMode::Foreground, origin: ExecOrigin::AgentTool, .. }
            if command == "cargo test" && cwd.as_os_str() == "/tmp/legacy-project")
    );
    assert!(matches!(
        &envelopes[8].item,
        Item::UserMessage {
            entry: UserMessageEntry::Steer,
            ..
        }
    ));
    assert!(
        matches!(&envelopes[9].item, Item::HostedToolCall { tool_name, .. } if tool_name == "web_search")
    );
    assert!(
        matches!(&envelopes[11].item, Item::ContextCompaction { summary: Some(summary), .. }
            if summary == "compacted summary")
    );

    assert_v2_roundtrip(&projected);
}

#[test]
fn approval_request_and_decision_fold_into_one_item() {
    let projected = project_fixture("basic_session.jsonl", &basic_session_lines());
    let envelopes = item_envelopes(&projected);
    let approvals: Vec<_> = envelopes
        .iter()
        .filter(|envelope| matches!(envelope.item, Item::Approval { .. }))
        .collect();

    assert_eq!(approvals.len(), 2);
    // One item id, one seq, revisions 1 then 2.
    assert_eq!(approvals[0].id, approvals[1].id);
    assert_eq!(approvals[0].seq, approvals[1].seq);
    assert_eq!((approvals[0].revision, approvals[1].revision), (1, 2));
    assert_eq!(approvals[0].state, ItemState::Waiting);
    assert_eq!(approvals[1].state, ItemState::Completed);

    let Item::Approval {
        approval_id,
        target,
        decision: None,
        ..
    } = &approvals[0].item
    else {
        panic!("first revision is the undecided request");
    };
    assert_eq!(approval_id, "appr-1");
    assert_eq!(
        target,
        &Some(ApprovalTarget::Command {
            command: "cargo test".into()
        })
    );

    let Item::Approval {
        decision: Some(decision),
        ..
    } = &approvals[1].item
    else {
        panic!("second revision carries the folded decision");
    };
    assert_eq!(decision.decision, ApprovalDecisionKind::Approved);
    assert_eq!(decision.scope, ApprovalScope::Once);
    assert_eq!(decision.decided_at, ts(15));
}

#[test]
fn legacy_bare_uuid_ids_round_trip_unchanged() {
    let projected = project_fixture("basic_session.jsonl", &basic_session_lines());

    let RolloutLineV2::SessionMeta { session, .. } = &projected[0] else {
        panic!("first line is the session meta");
    };
    assert_eq!(session.id.as_str(), uuid(0xb1).to_string());
    // Serializing the canonical id yields the identical bare-UUID string.
    assert_eq!(
        serde_json::to_string(&session.id).expect("serialize id"),
        format!("\"{}\"", uuid(0xb1))
    );

    let RolloutLineV2::Turn { turn, .. } = &projected[1] else {
        panic!("second line is the turn");
    };
    assert_eq!(turn.id.as_str(), uuid(0xb2).to_string());
    assert_eq!(turn.session_id.as_str(), uuid(0xb1).to_string());

    let RolloutLineV2::CompactionSnapshot {
        summary_item_id,
        preserved_item_ids,
        ..
    } = &projected[15]
    else {
        panic!("compaction snapshot line");
    };
    assert_eq!(summary_item_id.as_str(), uuid(0xb9).to_string());
    assert_eq!(
        preserved_item_ids
            .iter()
            .map(CanonicalItemId::as_str)
            .collect::<Vec<_>>(),
        vec![uuid(0xb3).to_string(), uuid(0xb4).to_string()]
    );
}

#[test]
fn internal_payloads_become_internal_lines_not_items() {
    let projected = project_fixture("internal_lines.jsonl", &internal_lines());

    assert_eq!(projected.len(), 7);
    assert!(item_envelopes(&projected).is_empty());

    use devo_core::InternalRecordV2;
    use devo_protocol::native::item::InternalEntry;

    let internal_entries: Vec<_> = projected
        .iter()
        .filter_map(|line| match line {
            RolloutLineV2::Internal { entry, .. } => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(internal_entries.len(), 5);
    assert!(
        matches!(&internal_entries[0], InternalRecordV2::Entry { entry: InternalEntry::HookPrompt { text } }
            if text == "hook text")
    );
    assert!(
        matches!(&internal_entries[1], InternalRecordV2::Entry { entry: InternalEntry::ToolProgress { call_id, message } }
            if call_id == "call-9" && message == "working")
    );
    assert!(
        matches!(&internal_entries[2], InternalRecordV2::Entry { entry: InternalEntry::TurnSummary { text } }
            if text == "3")
    );
    assert!(
        matches!(&internal_entries[3], InternalRecordV2::MessageEdit(record)
            if record.target_message_id == item_id(0xc5))
    );
    assert!(
        matches!(&internal_entries[4], InternalRecordV2::SessionContext(context)
            if context.base_instructions == "base")
    );

    assert_v2_roundtrip(&projected);
}

#[test]
fn subagent_session_and_failed_compaction_turn_project() {
    let projected = project_fixture("internal_lines.jsonl", &internal_lines());

    let RolloutLineV2::SessionMeta { session, .. } = &projected[0] else {
        panic!("first line is the session meta");
    };
    assert_eq!(
        session.parent,
        Some(SessionParent::Agent {
            session_id: devo_protocol::native::ids::SessionId::from_legacy_uuid(uuid(0xc0)),
            role: Some("explorer".into()),
        })
    );
    assert_eq!(
        session.settings.permission_profile,
        PermissionProfile::FullAccess
    );
    // No resolved model was recorded: the slug is explicitly empty.
    assert_eq!(session.model.model, "");
    assert!(session.git_info.is_none());

    let RolloutLineV2::Turn { turn, .. } = &projected[1] else {
        panic!("second line is the turn");
    };
    assert_eq!(turn.kind, CanonicalTurnKind::Compaction);
    assert_eq!(turn.status, CanonicalTurnStatus::Failed);
    let error = turn.error.as_ref().expect("failed turn carries an error");
    assert_eq!(error.error_code, "PROVIDER_SERVER_ERROR");
    assert_eq!(
        error.details,
        Some(serde_json::json!({ "recoveryHint": "retry later" }))
    );
}

#[test]
fn orphan_approval_decision_becomes_warning_item() {
    let projected = project_fixture("orphan_decision.jsonl", &orphan_decision_lines());

    assert_eq!(projected.len(), 5);
    let envelopes = item_envelopes(&projected);
    assert_eq!(envelopes.len(), 2);

    let warning = &envelopes[0];
    assert_eq!(warning.state, ItemState::Completed);
    assert!(
        matches!(&warning.item, Item::Warning { code, retryable: false, .. }
            if code == "legacyOrphanApprovalDecision")
    );
    // The orphan warning gets a fresh bare-UUID id and its own
    // first-appearance seq.
    assert!(uuid::Uuid::parse_str(warning.id.as_str()).is_ok());
    assert_eq!((warning.seq, warning.revision), (1, 1));

    assert!(
        matches!(&envelopes[1].item, Item::HostedToolCall { tool_name, output: Some(output), .. }
            if tool_name == "image_generation" && output == &serde_json::Value::String("image result".into()))
    );

    let RolloutLineV2::Turn { turn, .. } = &projected[1] else {
        panic!("second line is the turn");
    };
    // Waiting on an approval is still InProgress in the canonical model.
    assert_eq!(turn.status, CanonicalTurnStatus::InProgress);

    assert!(
        matches!(&projected[4], RolloutLineV2::SessionRollback { latest_turn_id: None, retained_turn_ids, .. }
            if retained_turn_ids.is_empty())
    );

    assert_v2_roundtrip(&projected);
}

#[test]
fn command_execution_falls_back_to_empty_cwd_before_session_meta() {
    let mut projector = LegacyProjector::new();
    let mut record = item_record(0xe1, 0xe2, 0xe3, 1);
    record.output_items = vec![TurnItem::CommandExecution(CommandExecutionItem {
        tool_call_id: "call-x".into(),
        tool_name: "exec_command".into(),
        command: "ls".into(),
        input: serde_json::json!({"command": "ls"}),
        output: serde_json::json!({}),
        is_error: false,
    })];
    let projected = projector
        .project_line(&item_line(record))
        .expect("projection succeeds");
    let envelopes = item_envelopes(&projected);
    assert!(
        matches!(&envelopes[0].item, Item::CommandExecution { cwd, .. } if cwd == &PathBuf::new())
    );
}
