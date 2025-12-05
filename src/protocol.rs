use chrono::{TimeZone, Utc};
use chrono_tz::Asia::Shanghai;
use rusqlite::{params, Connection, Result};
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
        // Write result to SQLite database
        if let Err(e) = insert_speech_result(&text, &emotion) {
            eprintln!("Failed to insert speech result: {}", e);
        }
        Self {
            msg_type: "llm".to_string(),
            emotion,
            text: Some(text),
        }
    }
}

/// 将情绪识别结果插入到SQLite数据库
/// 格式：id, text, emotion, created_at（ISO 8601时间戳）
fn insert_speech_result(text: &str, emotion: &str) -> rusqlite::Result<()> {
    let conn = Connection::open("history-emotion.db")?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS speech_results (
            id INTEGER PRIMARY KEY,
            text TEXT NOT NULL,
            emotion TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
        [],
    )?;
    let now = Shanghai
        .from_utc_datetime(&Utc::now().naive_utc())
        .to_rfc3339();
    conn.execute(
        "INSERT INTO speech_results (text, emotion, created_at) VALUES (?1, ?2, ?3)",
        params![text, emotion, now],
    )?;
    Ok(())
}
