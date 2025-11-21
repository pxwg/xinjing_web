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

// --- Ollama API 定义 ---
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: String,
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

    // 3. 测试 Ollama 连接
    match test_ollama_connection().await {
        Ok(_) => info!("✅ Ollama qwen2.5:0.5b 模型连接成功"),
        Err(e) => {
            error!(
                "❌ Ollama 连接失败: {}. 请确保 Ollama 已启动并安装了 qwen2.5:0.5b 模型",
                e
            );
            error!("💡 提示: 运行 'ollama run qwen2.5:0.5b' 来安装模型");
        }
    }

    // 4. 启动服务
    let app = Router::new().route("/ws", get(move |ws| ws_handler(ws, ctx.clone())));
    let addr = SocketAddr::from(([0, 0, 0, 0], 4321));
    info!("🚀 心镜 (Heart Mirror) 大脑已启动，监听: {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn test_ollama_connection() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let test_prompt = "测试";

    let request = OllamaRequest {
        model: "qwen2.5:0.5b".to_string(),
        prompt: test_prompt.to_string(),
        stream: false,
    };

    let response = client
        .post("http://127.0.0.1:11434/api/generate")
        .json(&request)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Ollama 返回错误状态: {}", response.status()).into())
    }
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
                                    let clean_text = text.trim();

                                    // 幻觉过滤
                                    if !clean_text.is_empty() && clean_text != "你去找我吧" {
                                        // 使用 Ollama 进行情绪分析
                                        let emotion = analyze_emotion_with_llm(clean_text).await;

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

// --- 使用 Ollama Qwen2.5:0.5b 进行情绪分析 ---
async fn analyze_emotion_with_llm(text: &str) -> String {
    let client = reqwest::Client::new();

    let prompt = format!(
    "Analyze the sentiment of the following text. ONLY output ONE word, strictly from this list: [[joy, anger, sadness, fear, calm, neutral, sleep]]. Do NOT output anything else.\n\nText: {}\n\nSentiment:",
    text
    );

    let request = OllamaRequest {
        model: "qwen2.5:1.5b".to_string(),
        prompt,
        stream: false,
    };

    match client
        .post("http://127.0.0.1:11434/api/generate")
        .json(&request)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(response) => {
            if let Ok(ollama_resp) = response.json::<OllamaResponse>().await {
                let emotion = ollama_resp.response.trim().to_lowercase();

                // 验证返回的情绪是否在允许的列表中
                let valid_emotions = [
                    "joy", "anger", "sadness", "fear", "calm", "neutral", "sleep",
                ];
                for valid_emotion in valid_emotions.iter() {
                    if emotion.contains(valid_emotion) {
                        return valid_emotion.to_string();
                    }
                }

                info!("LLM 返回了非预期的情绪: {}, 使用 neutral", emotion);
                "neutral".to_string()
            } else {
                warn!("解析 Ollama 响应失败，使用 neutral");
                "neutral".to_string()
            }
        }
        Err(e) => {
            warn!("Ollama 请求失败: {}, 使用 neutral", e);
            "neutral".to_string()
        }
    }
}

// Whisper 推理函数
fn run_whisper_inference(ctx: &WhisperContext, data: &[f32]) -> String {
    let mut state = ctx.create_state().expect("无法创建 Whisper State");
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    params.set_language(Some("zh"));
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
