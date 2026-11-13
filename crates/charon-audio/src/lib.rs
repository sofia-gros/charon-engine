//! 仕様 12: 音。依存クレートなしの**ミキサー**と、効果音の生成・WAV の読み込み。
//!
//! このクレートは、音を**混ぜて** (`Mixer::render`)、ステレオの `f32` の並びにするところまでを持つ。
//! 実際の音声デバイスへの出力は、`AudioOutput` を実装する別の部品 (デバイス用の依存クレートが要る) が受け持つ。

use std::collections::BTreeMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

/// 音 (モノラル、`f32`、-1.0..=1.0)。
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// 元のサンプルレート (Hz)。
    pub rate: u32,
    pub samples: Arc<[f32]>,
}

/// 波形の形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Sine,
    Square,
    Saw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WavError {
    /// `RIFF`/`WAVE` でない、または切れている。
    Malformed,
    /// 対応しない形式 (PCM 16 ビット・8 ビット・32 ビット float のモノラル / ステレオだけ)。
    Unsupported,
    /// 大きすぎる (1 億サンプルまで)。
    TooLarge,
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed => write!(f, "WAV の形が正しくありません"),
            Self::Unsupported => write!(f, "対応しない WAV の形式です"),
            Self::TooLarge => write!(f, "WAV が大きすぎます"),
        }
    }
}

impl std::error::Error for WavError {}

const MAX_SAMPLES: usize = 100_000_000;

/// WAV の `fmt ` チャンク: (形式の種類, チャンネル数, サンプルレート, ビット数)。
type WavFormat = (u16, u16, u32, u16);

impl Sound {
    pub fn new(rate: u32, samples: Vec<f32>) -> Self {
        Self { rate: rate.max(1), samples: samples.into() }
    }

    /// 長さ (秒)。
    pub fn seconds(&self) -> f32 {
        self.samples.len() as f32 / self.rate as f32
    }

    /// 周波数 `hz` の音。最後の 1/8 で小さくなる (ぷつっという音を避ける)。
    pub fn tone(shape: Shape, hz: f32, seconds: f32, rate: u32) -> Self {
        let rate = rate.max(1);
        let count = (seconds.max(0.0) * rate as f32) as usize;
        let samples = (0..count)
            .map(|i| {
                let phase = (i as f32 * hz / rate as f32).fract();
                let wave = match shape {
                    Shape::Sine => (phase * TAU).sin(),
                    Shape::Square => {
                        if phase < 0.5 {
                            0.6
                        } else {
                            -0.6
                        }
                    }
                    Shape::Saw => phase * 2.0 - 1.0,
                };
                wave * fade(i, count)
            })
            .collect();
        Self::new(rate, samples)
    }

    /// ノイズ (同じ `seed` なら同じ音)。時間とともに小さくなる (`decay` = 1 で最後に 0)。
    pub fn noise(seconds: f32, rate: u32, seed: u32, decay: f32) -> Self {
        let rate = rate.max(1);
        let count = (seconds.max(0.0) * rate as f32) as usize;
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let samples = (0..count)
            .map(|i| {
                // xorshift32。
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let white = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                let envelope = 1.0 - decay.clamp(0.0, 1.0) * (i as f32 / count.max(1) as f32);
                white * envelope
            })
            .collect();
        Self::new(rate, samples)
    }

    /// 周波数が `from` から `to` へ変わる音 (弾・ジャンプなど)。
    pub fn sweep(shape: Shape, from: f32, to: f32, seconds: f32, rate: u32) -> Self {
        let rate = rate.max(1);
        let count = (seconds.max(0.0) * rate as f32) as usize;
        let mut phase = 0.0_f32;
        let samples = (0..count)
            .map(|i| {
                let t = i as f32 / count.max(1) as f32;
                phase = (phase + (from + (to - from) * t) / rate as f32).fract();
                let wave = match shape {
                    Shape::Sine => (phase * TAU).sin(),
                    Shape::Square => {
                        if phase < 0.5 {
                            0.6
                        } else {
                            -0.6
                        }
                    }
                    Shape::Saw => phase * 2.0 - 1.0,
                };
                wave * fade(i, count)
            })
            .collect();
        Self::new(rate, samples)
    }

