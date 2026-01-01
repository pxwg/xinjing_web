use opus::{Channels, Decoder};
use std::fs::File;
use std::io::Write;
use std::path::Path;
use tracing::{error, info, warn};

pub struct AudioProcessor {
    decoder: Decoder,
    pcm_buffer: [i16; 5760],
    audio_buffer: Vec<f32>,
    vad_state: VadState,
}

struct VadState {
    silence_frames: usize,
    is_recording: bool,
    max_energy: f32,
}

const VAD_THRESHOLD_START: f32 = 800.0;
const VAD_THRESHOLD_END: f32 = 500.0;
const MAX_SILENCE_FRAMES: usize = 12;
const MAX_BUFFER_SIZE: usize = 16000 * 30;

impl AudioProcessor {
    /// 创建新的音频处理器
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
        })
    }

    /// 处理音频数据，返回是否有完整语音片段
    pub fn process_audio(&mut self, opus_data: &[u8]) -> Option<Vec<f32>> {
        match self.decoder.decode(opus_data, &mut self.pcm_buffer, false) {
            Ok(samples_count) => {
                let pcm_slice = &self.pcm_buffer[..samples_count];
                let energy = calculate_rms(pcm_slice);

                let samples: Vec<i16> = pcm_slice.to_vec();
                self.update_vad_state(&samples, energy)
            }
            Err(e) => {
                warn!("Opus解码错误: {}", e);
                None
            }
        }
    }

    /// 更新语音活动检测状态
    fn update_vad_state(&mut self, samples: &[i16], energy: f32) -> Option<Vec<f32>> {
        if !self.vad_state.is_recording {
            if energy > VAD_THRESHOLD_START {
                self.start_recording(samples, energy);
            }
            return None;
        }

        self.add_samples_to_buffer(samples);

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
    fn start_recording(&mut self, samples: &[i16], energy: f32) {
        self.vad_state.is_recording = true;
        self.vad_state.silence_frames = 0;
        self.vad_state.max_energy = energy;
        self.add_samples_to_buffer(samples);
    }

    /// 添加样本到缓冲区
    fn add_samples_to_buffer(&mut self, samples: &[i16]) {
        for &sample in samples {
            self.audio_buffer.push(sample as f32 / 32768.0);
        }
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
