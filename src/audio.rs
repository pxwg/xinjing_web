use opus::{Channels, Decoder};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tracing::{error, info, warn};

// 引入降噪和重采样库
use nnnoiseless::DenoiseState;
use rubato::{FastFixedIn, PolynomialDegree, Resampler};

/// 音频增强器：负责升采样、降噪、降采样
struct VoiceEnhancer {
    up_resampler: FastFixedIn<f32>,
    down_resampler: FastFixedIn<f32>,
    denoiser: DenoiseState<'static>,
    up_buffer: Vec<Vec<f32>>,
    down_buffer: Vec<Vec<f32>>,
    denoise_buffer: [f32; 480],
    wave_48k_buffer: Vec<f32>,
}

impl VoiceEnhancer {
    fn new() -> Self {
        // 升采样器 (16k -> 48k)
        let up_resampler =
            FastFixedIn::<f32>::new(48000.0 / 16000.0, 3.0, PolynomialDegree::Septic, 160, 1)
                .expect("无法创建升采样器");

        // 降采样器 (48k -> 16k)
        let down_resampler =
            FastFixedIn::<f32>::new(16000.0 / 48000.0, 1.0, PolynomialDegree::Septic, 480, 1)
                .expect("无法创建降采样器");

        // RNNoise 降噪器
        let denoiser = DenoiseState::new();

        Self {
            up_resampler,
            down_resampler,
            denoiser: *denoiser,
            up_buffer: vec![vec![0.0; 160]],
            down_buffer: vec![vec![0.0; 480]],
            denoise_buffer: [0.0; 480],
            wave_48k_buffer: Vec::with_capacity(480),
        }
    }

    /// 处理 10ms (160 samples) 的 16kHz 音频片段
    fn process_10ms_chunk(&mut self, input_16k: &[f32]) -> Vec<f32> {
        if input_16k.len() != 160 {
            warn!("VoiceEnhancer 接收到错误的块大小: {}", input_16k.len());
            return input_16k.to_vec();
        }

        // 1. 升采样 16k -> 48k
        self.up_buffer[0].copy_from_slice(input_16k);
        let output = match self.up_resampler.process(&self.up_buffer, None) {
            Ok(output) => output,
            Err(e) => {
                warn!("升采样失败: {:?}", e);
                return input_16k.to_vec();
            }
        };

        self.wave_48k_buffer.clear();
        self.wave_48k_buffer.extend_from_slice(&output[0]);

        // 2. 降噪 (RNNoise 需要 480 样本/帧)
        let mut noisy_frame = [0.0; 480];
        if self.wave_48k_buffer.len() >= 480 {
            noisy_frame.copy_from_slice(&self.wave_48k_buffer[..480]);
        } else {
            return input_16k.to_vec();
        }

        self.denoiser
            .process_frame(&mut self.denoise_buffer, &noisy_frame);

        // 3. 降采样 48k -> 16k
        self.down_buffer[0].copy_from_slice(&self.denoise_buffer);
        match self.down_resampler.process(&self.down_buffer, None) {
            Ok(output) => output[0].clone(),
            Err(e) => {
                warn!("降采样失败: {:?}", e);
                input_16k.to_vec()
            }
        }
    }
}

pub struct AudioProcessor {
    decoder: Decoder,
    pcm_buffer: [i16; 5760],
    audio_buffer: Vec<f32>,
    vad_state: VadState,
    enhancer: VoiceEnhancer,
    leftover_buffer: Vec<f32>,
}

struct VadState {
    silence_frames: usize,
    is_recording: bool,
    max_energy: f32,
}

// 稍微调低 VAD 阈值，因为降噪后的底噪会变小
const VAD_THRESHOLD_START: f32 = 400.0;
const VAD_THRESHOLD_END: f32 = 200.0;
const MAX_SILENCE_FRAMES: usize = 12;
const MAX_BUFFER_SIZE: usize = 16000 * 30;

impl AudioProcessor {
    pub fn new() -> Result<Self, opus::Error> {
        let decoder = Decoder::new(16000, Channels::Mono)?;

        Ok(Self {
            decoder,
            pcm_buffer: [0i16; 5760],
            audio_buffer: Vec::with_capacity(16000 * 10),
            vad_state: VadState {
                silence_frames: 0,
                is_recording: false,
                max_energy: 0.0,
            },
            enhancer: VoiceEnhancer::new(),
            leftover_buffer: Vec::new(),
        })
    }

