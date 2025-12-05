use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceMessage {
    Hello { version: String },
    Event { key: String, value: String },
}

#[derive(Debug, Serialize)]
pub struct ServerResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub emotion: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl ServerResponse {
    /// 创建初始连接响应
    pub fn initial_connection() -> Self {
        Self {
            msg_type: "llm".to_string(),
            emotion: "calm".to_string(),
            text: Some("Connected & Ready".to_string()),
        }
    }

    /// 创建语音识别结果响应
    pub fn speech_result(text: String, emotion: String) -> Self {
        Self {
            msg_type: "llm".to_string(),
            emotion,
            text: Some(text),
        }
    }
}
