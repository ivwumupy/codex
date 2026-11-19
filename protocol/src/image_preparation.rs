use serde::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDetailSetting {
    High,
    Original,
}

/// Measurements for one successfully decoded image at the point where Codex prepares it for
/// durable conversation history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ImagePreparationMetadata {
    /// Set for images embedded in message content.
    pub message_role: Option<String>,
    /// Set to the originating call ID for tool-output images. This joins to the `item_id` on
    /// existing tool events for tool type and provenance.
    pub item_id: Option<String>,
    pub effective_detail: ImageDetailSetting,
    pub source_width: u32,
    pub source_height: u32,
    pub prepared_width: u32,
    pub prepared_height: u32,
}
