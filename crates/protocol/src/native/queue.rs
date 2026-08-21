//! Session input queue: pre-item entries that have not materialized into a
//! user message and do not belong to any turn. Because they are pre-items they
//! are freely editable; the "persisted messages are immutable" rule does not
//! apply.
//!
//! Truth source: `devo-api-design/01-native-api.md` §4.3.

use chrono::DateTime;
use chrono::Utc;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::ids::QueueItemId;
use super::item::UserInput;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct QueueEntry {
    pub queue_item_id: QueueItemId,
    pub position: u32,
    /// Full editable content; `session/queue/update` replaces it wholesale.
    pub input: Vec<UserInput>,
    /// Short single-line preview for list rendering.
    pub preview: String,
    pub enqueued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub enum QueueChange {
    Added,
    Updated,
    Removed,
    /// Promoted out of the queue into the running turn as a steer.
    Promoted,
    /// Dequeued to start its own new turn.
    Drained,
}