    /// WAV (RIFF) を読む。PCM 8・16・24 ビットと 32 ビット float、モノラルとステレオ (ステレオは平均してモノラルにする)。
    pub fn from_wav(bytes: &[u8]) -> Result<Self, WavError> {
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err(WavError::Malformed);
        }
        let mut format: Option<WavFormat> = None;
        let mut data: Option<&[u8]> = None;
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size =
                u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
                    as usize;
            let body = at + 8;
            let end = body.checked_add(size).ok_or(WavError::Malformed)?;
            if end > bytes.len() {
                return Err(WavError::Malformed);
            }
            match id {
                b"fmt " if size >= 16 => {
                    let b = &bytes[body..];
                    format = Some((
                        u16::from_le_bytes([b[0], b[1]]),
                        u16::from_le_bytes([b[2], b[3]]),
                        u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                        u16::from_le_bytes([b[14], b[15]]),
                    ));
                }
                b"data" => data = Some(&bytes[body..end]),
                _ => {}
            }
            at = end + (size & 1); // チャンクは偶数バイトに詰める。
        }
        let ((tag, channels, rate, bits), data) = format.zip(data).ok_or(WavError::Malformed)?;
        if rate == 0 || !(1..=2).contains(&channels) {
            return Err(WavError::Unsupported);
        }
        let width = usize::from(bits / 8);
        let frame = width * usize::from(channels);
        if frame == 0 || !matches!((tag, bits), (1, 8) | (1, 16) | (1, 24) | (3, 32)) {
            return Err(WavError::Unsupported);
        }
        let frames = data.len() / frame;
        if frames > MAX_SAMPLES {
            return Err(WavError::TooLarge);
        }
        let read = |chunk: &[u8]| -> f32 {
            match (tag, bits) {
                (1, 8) => (f32::from(chunk[0]) - 128.0) / 128.0,
                (1, 16) => f32::from(i16::from_le_bytes([chunk[0], chunk[1]])) / 32768.0,
                (1, 24) => {
                    let v = i32::from_le_bytes([0, chunk[0], chunk[1], chunk[2]]) >> 8;
                    v as f32 / 8_388_608.0
                }
                _ => f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).clamp(-1.0, 1.0),
            }
        };
        let samples = data
            .chunks_exact(frame)
            .map(|f| {
                let sum: f32 = f.chunks_exact(width).map(read).sum();
                sum / f32::from(channels)
            })
            .collect();
        Ok(Self::new(rate, samples))
    }
}

/// 最後の 1/8 で 0 へ。
fn fade(i: usize, count: usize) -> f32 {
    let tail = (count / 8).max(1);
    let left = count.saturating_sub(i);
    if left >= tail { 1.0 } else { left as f32 / tail as f32 }
}

/// 音量を分けて調整する系統。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bus {
    Sfx,
    Music,
}

/// 鳴らし方。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayParams {
    /// 0.0..=2.0。
    pub volume: f32,
    /// -1.0 (左)..=1.0 (右)。
    pub pan: f32,
    /// 0.25..=4.0 (1.0 が元の高さ)。
    pub pitch: f32,
    pub looped: bool,
    pub bus: Bus,
}

impl Default for PlayParams {
    fn default() -> Self {
        Self { volume: 1.0, pan: 0.0, pitch: 1.0, looped: false, bus: Bus::Sfx }
    }
}

/// 鳴っている音の番号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoiceId(pub u64);

#[derive(Clone, Debug)]
struct Voice {
    sound: Sound,
    position: f64,
    params: PlayParams,
}

/// 同時に鳴らせる音の数の上限。
pub const MAX_VOICES: usize = 32;

