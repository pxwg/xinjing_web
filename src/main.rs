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

    let speech_recognizer = Arc::new(SpeechRecognizer::new("ggml-base.bin").await);
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
