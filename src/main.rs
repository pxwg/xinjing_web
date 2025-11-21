use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tracing::{error, info};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DeviceMessage {
    Hello { version: String },
    Event { key: String, value: String },
}

#[derive(Debug, Serialize)]
struct ServerResponse {
    #[serde(rename = "type")]
    msg_type: String,
    emotion: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[tokio::main]
async fn main() {
    // 初始化日志，使其打印到控制台
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let app = Router::new().route("/ws", get(ws_handler));

    let addr = SocketAddr::from(([0, 0, 0, 0], 4321));
    info!("心镜 (Heart Mirror) 大脑已启动，监听: {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    info!("新的边缘探针 (Client) 已连接");

    // 发送初始欢迎消息
    let initial_state = ServerResponse {
        msg_type: "llm".to_string(),
        emotion: "calm".to_string(),
        text: Some("Connected".to_string()),
    };
    let _ = socket
        .send(Message::Text(
            serde_json::to_string(&initial_state).unwrap(),
        ))
        .await;

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                info!("收到文本帧: {}", text);
                // 简单的回显测试 logic
                if text.contains("ping") {
                    let _ = socket.send(Message::Text("pong".to_string())).await;
                }
                handle_text_frame(&text);
            }
            Ok(Message::Binary(data)) => {
                info!("收到音频帧: {} bytes", data.len());
                // 模拟随机反馈
                if rand::random::<f32>() > 0.5 {
                    info!(">>> 模拟 AI 触发反馈: joy");
                    let resp = ServerResponse {
                        msg_type: "llm".to_string(),
                        emotion: "joy".to_string(),
                        text: None,
                    };
                    let _ = socket
                        .send(Message::Text(serde_json::to_string(&resp).unwrap()))
                        .await;
                }
            }
            Ok(Message::Close(_)) => {
                info!("连接断开");
                break;
            }
            Err(e) => {
                error!("Socket 错误: {}", e);
                break;
            }
            _ => {}
        }
    }
}

fn handle_text_frame(text: &str) {
    match serde_json::from_str::<DeviceMessage>(text) {
        Ok(DeviceMessage::Hello { version }) => info!("握手成功，版本: {}", version),
        Ok(DeviceMessage::Event { key, value }) => info!("事件触发: {} -> {}", key, value),
        Err(_) => info!("(非 JSON 文本或格式错误): {}", text),
    }
}