/// 音を混ぜる。
#[derive(Clone, Debug)]
pub struct Mixer {
    rate: u32,
    voices: BTreeMap<VoiceId, Voice>,
    next_id: u64,
    master: f32,
    sfx: f32,
    music: f32,
}

impl Mixer {
    /// 出力のサンプルレート (Hz)。
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1),
            voices: BTreeMap::new(),
            next_id: 1,
            master: 1.0,
            sfx: 1.0,
            music: 1.0,
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// 出力のサンプルレートを変える (デバイスに合わせる)。鳴っている音は止める。
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
        self.voices.clear();
    }

    /// 全体・効果音・音楽の音量 (0.0..=1.0 に収める。NaN は 0)。
    pub fn set_volumes(&mut self, master: f32, sfx: f32, music: f32) {
        let clean = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        (self.master, self.sfx, self.music) = (clean(master), clean(sfx), clean(music));
    }

    /// 鳴らす。同時に鳴らせる上限 (`MAX_VOICES`) に達していたら鳴らさず `None`。空の音も `None`。
    pub fn play(&mut self, sound: &Sound, params: PlayParams) -> Option<VoiceId> {
        if sound.samples.is_empty() || self.voices.len() >= MAX_VOICES {
            return None;
        }
        let clean = |v: f32, lo: f32, hi: f32, default: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { default }
        };
        let params = PlayParams {
            volume: clean(params.volume, 0.0, 2.0, 1.0),
            pan: clean(params.pan, -1.0, 1.0, 0.0),
            pitch: clean(params.pitch, 0.25, 4.0, 1.0),
            ..params
        };
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.voices.insert(id, Voice { sound: sound.clone(), position: 0.0, params });
        Some(id)
    }

    pub fn stop(&mut self, id: VoiceId) -> bool {
        self.voices.remove(&id).is_some()
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.voices.contains_key(&id)
    }

    /// 鳴っている音の数。
    pub fn active(&self) -> usize {
        self.voices.len()
    }

    /// `out` (左右が交互のステレオ、長さは偶数) に、次の分を混ぜて**上書き**する。-1.0..=1.0 に収める。
    /// 鳴り終わった音は取り除く。
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / 2;
        let mut finished = Vec::new();
        for (&id, voice) in &mut self.voices {
            let bus = match voice.params.bus {
                Bus::Sfx => self.sfx,
                Bus::Music => self.music,
            };
            let gain = voice.params.volume * bus * self.master;
            // 定power のパン。
            let angle = (voice.params.pan + 1.0) * 0.5 * FRAC_PI_2;
            let (left, right) = (gain * angle.cos(), gain * angle.sin());
            let step =
                f64::from(voice.params.pitch) * f64::from(voice.sound.rate) / f64::from(self.rate);
            let samples = &voice.sound.samples;
            let length = samples.len() as f64;
            for frame in 0..frames {
                if voice.position >= length {
                    if voice.params.looped {
                        voice.position %= length;
                    } else {
                        finished.push(id);
                        break;
                    }
                }
                let index = voice.position as usize;
                let next = if index + 1 < samples.len() {
                    samples[index + 1]
                } else if voice.params.looped {
                    samples[0]
                } else {
                    0.0
                };
                let t = (voice.position - index as f64) as f32;
                let value = samples[index] * (1.0 - t) + next * t;
                out[frame * 2] += value * left;
                out[frame * 2 + 1] += value * right;
                voice.position += step;
            }
        }
        for id in finished {
            self.voices.remove(&id);
        }
        for value in out.iter_mut() {
            *value = value.clamp(-1.0, 1.0);
        }
    }
}

/// 音声デバイスへの出力 (デバイス用の依存クレートで実装する。このクレートは実装を持たない)。
pub trait AudioOutput {
    /// デバイスのサンプルレート (Hz)。
    fn rate(&self) -> u32;
    /// 混ぜた音を渡す (ステレオの `f32`)。
    fn submit(&mut self, interleaved: &[f32]);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use super::*;

