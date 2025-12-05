use opus::{Channels, Decoder};
use tracing::{error, warn};

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
}

/// 计算音频样本的RMS能量
fn calculate_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|&s| (s as f32).powi(2)).sum();
    (sum / samples.len() as f32).sqrt()
}
