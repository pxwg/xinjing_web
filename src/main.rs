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
use tracing::{debug, error, info, warn};
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

    // 2. 加载 Whisper 中文模型 (ggml-base.bin)
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

    // A. 音频管道初始化
    let mut decoder = match Decoder::new(16000, Channels::Mono) {
        Ok(d) => d,
        Err(e) => {
            error!("Opus Init Error: {}", e);
            return;
        }
    };

    let mut pcm_i16 = [0i16; 5760];
    let mut audio_buffer: Vec<f32> = Vec::with_capacity(16000 * 10);

    // --- VAD 参数调整 ---
    let mut silence_frames = 0;
    let mut is_recording_speech = false;

    // 建议调高阈值，500 对于某些麦克风底噪来说可能太低了
    // 你可以在客户端打印一下 quiet 时的 RMS，通常在 100-800 之间
    const VAD_THRESHOLD_START: f32 = 800.0; // 开始说话的阈值 (高一点，防误触)
    const VAD_THRESHOLD_END: f32 = 500.0; // 持续说话的阈值 (低一点，防断句)
    const MAX_SILENCE_FRAMES: usize = 10; // 约 600ms-1s 的静音判停

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

                // 原有的 Ping/Pong 测试逻辑
                if text.contains("ping") {
                    let _ = socket.send(Message::Text("pong".to_string())).await;
                }

                // 原有的 JSON 解析逻辑
                handle_text_frame(&text);
            }

            Ok(Message::Binary(data)) => {
                match decoder.decode(&data, &mut pcm_i16, false) {
                    Ok(samples_count) => {
                        let slice = &pcm_i16[..samples_count];
                        let energy = calculate_rms(slice);

                        // --- 核心逻辑修改开始 ---

                        // 1. 状态机逻辑
                        if !is_recording_speech {
                            // A. 当前处于【待机状态】
                            if energy > VAD_THRESHOLD_START {
                                info!("🎤 检测到人声 (Energy: {:.0})，开始录音...", energy);
                                is_recording_speech = true;
                                silence_frames = 0;
                                // 把这一帧存进去，避免丢字
                                for &sample in slice {
                                    audio_buffer.push(sample as f32 / 32768.0);
                                }
                            } else {
                                // B. 只是噪音/静音 -> 丢弃！不要存入 buffer！
                                // 这样 Whisper 永远不会收到纯噪音，彻底解决幻觉
                            }
                        } else {
                            // C. 当前处于【录音状态】
                            // 无论能量大小，先存入 buffer (防止说话中间的微弱停顿被切掉)
                            for &sample in slice {
                                audio_buffer.push(sample as f32 / 32768.0);
                            }

                            // D. 判断是否说完
                            if energy < VAD_THRESHOLD_END {
                                silence_frames += 1;
                            } else {
                                silence_frames = 0; // 还在说话，重置静音计数
                            }

                            // E. 触发识别条件
                            if silence_frames >= MAX_SILENCE_FRAMES {
                                // 只有累积了足够长的音频才识别 (避免短促的碰撞声触发)
                                if audio_buffer.len() > 8000 {
                                    // 0.5秒以上
                                    info!(
                                        "⏹️ 语音结束，缓冲区大小: {}，提交识别...",
                                        audio_buffer.len()
                                    );

                                    let text = run_whisper_inference(&ctx, &audio_buffer);

                                    // 过滤掉常见的空白幻觉
                                    let clean_text = text.trim();
                                    // 这里可以加一个简单的黑名单过滤
                                    if !clean_text.is_empty() && clean_text != "你去找我吧" {
                                        info!("🗣️ 识别结果: [{}]", clean_text);

                                        let resp = ServerResponse {
                                            msg_type: "llm".to_string(),
                                            emotion: "joy".to_string(),
                                            text: Some(clean_text.to_string()),
                                        };
                                        let _ = socket
                                            .send(Message::Text(
                                                serde_json::to_string(&resp).unwrap(),
                                            ))
                                            .await;
                                    } else {
                                        info!("(忽略幻觉/无效内容)");
                                    }
                                } else {
                                    info!("(音频太短，丢弃)");
                                }

                                // F. 重置状态，回到待机
                                audio_buffer.clear();
                                silence_frames = 0;
                                is_recording_speech = false;
                            }
                        }

                        // 保护机制：防止一直说话导致内存溢出 (比如 30秒强制截断)
                        if audio_buffer.len() > 16000 * 30 {
                            warn!("缓冲区过大，强制截断重置");
                            audio_buffer.clear();
                            is_recording_speech = false;
                        }
                        // --- 核心逻辑修改结束 ---
                    }
                    Err(e) => warn!("Opus 解码失败: {}", e),
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

// --- 辅助函数保持不变 ---

fn handle_text_frame(text: &str) {
    match serde_json::from_str::<DeviceMessage>(text) {
        Ok(DeviceMessage::Hello { version }) => info!("握手成功，版本: {}", version),
        Ok(DeviceMessage::Event { key, value }) => info!("事件触发: {} -> {}", key, value),
        Err(_) => info!("(非 JSON 文本或格式错误): {}", text),
    }
}

fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&s| (s as f32).powi(2)).sum();
    (sum / samples.len() as f32).sqrt()
}

// Whisper 推理函数 (配置为中文)
fn run_whisper_inference(ctx: &WhisperContext, data: &[f32]) -> String {
    let mut state = ctx.create_state().expect("无法创建 Whisper State");

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    // 设置中文
    params.set_language(Some("zh"));
    params.set_n_threads(4);
    params.set_print_special(false);
    params.set_print_progress(false);

    // 执行推理
    if let Err(e) = state.full(params, data) {
        error!("Whisper 推理失败: {}", e);
        return String::new();
    }

    // 1. 获取分段数量 (在这个版本直接返回 i32)
    let num_segments = state.full_n_segments();

    let mut result = String::new();
    for i in 0..num_segments {
        // 2. 修正：使用 if let Some 匹配 Option 类型
        if let Some(segment) = state.get_segment(i) {
            result.push_str(&segment.to_string());
        }
    }
    result
}