    const RATE: u32 = 8_000;

    fn peak(buffer: &[f32]) -> f32 {
        buffer.iter().fold(0.0, |m, v| m.max(v.abs()))
    }

    #[test]
    fn tone_has_the_requested_length_and_pitch() {
        let sound = Sound::tone(Shape::Sine, 440.0, 0.5, RATE);
        assert_eq!(sound.samples.len(), 4_000);
        assert!((sound.seconds() - 0.5).abs() < 1e-6);
        // 440 Hz の正弦波は、0.25 秒あたり 110 回、符号が負から正に変わる。
        let crossings =
            sound.samples[..2_000].windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((108..=111).contains(&crossings), "{crossings}");
        assert!(sound.samples.last().unwrap().abs() < 0.01, "最後は 0 に近づく");
        assert!(peak(&sound.samples) <= 1.0);
    }

    #[test]
    fn noise_is_deterministic_and_decays() {
        let a = Sound::noise(0.2, RATE, 7, 1.0);
        let b = Sound::noise(0.2, RATE, 7, 1.0);
        let c = Sound::noise(0.2, RATE, 8, 1.0);
        assert_eq!(a, b, "同じ seed は同じ音");
        assert_ne!(a, c);
        let head: f32 = a.samples[..200].iter().map(|v| v.abs()).sum();
        let tail: f32 = a.samples[a.samples.len() - 200..].iter().map(|v| v.abs()).sum();
        assert!(head > tail * 3.0, "小さくなる ({head} → {tail})");
    }

    #[test]
    fn mixer_plays_once_and_removes_finished_voices() {
        let mut mixer = Mixer::new(RATE);
        let sound = Sound::new(RATE, vec![0.5; 100]);
        let id = mixer.play(&sound, PlayParams::default()).unwrap();
        assert!(mixer.is_playing(id));
        let mut out = vec![0.0; 2 * 60];
        mixer.render(&mut out);
        assert!(mixer.is_playing(id), "まだ鳴っている");
        assert!(out.iter().all(|v| *v > 0.0), "左右に出る");
        mixer.render(&mut out);
        assert!(!mixer.is_playing(id), "鳴り終わったら消える");
        assert_eq!(mixer.active(), 0);
        let silent = out[2 * 40..].iter().all(|v| *v == 0.0);
        assert!(silent, "鳴り終わったあとは無音 (上書きする)");
    }

    #[test]
    fn looped_voices_repeat_until_stopped() {
        let mut mixer = Mixer::new(RATE);
        let sound = Sound::new(RATE, vec![0.25; 10]);
        let id = mixer.play(&sound, PlayParams { looped: true, ..Default::default() }).unwrap();
        let mut out = vec![0.0; 2 * 1_000];
        mixer.render(&mut out);
        assert!(mixer.is_playing(id));
        assert!(out.iter().all(|v| *v > 0.0), "途切れない");
        assert!(mixer.stop(id));
        assert!(!mixer.stop(id));
        mixer.render(&mut out);
        assert_eq!(peak(&out), 0.0);
    }

