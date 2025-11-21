use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use opus::{Channels, Decoder};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tracing::{error, info, warn};

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
    // 初始化日志
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

    // --- 1. 初始化 Opus 解码器 ---
    // 采样率: 16000Hz (AI 模型/Whisper 的标准输入), 通道: 单声道
    let mut decoder = match Decoder::new(16000, Channels::Mono) {
        Ok(d) => d,
        Err(e) => {
            error!("无法初始化 Opus 解码器: {}", e);
            return; // 解码器初始化失败，断开连接
        }
    };

    // --- 2. 预分配 PCM 缓冲区 ---
    // 120ms @ 16kHz = 1920 samples。给 5760 足够大，防止某些大数据包溢出
    let mut pcm_buffer = [0i16; 5760];

    // 发送初始欢迎消息
    let initial_state = ServerResponse {
        msg_type: "llm".to_string(),
        emotion: "calm".to_string(),
        text: Some("Connected & Audio Ready".to_string()),
    };
    if let Ok(json) = serde_json::to_string(&initial_state) {
        let _ = socket.send(Message::Text(json)).await;
    }

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                // 处理控制指令
                if text.contains("ping") {
                    let _ = socket.send(Message::Text("pong".to_string())).await;
                }
                handle_text_frame(&text);
            }
            Ok(Message::Binary(data)) => {
                // --- 3. 解码 Opus 音频帧 ---
                // decode(输入字节, 输出buffer, 是否FEC)
                match decoder.decode(&data, &mut pcm_buffer, false) {
                    Ok(samples_count) => {
                        // 获取实际解码出来的 PCM 切片
                        let pcm_slice = &pcm_buffer[..samples_count];

                        // 计算能量 (RMS)，看看是不是静音
                        let energy = calculate_rms(pcm_slice);

                        // 只有当声音比较大时才打印日志，避免刷屏
                        if energy > 100.0 {
                            info!(
                                "收到音频 | 字节: {} | 解码采样数: {} | 能量(RMS): {:.2}",
                                data.len(),
                                samples_count,
                                energy
                            );

                            // TODO: 在这里将 pcm_slice 存入缓冲区，凑够时长后发给 Whisper
                        } else {
                            // 静音帧，可以选择忽略或 trace 记录
                            tracing::debug!("收到静音帧...");
                        }
                    }
                    Err(e) => {
                        // 如果发送的不是合法 Opus 数据（比如之前的 pseudo_packet），这里会报错
                        warn!("Opus 解码失败: {} (数据长度: {})", e, data.len());
                    }
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

// 辅助工具：计算音频能量 (Root Mean Square)
fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_squares: f32 = samples.iter().map(|&s| (s as f32).powi(2)).sum();
    (sum_squares / samples.len() as f32).sqrt()
}
