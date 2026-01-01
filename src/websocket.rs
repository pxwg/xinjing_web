use axum::extract::ws::{Message, WebSocket};
use std::sync::Arc;
use tracing::{error, info, warn};
// 引入 WhisperState
use whisper_rs::WhisperState;

use crate::audio::AudioProcessor;
use crate::emotion::EmotionAnalyzer;
use crate::protocol::{DeviceMessage, ServerResponse};
use crate::speech::SpeechRecognizer;

/// WebSocket连接处理器
pub async fn handle_connection(
    mut socket: WebSocket,
    speech_recognizer: Arc<SpeechRecognizer>,
    emotion_analyzer: Arc<EmotionAnalyzer>,
) {
    info!("新连接");

    let mut audio_processor = match AudioProcessor::new() {
        Ok(processor) => processor,
        Err(e) => {
            warn!("音频处理器初始化失败: {}", e);
            return;
        }
    };

    // 1. 在连接开始时，只创建一次 Whisper State
    let mut whisper_state = match speech_recognizer.create_state() {
        Ok(state) => {
            info!("Whisper State 初始化成功 (缓冲区已分配)");
            state
        }
        Err(e) => {
            error!("Whisper State 初始化失败: {}", e);
            return;
        }
    };

    send_initial_response(&mut socket).await;

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                handle_text_message(&mut socket, &text).await;
            }
            Ok(Message::Binary(data)) => {
                handle_audio_message(
                    &mut socket,
                    &mut audio_processor,
                    &speech_recognizer,
                    &mut whisper_state, // 2. 传入 state 的可变引用
                    &emotion_analyzer,
                    &data,
                )
                .await;
            }
            Ok(Message::Close(_)) => break,
            _ => {}
        }
    }

    info!("连接断开");
    // whisper_state 在这里会被自动释放
}

/// 发送初始连接响应
async fn send_initial_response(socket: &mut WebSocket) {
    let response = ServerResponse::initial_connection();
    if let Ok(json) = serde_json::to_string(&response) {
        let _ = socket.send(Message::Text(json)).await;
    }
}

/// 处理文本消息
async fn handle_text_message(socket: &mut WebSocket, text: &str) {
    info!("收到文本帧: {}", text);

    if text.contains("ping") {
        let _ = socket.send(Message::Text("pong".to_string())).await;
        return;
    }

    match serde_json::from_str::<DeviceMessage>(text) {
        Ok(DeviceMessage::Hello { version }) => {
            info!("APP握手: {}", version);
        }
        Ok(DeviceMessage::Event { key, value }) => {
            info!("APP事件: {} -> {}", key, value);
        }
        Err(_) => {
            info!("Raw Text: {}", text);
        }
    }
}

/// 处理音频消息
async fn handle_audio_message(
    socket: &mut WebSocket,
    audio_processor: &mut AudioProcessor,
    speech_recognizer: &Arc<SpeechRecognizer>,
    whisper_state: &mut WhisperState, // 接收 state
    emotion_analyzer: &Arc<EmotionAnalyzer>,
    audio_data: &[u8],
) {
    if let Some(complete_audio) = audio_processor.process_audio(audio_data) {
        process_complete_speech(
            socket,
            speech_recognizer,
            whisper_state, // 传递 state
            emotion_analyzer,
            complete_audio,
        )
        .await;
    }
}

/// 处理完整的语音片段
async fn process_complete_speech(
    socket: &mut WebSocket,
    speech_recognizer: &Arc<SpeechRecognizer>,
    whisper_state: &mut WhisperState, // 接收 state
    emotion_analyzer: &Arc<EmotionAnalyzer>,
    audio_data: Vec<f32>,
) {
    // 3. 使用 recognize_with_state 进行推理
    let text = speech_recognizer.recognize_with_state(whisper_state, &audio_data);
    let clean_text = text.trim();

    if is_valid_speech(clean_text) {
        let emotion = emotion_analyzer.analyze(clean_text).await;
        info!("🗣️ 结果: [{}] | 情绪: [{}]", clean_text, emotion);

        let response = ServerResponse::speech_result(clean_text.to_string(), emotion);
        if let Ok(json) = serde_json::to_string(&response) {
            let _ = socket.send(Message::Text(json)).await;
        }
    } else {
        info!("(忽略无效语音)");
    }
}

/// 验证语音识别结果是否有效
fn is_valid_speech(text: &str) -> bool {
    !text.is_empty() && text != "你去找我吧"
}