    #[test]
    fn pan_volume_and_buses_scale_the_output() {
        let sound = Sound::new(RATE, vec![0.5; 1_000]);
        let level = |params: PlayParams, volumes: (f32, f32, f32)| -> (f32, f32) {
            let mut mixer = Mixer::new(RATE);
            mixer.set_volumes(volumes.0, volumes.1, volumes.2);
            mixer.play(&sound, params).unwrap();
            let mut out = vec![0.0; 2 * 10];
            mixer.render(&mut out);
            (out[0], out[1])
        };
        let (l, r) = level(PlayParams::default(), (1.0, 1.0, 1.0));
        assert!((l - r).abs() < 1e-5 && l > 0.3, "中央は左右同じ");
        let (l, r) = level(PlayParams { pan: -1.0, ..Default::default() }, (1.0, 1.0, 1.0));
        assert!(l > 0.45 && r < 1e-4, "左いっぱい: {l} {r}");
        let (l, r) = level(PlayParams { pan: 1.0, ..Default::default() }, (1.0, 1.0, 1.0));
        assert!(r > 0.45 && l < 1e-4);
        let (full, _) = level(PlayParams::default(), (1.0, 1.0, 1.0));
        let (half, _) = level(PlayParams { volume: 0.5, ..Default::default() }, (1.0, 1.0, 1.0));
        assert!((half - full * 0.5).abs() < 1e-5, "音量");
        let (master, _) = level(PlayParams::default(), (0.5, 1.0, 1.0));
        assert!((master - full * 0.5).abs() < 1e-5, "全体の音量");
        let (music_off, _) = level(PlayParams::default(), (1.0, 1.0, 0.0));
        assert!((music_off - full).abs() < 1e-5, "音楽の音量は効果音に効かない");
        let (music, _) =
            level(PlayParams { bus: Bus::Music, ..Default::default() }, (1.0, 1.0, 0.0));
        assert_eq!(music, 0.0, "音楽の音量 0 なら音楽は無音");
    }

    #[test]
    fn changing_the_rate_keeps_sound_speed() {
        // 8 kHz の音を 16 kHz の出力で鳴らすと、同じ長さ (秒) で鳴り終わる。
        let sound = Sound::new(8_000, vec![0.5; 800]);
        let mut mixer = Mixer::new(8_000);
        mixer.set_rate(16_000);
        assert_eq!(mixer.rate(), 16_000);
        mixer.play(&sound, PlayParams::default());
        let mut out = vec![0.0; 2 * 1_000];
        mixer.render(&mut out);
        assert!(mixer.is_playing(VoiceId(1)), "0.1 秒の音は 1600 フレームで鳴り終わる");
        mixer.render(&mut out);
        assert_eq!(mixer.active(), 0);
        mixer.play(&sound, PlayParams::default());
        mixer.set_rate(0);
        assert_eq!((mixer.rate(), mixer.active()), (1, 0), "レートを変えると鳴っている音は止まる");
    }

    #[test]
    fn pitch_changes_the_speed() {
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let sound = Sound::new(RATE, ramp);
        let mut slow = Mixer::new(RATE);
        slow.play(&sound, PlayParams { pitch: 0.5, ..Default::default() });
        let mut fast = Mixer::new(RATE);
        fast.play(&sound, PlayParams { pitch: 2.0, ..Default::default() });
        let (mut a, mut b) = (vec![0.0; 2 * 20], vec![0.0; 2 * 20]);
        slow.render(&mut a);
        fast.render(&mut b);
        assert!(b[2 * 19] > a[2 * 19] * 3.0, "高いほど先へ進む");
        // 倍の高さは、半分の長さで鳴り終わる。
        let mut mixer = Mixer::new(RATE);
        mixer.play(&sound, PlayParams { pitch: 2.0, ..Default::default() });
        let mut out = vec![0.0; 2 * 60];
        mixer.render(&mut out);
        assert_eq!(mixer.active(), 0);
    }

    #[test]
    fn voices_are_limited_and_values_are_cleaned() {
        let mut mixer = Mixer::new(RATE);
        let sound = Sound::new(RATE, vec![0.1; 10_000]);
        for _ in 0..MAX_VOICES {
            assert!(mixer.play(&sound, PlayParams::default()).is_some());
        }
        assert!(mixer.play(&sound, PlayParams::default()).is_none(), "上限");
        assert!(mixer.play(&Sound::new(RATE, Vec::new()), PlayParams::default()).is_none());
        mixer.stop_all();
        assert_eq!(mixer.active(), 0);
        let nasty =
            PlayParams { volume: f32::NAN, pan: f32::INFINITY, pitch: -3.0, ..Default::default() };
        let id = mixer.play(&sound, nasty).unwrap();
        let mut out = vec![0.0; 2 * 10];
        mixer.render(&mut out);
        assert!(out.iter().all(|v| v.is_finite()), "NaN・無限大は出さない");
        assert!(mixer.is_playing(id));
        // 大きな音は -1..1 に収める。
        let loud = Sound::new(RATE, vec![1.0; 100]);
        let mut mixer = Mixer::new(RATE);
        for _ in 0..10 {
            mixer.play(&loud, PlayParams { volume: 2.0, ..Default::default() });
        }
        mixer.render(&mut out);
        assert!(peak(&out) <= 1.0);
    }

