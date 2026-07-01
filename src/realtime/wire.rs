use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRealtimeEnvelope {
    pub schema: String,
    pub schema_version: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub message_id: String,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRealtimeIssue {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeRealtimeConnectionIdentity {
    pub(super) connection_id: String,
    pub(super) client_id: String,
    pub(super) window_id: String,
    pub(super) resume_token: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RuntimeRealtimeSessionRevisions {
    pub(super) session_revision: u64,
    pub(super) view_revision: u64,
    pub(super) control_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) graph_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RuntimeRealtimeReplay {
    pub events: Vec<RuntimeRealtimeEnvelope>,
    pub high_water_sequence: u64,
}
