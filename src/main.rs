use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use opus::{Channels, Decoder};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{error, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

// --- 协议定义 ---
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
    // 1. 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // 2. 加载 Whisper 中文模型
    let model_path = "ggml-base.bin";
    if !std::path::Path::new(model_path).exists() {
        panic!(
            "❌ 错误: 找不到模型 '{}'。请先下载支持中文的 ggml 模型 (非 .en 版)。",
            model_path
        );
    }

    info!("正在加载 Whisper 模型 (这也是大脑启动最慢的一步)...");
    let ctx = Arc::new(
        WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .expect("模型加载失败"),
    );
    info!("✅ Whisper 模型加载完毕，支持中文识别");

    // 3. 启动服务
    let app = Router::new().route("/ws", get(move |ws| ws_handler(ws, ctx.clone())));
    let addr = SocketAddr::from(([0, 0, 0, 0], 4321));
    info!("🚀 心镜 (Heart Mirror) 大脑已启动，监听: {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, ctx: Arc<WhisperContext>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, ctx))
}

// --- 核心处理逻辑 ---
async fn handle_socket(mut socket: WebSocket, ctx: Arc<WhisperContext>) {
    info!("🔌 新的边缘探针 (Client) 已连接");

    let mut decoder = match Decoder::new(16000, Channels::Mono) {
        Ok(d) => d,
        Err(e) => {
            error!("Opus Init Error: {}", e);
            return;
        }
    };

    let mut pcm_i16 = [0i16; 5760];
    let mut audio_buffer: Vec<f32> = Vec::with_capacity(16000 * 10);

    // --- VAD 参数 ---
    let mut silence_frames = 0;
    let mut is_recording_speech = false;
    let mut max_recorded_energy: f32 = 0.0;

    const VAD_THRESHOLD_START: f32 = 800.0;
    const VAD_THRESHOLD_END: f32 = 500.0;
    const MAX_SILENCE_FRAMES: usize = 12;

    // B. 发送初始状态
    let initial_state = ServerResponse {
        msg_type: "llm".to_string(),
        emotion: "calm".to_string(),
        text: Some("Connected & Ready".to_string()),
    };
    if let Ok(json) = serde_json::to_string(&initial_state) {
        let _ = socket.send(Message::Text(json)).await;
    }

    // C. 主消息循环
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                info!("收到文本帧: {}", text);
                if text.contains("ping") {
                    let _ = socket.send(Message::Text("pong".to_string())).await;
                }
                handle_text_frame(&text);
            }

            Ok(Message::Binary(data)) => {
                match decoder.decode(&data, &mut pcm_i16, false) {
                    Ok(samples_count) => {
                        let slice = &pcm_i16[..samples_count];
                        let energy = calculate_rms(slice);

                        // VAD 状态机
                        if !is_recording_speech {
                            if energy > VAD_THRESHOLD_START {
                                info!("🎤 开始录音 (Start Energy: {:.0})", energy);
                                is_recording_speech = true;
                                silence_frames = 0;
                                max_recorded_energy = energy;
                                for &sample in slice {
                                    audio_buffer.push(sample as f32 / 32768.0);
                                }
                            }
                        } else {
                            for &sample in slice {
                                audio_buffer.push(sample as f32 / 32768.0);
                            }

                            if energy > max_recorded_energy {
                                max_recorded_energy = energy;
                            }

                            if energy < VAD_THRESHOLD_END {
                                silence_frames += 1;
                            } else {
                                silence_frames = 0;
                            }

                            // 触发识别
                            if silence_frames >= MAX_SILENCE_FRAMES {
                                if audio_buffer.len() > 8000 {
                                    info!(
                                        "⏹️ 语音结束，峰值能量: {:.0}，提交识别...",
                                        max_recorded_energy
                                    );

                                    let text = run_whisper_inference(&ctx, &audio_buffer);
                                    // 1. 在这里转为简体，或者依赖 Prompt 的效果
                                    let clean_text = text.trim();

                                    // 2. 幻觉过滤
                                    if !clean_text.is_empty() && clean_text != "你去找我吧" {
                                        // 3. 情绪分析 (现在能更好地匹配简体关键词了)
                                        let emotion =
                                            analyze_emotion(clean_text, max_recorded_energy);

                                        info!("🗣️ 结果: [{}] | 情绪: [{}]", clean_text, emotion);

                                        let resp = ServerResponse {
                                            msg_type: "llm".to_string(),
                                            emotion: emotion,
                                            text: Some(clean_text.to_string()),
                                        };
                                        let _ = socket
                                            .send(Message::Text(
                                                serde_json::to_string(&resp).unwrap(),
                                            ))
                                            .await;
                                    } else {
                                        info!("(忽略幻觉)");
                                    }
                                } else {
                                    info!("(音频太短丢弃)");
                                }

                                audio_buffer.clear();
                                silence_frames = 0;
                                is_recording_speech = false;
                                max_recorded_energy = 0.0;
                            }
                        }

                        // 保护机制
                        if audio_buffer.len() > 16000 * 30 {
                            warn!("缓冲区过大，重置");
                            audio_buffer.clear();
                            is_recording_speech = false;
                        }
                    }
                    Err(e) => warn!("Opus Error: {}", e),
                }
            }
            Ok(Message::Close(_)) => break,
            _ => {}
        }
    }
    info!("连接断开");
}