    fn wav(bits: u16, channels: u16, tag: u16, data: &[u8]) -> Vec<u8> {
        let rate: u32 = 8_000;
        let block = channels * bits / 8;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data.len() as u32).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(tag.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * u32::from(block)).to_le_bytes());
        v.extend(block.to_le_bytes());
        v.extend(bits.to_le_bytes());
        v.extend(b"data");
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(data);
        v
    }

    #[test]
    fn wav_files_are_read() {
        let mono16 = [16384i16, -16384, 0].iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
        let sound = Sound::from_wav(&wav(16, 1, 1, &mono16)).unwrap();
        assert_eq!(sound.rate, 8_000);
        assert_eq!(&*sound.samples, &[0.5, -0.5, 0.0]);
        // ステレオは平均する。
        let stereo =
            [16384i16, 0, 0, -16384].iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
        let sound = Sound::from_wav(&wav(16, 2, 1, &stereo)).unwrap();
        assert_eq!(&*sound.samples, &[0.25, -0.25]);
        let eight = Sound::from_wav(&wav(8, 1, 1, &[128, 255, 0])).unwrap();
        assert!(
            (eight.samples[0]).abs() < 1e-6 && eight.samples[1] > 0.99 && eight.samples[2] < -0.99
        );
        let float = [0.5f32, -2.0].iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<_>>();
        let float = Sound::from_wav(&wav(32, 1, 3, &float)).unwrap();
        assert_eq!(&*float.samples, &[0.5, -1.0], "float は -1..1 に収める");
    }

    #[test]
    fn bad_wav_files_are_rejected() {
        assert_eq!(Sound::from_wav(b"nope"), Err(WavError::Malformed));
        assert_eq!(Sound::from_wav(&[]), Err(WavError::Malformed));
        let good = wav(16, 1, 1, &[0, 0, 0, 0]);
        assert!(Sound::from_wav(&good).is_ok());
        let mut truncated = good.clone();
        truncated.truncate(good.len() - 2);
        assert_eq!(
            Sound::from_wav(&truncated),
            Err(WavError::Malformed),
            "データが途中で切れている"
        );
        assert_eq!(
            Sound::from_wav(&wav(16, 1, 2, &[0, 0])),
            Err(WavError::Unsupported),
            "ADPCM など"
        );
        assert_eq!(
            Sound::from_wav(&wav(16, 6, 1, &[0; 12])),
            Err(WavError::Unsupported),
            "6 チャンネル"
        );
        let no_data = &good[..36];
        assert_eq!(Sound::from_wav(no_data), Err(WavError::Malformed));
    }

    #[test]
    fn rendering_is_deterministic() {
        let render = || {
            let mut mixer = Mixer::new(44_100);
            let boom = Sound::noise(0.3, 22_050, 3, 1.0);
            let beep = Sound::sweep(Shape::Square, 880.0, 220.0, 0.2, 22_050);
            mixer.play(&boom, PlayParams { pan: -0.3, pitch: 0.8, ..Default::default() });
            mixer.play(&beep, PlayParams { pan: 0.6, looped: true, ..Default::default() });
            let mut out = vec![0.0; 2 * 4_000];
            mixer.render(&mut out);
            out
        };
        assert_eq!(render(), render());
    }
}
