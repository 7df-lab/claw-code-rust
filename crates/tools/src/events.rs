use serde::{Deserialize, Serialize};

/// Sender for streaming tool output progress during execution.
/// Each `String` is a text delta chunk. The sender is consumed when
/// execution completes (sender drops after handle() returns).
pub type ToolProgressSender = tokio::sync::mpsc::UnboundedSender<String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_event_serde_begin() {
        let event = ToolEvent::ToolCallBegin {
            tool_name: "read".into(),
            call_id: "call-1".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_begin");
        assert_eq!(json["tool_name"], "read");
        assert_eq!(json["call_id"], "call-1");
    }

    #[test]
    fn tool_event_serde_end() {
        let event = ToolEvent::ToolCallEnd {
            tool_name: "bash".into(),
            call_id: "call-2".into(),
            duration_ms: 1234,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_call_end");
        assert_eq!(json["duration_ms"], 1234);
    }

    #[test]
    fn tool_event_serde_progress() {
        let event = ToolEvent::ToolProgress {
            tool_name: "webfetch".into(),
            call_id: "call-3".into(),
            message: "fetching...".into(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "tool_progress");
        assert_eq!(json["message"], "fetching...");
    }

    #[test]
    fn tool_event_roundtrip() {
        let events = vec![
            ToolEvent::ToolCallBegin {
                tool_name: "a".into(),
                call_id: "b".into(),
            },
            ToolEvent::ToolCallEnd {
                tool_name: "c".into(),
                call_id: "d".into(),
                duration_ms: 5,
            },
            ToolEvent::ToolProgress {
                tool_name: "e".into(),
                call_id: "f".into(),
                message: "g".into(),
            },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let deserialized: ToolEvent = serde_json::from_str(&json).unwrap();
            match (&event, &deserialized) {
                (
                    ToolEvent::ToolCallBegin { tool_name, call_id },
                    ToolEvent::ToolCallBegin {
                        tool_name: tn,
                        call_id: ci,
                    },
                ) => {
                    assert_eq!(tool_name, tn);
                    assert_eq!(call_id, ci);
                }
                (
                    ToolEvent::ToolCallEnd {
                        tool_name,
                        call_id,
                        duration_ms,
                    },
                    ToolEvent::ToolCallEnd {
                        tool_name: tn,
                        call_id: ci,
                        duration_ms: d,
                    },
                ) => {
                    assert_eq!(tool_name, tn);
                    assert_eq!(call_id, ci);
                    assert_eq!(duration_ms, d);
                }
                (
                    ToolEvent::ToolProgress {
                        tool_name,
                        call_id,
                        message,
                    },
                    ToolEvent::ToolProgress {
                        tool_name: tn,
                        call_id: ci,
                        message: m,
                    },
                ) => {
                    assert_eq!(tool_name, tn);
                    assert_eq!(call_id, ci);
                    assert_eq!(message, m);
                }
                _ => panic!("variant mismatch"),
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolEvent {
    ToolCallBegin {
        tool_name: String,
        call_id: String,
    },
    ToolCallEnd {
        tool_name: String,
        call_id: String,
        duration_ms: u64,
    },
    ToolProgress {
        tool_name: String,
        call_id: String,
        message: String,
    },
}
