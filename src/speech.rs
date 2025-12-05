use std::path::Path;
use tracing::{error, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct SpeechRecognizer {
    context: WhisperContext,
}

impl SpeechRecognizer {
    /// 创建新的语音识别器
    pub async fn new(model_path: &str) -> Self {
        Self::validate_model_path(model_path);

        info!("正在加载 Whisper 模型...");
        let context =
            WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                .expect("模型加载失败");

        info!("✅ Whisper 模型加载完毕");

        Self { context }
    }

    /// 对音频数据进行语音识别
    pub fn recognize(&self, audio_data: &[f32]) -> String {
        let mut state = match self.context.create_state() {
            Ok(state) => state,
            Err(e) => {
                error!("无法创建 Whisper State: {}", e);
                return String::new();
            }
        };

        let params = self.create_inference_params();

        if let Err(e) = state.full(params, audio_data) {
            error!("Whisper推理失败: {}", e);
            return String::new();
        }

        self.extract_text_from_segments(&state)
    }

    /// 验证模型文件是否存在
    fn validate_model_path(model_path: &str) {
        if !Path::new(model_path).exists() {
            panic!(
                "❌ 错误: 找不到模型 '{}'。请先下载支持中文的 ggml 模型",
                model_path
            );
        }
    }

    /// 创建推理参数
    fn create_inference_params(&self) -> FullParams {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("zh"));
        params.set_initial_prompt("简体中文");
        params.set_n_threads(4);
        params.set_print_special(false);
        params.set_print_progress(false);
        params
    }

    /// 从分段中提取文本
    fn extract_text_from_segments(&self, state: &whisper_rs::WhisperState) -> String {
        let num_segments = state.full_n_segments();
        let mut result = String::new();

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                result.push_str(&segment.to_string());
            }
        }

        result
    }
}
