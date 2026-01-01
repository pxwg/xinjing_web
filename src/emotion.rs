use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{error, info, warn};

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

pub struct EmotionAnalyzer {
    client: Client,
    model_name: String,
    valid_emotions: &'static [&'static str],
    api_base_url: String,
}

impl EmotionAnalyzer {
    /// 创建新的情绪分析器
    pub async fn new() -> Self {
        let ollama_host =
            std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://ollama:11434".to_string());

        let analyzer = Self {
            client: Client::new(),
            model_name: "qwen2.5:1.5b".to_string(),
            valid_emotions: &[
                "neutral",
                "happy",
                "joy",
                "sad",
                "angry",
                "crying",
                "loving",
                "embarrassed",
                "surprised",
                "shocked",
                "thinking",
                "winking",
                "cool",
                "relaxed",
                "delicious",
                "kissy",
                "confident",
                "sleepy",
                "silly",
                "confused",
            ],
            api_base_url: format!("{}/api/generate", ollama_host),
        };

        analyzer.test_connection().await;
        analyzer
    }

    /// 分析文本情绪
    pub async fn analyze(&self, text: &str) -> String {
        let prompt = self.build_emotion_prompt(text);

        match self.send_ollama_request(&prompt).await {
            Ok(response) => self.validate_emotion_response(&response),
            Err(e) => {
                warn!("情绪分析失败: {}, 使用默认情绪", e);
                "neutral".to_string()
            }
        }
    }

    /// 测试与Ollama的连接
    async fn test_connection(&self) {
        match self.send_test_request().await {
            Ok(_) => info!("✅ Ollama {} 模型连接成功", self.model_name),
            Err(e) => {
                error!("❌ Ollama 连接失败: {}", e);
                error!("💡 提示: 运行 'ollama run {}' 来安装模型", self.model_name);
            }
        }
    }

    /// 发送测试请求
    async fn send_test_request(&self) -> Result<(), Box<dyn std::error::Error>> {
        let request = OllamaRequest {
            model: self.model_name.clone(),
            prompt: "测试".to_string(),
            stream: false,
        };

        let response = self
            .client
            .post(&self.api_base_url)
            .json(&request)
            .timeout(Duration::from_secs(10))
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("Ollama 返回错误状态: {}", response.status()).into())
        }
    }

    /// 构建情绪分析提示词
    fn build_emotion_prompt(&self, text: &str) -> String {
        let prompt = format!(
            "Analyze the sentiment of the following text. ONLY output ONE word, strictly from this list: {:?}. Do NOT output anything else.\n\nText: {}\n\nSentiment:",
            self.valid_emotions, text,
        );
        info!("情绪分析提示词: {}", prompt);
        prompt
    }

    /// 发送Ollama请求
    async fn send_ollama_request(
        &self,
        prompt: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let request = OllamaRequest {
            model: self.model_name.clone(),
            prompt: prompt.to_string(),
            stream: false,
        };

        let response = self
            .client
            .post(&self.api_base_url)
            .json(&request)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        let ollama_resp: OllamaResponse = response.json().await?;
        Ok(ollama_resp.response)
    }

    /// 验证并清理情绪响应
    fn validate_emotion_response(&self, response: &str) -> String {
        let emotion = response.trim().to_lowercase();

        for &valid_emotion in self.valid_emotions {
            if emotion.contains(valid_emotion) {
                return valid_emotion.to_string();
            }
        }

        info!("LLM 返回了非预期的情绪: {}, 使用 neutral", emotion);
        "neutral".to_string()
    }
}
