use axum::{extract::ws::WebSocketUpgrade, response::IntoResponse, routing::get, Router};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::info;

mod audio;
mod emotion;
mod protocol;
mod speech;
mod websocket;

use emotion::EmotionAnalyzer;
use speech::SpeechRecognizer;

#[tokio::main]
async fn main() {
    init_logging();

    // 修改：优先从环境变量读取模型路径，默认值为 "ggml-base.bin"
    let model_path = std::env::var("MODEL_PATH").unwrap_or_else(|_| "ggml-base.bin".to_string());

    info!("正在初始化系统...");
    info!("加载 Whisper 模型路径: {}", model_path);

    // 传入动态获取的路径
    let speech_recognizer = Arc::new(SpeechRecognizer::new(&model_path).await);
    let emotion_analyzer = Arc::new(EmotionAnalyzer::new().await);

    let app = Router::new().route(
        "/ws",
        get(move |ws| ws_handler(ws, speech_recognizer.clone(), emotion_analyzer.clone())),
    );

    let addr = SocketAddr::from(([0, 0, 0, 0], 4321));
    info!("🚀 心镜 (Heart Mirror) 大脑已启动，监听: {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// 初始化日志系统
fn init_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();
}

/// WebSocket 升级处理器
async fn ws_handler(
    ws: WebSocketUpgrade,
    speech_recognizer: Arc<SpeechRecognizer>,
    emotion_analyzer: Arc<EmotionAnalyzer>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        websocket::handle_connection(socket, speech_recognizer, emotion_analyzer)
    })
}