fn handle_text_frame(text: &str) {
    match serde_json::from_str::<DeviceMessage>(text) {
        Ok(DeviceMessage::Hello { version }) => info!("APP握手: {}", version),
        Ok(DeviceMessage::Event { key, value }) => info!("APP事件: {} -> {}", key, value),
        Err(_) => info!("Raw Text: {}", text),
    }
}

fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&s| (s as f32).powi(2)).sum();
    (sum / samples.len() as f32).sqrt()
}

// --- 简单的综合情绪分析器 ---
fn analyze_emotion(text: &str, max_energy: f32) -> String {
    let t = text.to_lowercase();

    // 1. 极高能量兜底 (大喊大叫)
    if max_energy > 15000.0 {
        if t.contains("滚") || t.contains("死") {
            return "anger".to_string();
        }
        return "fear".to_string();
    }

    // 2. 关键词匹配 (基于简体中文)
    if t.contains("开心") || t.contains("快乐") || t.contains("哈哈") || t.contains("棒") {
        return "joy".to_string();
    }
    if t.contains("滚") || t.contains("烦") || t.contains("讨厌") || t.contains("气") {
        return "anger".to_string();
    }
    if t.contains("难过") || t.contains("累") || t.contains("苦") || t.contains("失望") {
        return "sadness".to_string();
    }
    if t.contains("怕") || t.contains("吓") || t.contains("救命") {
        return "fear".to_string();
    }
    if t.contains("安") || t.contains("静") || t.contains("睡") {
        return "sleep".to_string();
    }

    // 3. 极低能量兜底
    if max_energy < 1500.0 {
        return "calm".to_string();
    }

    "neutral".to_string()
}

// Whisper 推理函数
fn run_whisper_inference(ctx: &WhisperContext, data: &[f32]) -> String {
    let mut state = ctx.create_state().expect("无法创建 Whisper State");
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    params.set_language(Some("zh"));

    // 🔥 关键修改：使用 Prompt 强制模型“模仿”简体中文风格
    params.set_initial_prompt("简体中文");

    params.set_n_threads(4);
    params.set_print_special(false);
    params.set_print_progress(false);

    if let Err(e) = state.full(params, data) {
        error!("Whisper Fail: {}", e);
        return String::new();
    }

    let num_segments = state.full_n_segments();
    let mut result = String::new();
    for i in 0..num_segments {
        if let Some(segment) = state.get_segment(i) {
            result.push_str(&segment.to_string());
        }
    }
    result
}