    /// 处理音频数据，返回是否有完整语音片段
    pub fn process_audio(&mut self, opus_data: &[u8]) -> Option<Vec<f32>> {
        match self.decoder.decode(opus_data, &mut self.pcm_buffer, false) {
            Ok(samples_count) => {
                let pcm_slice = &self.pcm_buffer[..samples_count];

                // 1. 转换为 f32 并添加到剩余缓冲区
                let samples_f32: Vec<f32> = pcm_slice.iter().map(|&s| s as f32 / 32768.0).collect();
                self.leftover_buffer.extend(samples_f32);

                // 2. 按 10ms (160 samples) 分块处理
                let mut enhanced_samples = Vec::new();

                while self.leftover_buffer.len() >= 160 {
                    let chunk: Vec<f32> = self.leftover_buffer.drain(0..160).collect();
                    let clean_chunk = self.enhancer.process_10ms_chunk(&chunk);
                    enhanced_samples.extend(clean_chunk);
                }

                if enhanced_samples.is_empty() {
                    return None;
                }

                // 3. 计算能量 (使用增强后的音频)
                let samples_i16: Vec<i16> = enhanced_samples
                    .iter()
                    .map(|&s| (s * 32767.0).clamp(-32768.0, 32767.0) as i16)
                    .collect();

                let energy = calculate_rms(&samples_i16);

                // 4. 更新 VAD 状态
                self.update_vad_state(&enhanced_samples, energy)
            }
            Err(e) => {
                warn!("Opus解码错误: {}", e);
                None
            }
        }
    }

    /// 更新语音活动检测状态
    fn update_vad_state(&mut self, samples: &[f32], energy: f32) -> Option<Vec<f32>> {
        if !self.vad_state.is_recording {
            if energy > VAD_THRESHOLD_START {
                info!("检测到语音 (能量: {:.1})，开始录音...", energy);
                self.start_recording(samples, energy);
            }
            return None;
        }

        self.audio_buffer.extend_from_slice(samples);

        if energy > self.vad_state.max_energy {
            self.vad_state.max_energy = energy;
        }

        if energy < VAD_THRESHOLD_END {
            self.vad_state.silence_frames += 1;
        } else {
            self.vad_state.silence_frames = 0;
        }

        if self.vad_state.silence_frames >= MAX_SILENCE_FRAMES {
            return self.finalize_recording();
        }

        self.check_buffer_overflow();
        None
    }

    /// 开始录音
    fn start_recording(&mut self, samples: &[f32], energy: f32) {
        self.vad_state.is_recording = true;
        self.vad_state.silence_frames = 0;
        self.vad_state.max_energy = energy;
        self.audio_buffer.extend_from_slice(samples);
    }

    /// 完成录音并返回音频数据
    fn finalize_recording(&mut self) -> Option<Vec<f32>> {
        if self.audio_buffer.len() > 8000 {
            let result = self.audio_buffer.clone();

            // 仅在debug模式下保存音频到本地文件
            #[cfg(debug_assertions)]
            if let Err(e) = self.save_audio_to_file(&result) {
                error!("保存音频文件失败: {}", e);
            }
            self.reset_state();
            Some(result)
        } else {
            self.reset_state();
            None
        }
    }

    /// 重置录音状态
    fn reset_state(&mut self) {
        self.audio_buffer.clear();
        self.vad_state.silence_frames = 0;
        self.vad_state.is_recording = false;
        self.vad_state.max_energy = 0.0;
    }

    /// 检查缓冲区溢出
    fn check_buffer_overflow(&mut self) {
        if self.audio_buffer.len() > MAX_BUFFER_SIZE {
            warn!("缓冲区过大，重置");
            self.reset_state();
        }
    }

    /// 将音频数据保存为WAV文件
    fn save_audio_to_file(&self, audio_data: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
        use std::time::{SystemTime, UNIX_EPOCH};

        // 创建唯一的文件名
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let filename = format!("audio_capture_{}.wav", timestamp);

        // 确保audio目录存在
        std::fs::create_dir_all("audio")?;
        let filepath = Path::new("audio").join(&filename);

        // WAV文件参数
        let sample_rate = 16000u32;
        let num_channels = 1u16;
        let bits_per_sample = 16u16;
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        let block_align = num_channels * bits_per_sample / 8;

        // 转换f32到i16
        let pcm_data: Vec<i16> = audio_data
            .iter()
            .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        let data_size = pcm_data.len() * 2; // 2 bytes per sample
        let file_size = 36 + data_size;

        let mut file = File::create(&filepath)?;

        // WAV header
        file.write_all(b"RIFF")?;
        file.write_all(&(file_size as u32).to_le_bytes())?;
        file.write_all(b"WAVE")?;

        // fmt chunk
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?; // chunk size
        file.write_all(&1u16.to_le_bytes())?; // audio format (PCM)
        file.write_all(&num_channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;

        // data chunk
        file.write_all(b"data")?;
        file.write_all(&(data_size as u32).to_le_bytes())?;

        // PCM data
        for sample in pcm_data {
            file.write_all(&sample.to_le_bytes())?;
        }

        info!("音频已保存到: {}", filepath.display());
        Ok(())
    }
}

/// 计算音频样本的RMS能量
fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&s| (s as f32).powi(2)).sum();
    (sum / samples.len() as f32).sqrt()
}
