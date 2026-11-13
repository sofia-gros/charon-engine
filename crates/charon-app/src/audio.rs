//! 仕様 12: ゲームの出来事 (ジャンプ・撃つ・ワイヤーが着く・掘る・拾う) を効果音にする。
//!
//! 出来事は、前のフレームと今のフレームの状態 (`Snapshot`) の差から決める (描画に依存しない純粋な計算)。
//! 音は `charon-audio` のミキサーで混ぜる。音声デバイスへの出力 (`AudioOutput`) は、デバイス用の依存クレートで実装して
//! `Audio::attach_output` で渡す (このクレートの既定は出力なし = 混ぜるだけ)。

use std::sync::{Arc, Mutex};

use charon_audio::{AudioOutput, Bus, Mixer, PlayParams, Shape, Sound, VoiceId};

/// 混ぜる音の出力レート (Hz)。
pub const RATE: u32 = 44_100;

/// 音になる出来事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundEvent {
    Jump,
    Shoot,
    HookAnchor,
    Pickup,
    DigStart,
    DigStop,
}

/// 出来事を決めるための、1 フレームの状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub grounded: bool,
    pub vy: i16,
    /// 飛んでいる弾の数。
    pub projectiles: usize,
    /// ワイヤーが着いているか。
    pub anchored: bool,
    /// 掘る道具が掘れる所に当たっているか。
    pub digging: bool,
    /// 持ち物の全部の数。
    pub items: u64,
}

/// 前のフレームから今のフレームへの出来事 (決まった順)。
pub fn events(prev: &Snapshot, now: &Snapshot) -> Vec<SoundEvent> {
    let mut out = Vec::new();
    if prev.grounded && !now.grounded && now.vy < 0 {
        out.push(SoundEvent::Jump);
    }
    if now.projectiles > prev.projectiles {
        out.push(SoundEvent::Shoot);
    }
    if !prev.anchored && now.anchored {
        out.push(SoundEvent::HookAnchor);
    }
    if now.items > prev.items {
        out.push(SoundEvent::Pickup);
    }
    match (prev.digging, now.digging) {
        (false, true) => out.push(SoundEvent::DigStart),
        (true, false) => out.push(SoundEvent::DigStop),
        _ => {}
    }
    out
}

/// 効果音 (計算で作る。画像のアセットがないデモ用)。ゲームは自分の `Sound` (WAV から読む) に差し替えられる。
#[derive(Clone, Debug)]
pub struct SoundBank {
    pub jump: Sound,
    pub shoot: Sound,
    pub anchor: Sound,
    pub pickup: Sound,
    /// 掘っている間、繰り返す。
    pub dig: Sound,
}

impl SoundBank {
    pub fn procedural(rate: u32) -> Self {
        Self {
            jump: Sound::sweep(Shape::Sine, 280.0, 620.0, 0.12, rate),
            shoot: Sound::sweep(Shape::Saw, 900.0, 180.0, 0.16, rate),
            anchor: Sound::noise(0.07, rate, 11, 1.0),
            pickup: Sound::sweep(Shape::Sine, 700.0, 1400.0, 0.09, rate),
            dig: Sound::sweep(Shape::Square, 110.0, 95.0, 0.25, rate),
        }
    }
}

impl SoundBank {
    /// 置き場の `jump` / `shoot` / `anchor` / `pickup` / `dig` (拡張子 `.wav` か `.ogg`、`sounds/` の下) を読み、
    /// なければ、または読めなければ (報告して) 生成した音にする。
    pub fn from_assets(assets: &mut crate::assets::Assets, rate: u32) -> Self {
        let fallback = Self::procedural(rate);
        let mut load = |name: &str, default: Sound| {
            for ext in ["wav", "ogg"] {
                let file = format!("sounds/{name}.{ext}");
                match assets.sound(&file) {
                    Ok(sound) => return sound,
                    Err(crate::assets::AssetError::File(_)) => {}
                    Err(err) => eprintln!("{file} を読めません: {err}"),
                }
            }
            default
        };
        Self {
            jump: load("jump", fallback.jump.clone()),
            shoot: load("shoot", fallback.shoot.clone()),
            anchor: load("anchor", fallback.anchor.clone()),
            pickup: load("pickup", fallback.pickup.clone()),
            dig: load("dig", fallback.dig.clone()),
        }
    }
}

/// ミキサーを、音声デバイスの別スレッドと共有する。ロックが毒になっていても (別スレッドが落ちても) 使い続ける。
type SharedMixer = Arc<Mutex<Mixer>>;

fn lock(mixer: &SharedMixer) -> std::sync::MutexGuard<'_, Mixer> {
    mixer.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 混ぜたステレオ (左右交互) を、デバイスのチャンネル数に並べ直す: モノラルは平均、3 チャンネル以上は左右だけで残りは無音。
pub fn fill_channels(stereo: &[f32], channels: usize, out: &mut [f32]) {
    let channels = channels.max(1);
    for (frame, pair) in out.chunks_mut(channels).zip(stereo.as_chunks::<2>().0) {
        match frame.len() {
            1 => frame[0] = (pair[0] + pair[1]) * 0.5,
            _ => {
                frame[0] = pair[0];
                frame[1] = pair[1];
                for extra in &mut frame[2..] {
                    *extra = 0.0;
                }
            }
        }
    }
}

/// 出来事を音にして、混ぜる。
pub struct Audio {
    mixer: SharedMixer,
    bank: SoundBank,
    previous: Snapshot,
    dig_voice: Option<VoiceId>,
    output: Option<Box<dyn AudioOutput>>,
    /// 音声デバイスの出力ストリーム (開いている間、鳴る)。
    #[cfg(not(target_arch = "wasm32"))]
    device: Option<cpal::Stream>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Audio").field("active", &lock(&self.mixer).active()).finish_non_exhaustive()
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::new()
    }
}

impl Audio {
    pub fn new() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer::new(RATE))),
            bank: SoundBank::procedural(RATE),
            previous: Snapshot::default(),
            dig_voice: None,
            output: None,
            #[cfg(not(target_arch = "wasm32"))]
            device: None,
        }
    }

    pub fn set_volumes(&mut self, master: f32, sfx: f32, music: f32) {
        lock(&self.mixer).set_volumes(master, sfx, music);
    }

    /// 効果音を差し替える。
    pub fn set_bank(&mut self, bank: SoundBank) {
        self.bank = bank;
    }

    /// 音声デバイスへの出力を渡す (なければ混ぜるだけ)。
    pub fn attach_output(&mut self, output: Box<dyn AudioOutput>) {
        self.output = Some(output);
    }

    pub fn active_voices(&self) -> usize {
        lock(&self.mixer).active()
    }

    /// 既定の音声デバイスを開いて、鳴らし始める (仕様 12 §5)。デバイスのサンプルレートにミキサーを合わせる。
    /// デバイスがない・開けないときは `Err` で、そのまま混ぜるだけで続けられる。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn start_device(&mut self) -> Result<(), String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("音声の出力デバイスがありません")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate();
        let channels = usize::from(config.channels());
        {
            let mut mixer = lock(&self.mixer);
            mixer.set_rate(rate);
        }
        let shared = Arc::clone(&self.mixer);
        let mut scratch: Vec<f32> = Vec::new();
        let error = |e: cpal::Error| eprintln!("音声の出力でエラー: {e}");
        let stream_config: cpal::StreamConfig = config.into();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                stream_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    render_into(&shared, &mut scratch, channels, data);
                },
                error,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                stream_config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mut floats = vec![0.0f32; data.len()];
                    render_into(&shared, &mut scratch, channels, &mut floats);
                    for (out, value) in data.iter_mut().zip(floats) {
                        *out = (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                    }
                },
                error,
                None,
            ),
            other => return Err(format!("対応しないサンプル形式です: {other}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        self.device = Some(stream);
        Ok(())
    }

    /// 音声デバイスを開いているか。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn device_open(&self) -> bool {
        self.device.is_some()
    }

    /// 今のフレームの状態から出来事を決めて、音を鳴らす。
    pub fn update(&mut self, now: Snapshot) -> Vec<SoundEvent> {
        let events = events(&self.previous, &now);
        let mut mixer = lock(&self.mixer);
        for event in &events {
            let play = |mixer: &mut Mixer, sound: &Sound, volume: f32| {
                mixer.play(sound, PlayParams { volume, bus: Bus::Sfx, ..Default::default() })
            };
            match event {
                SoundEvent::Jump => {
                    play(&mut mixer, &self.bank.jump, 0.5);
                }
                SoundEvent::Shoot => {
                    play(&mut mixer, &self.bank.shoot, 0.6);
                }
                SoundEvent::HookAnchor => {
                    play(&mut mixer, &self.bank.anchor, 0.7);
                }
                SoundEvent::Pickup => {
                    play(&mut mixer, &self.bank.pickup, 0.4);
                }
                SoundEvent::DigStart => {
                    self.dig_voice = mixer.play(
                        &self.bank.dig,
                        PlayParams {
                            volume: 0.35,
                            looped: true,
                            bus: Bus::Sfx,
                            ..Default::default()
                        },
                    );
                }
                SoundEvent::DigStop => {
                    if let Some(id) = self.dig_voice.take() {
                        mixer.stop(id);
                    }
                }
            }
        }
        self.previous = now;
        events
    }

    /// 出来事を指定して鳴らす (ゲームのロジックが頼んだ音)。
    pub fn play_events(&mut self, events: &[SoundEvent]) {
        let mut mixer = lock(&self.mixer);
        for event in events {
            let (sound, volume) = match event {
                SoundEvent::Jump => (&self.bank.jump, 0.5),
                SoundEvent::Shoot => (&self.bank.shoot, 0.6),
                SoundEvent::HookAnchor => (&self.bank.anchor, 0.7),
                SoundEvent::Pickup => (&self.bank.pickup, 0.4),
                // 掘る音は続くので、状態の変化で鳴らす。ここでは扱わない。
                SoundEvent::DigStart | SoundEvent::DigStop => continue,
            };
            mixer.play(sound, PlayParams { volume, bus: Bus::Sfx, ..Default::default() });
        }
    }

    /// `out` (ステレオ) に混ぜる。
    pub fn render(&mut self, out: &mut [f32]) {
        lock(&self.mixer).render(out);
    }

    /// 出力があれば、1 フレーム (1/60 秒) 分を混ぜて渡す。
    pub fn pump(&mut self) {
        let Some(output) = self.output.as_mut() else { return };
        let frames = (u64::from(output.rate()) / 60) as usize;
        let mut buffer = vec![0.0; frames * 2];
        lock(&self.mixer).render(&mut buffer);
        output.submit(&buffer);
    }
}

/// デバイスのコールバックから呼ぶ: ミキサーで混ぜて、デバイスのチャンネル数に並べる。
#[cfg(not(target_arch = "wasm32"))]
fn render_into(mixer: &SharedMixer, scratch: &mut Vec<f32>, channels: usize, out: &mut [f32]) {
    let frames = out.len() / channels.max(1);
    scratch.resize(frames * 2, 0.0);
    lock(mixer).render(scratch);
    fill_channels(scratch, channels, out);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // テストの前提が崩れた場合はその場で失敗させる。
mod tests {
    use super::*;

    fn standing() -> Snapshot {
        Snapshot { grounded: true, ..Default::default() }
    }

    #[test]
    fn events_follow_state_changes() {
        let idle = standing();
        assert!(events(&idle, &idle).is_empty(), "変わらなければ何も起きない");
        let jumping = Snapshot { grounded: false, vy: -96, ..idle };
        assert_eq!(events(&idle, &jumping), [SoundEvent::Jump]);
        let falling = Snapshot { grounded: false, vy: 10, ..idle };
        assert!(events(&idle, &falling).is_empty(), "落ちただけでは跳ばない");
        assert!(events(&jumping, &jumping).is_empty(), "跳んでいる間は 1 度だけ");
        let shot = Snapshot { projectiles: 1, ..idle };
        assert_eq!(events(&idle, &shot), [SoundEvent::Shoot]);
        assert!(
            events(&shot, &Snapshot { projectiles: 0, ..idle }).is_empty(),
            "弾が消えても鳴らない"
        );
        let hooked = Snapshot { anchored: true, ..idle };
        assert_eq!(events(&idle, &hooked), [SoundEvent::HookAnchor]);
        assert_eq!(events(&idle, &Snapshot { items: 3, ..idle }), [SoundEvent::Pickup]);
        let digging = Snapshot { digging: true, ..idle };
        assert_eq!(events(&idle, &digging), [SoundEvent::DigStart]);
        assert_eq!(events(&digging, &idle), [SoundEvent::DigStop]);
        // 同時の出来事は決まった順 (ジャンプ → 撃つ → ワイヤー → 拾う → 掘る)。
        let busy = Snapshot {
            grounded: false,
            vy: -5,
            projectiles: 2,
            anchored: true,
            items: 9,
            digging: true,
        };
        assert_eq!(
            events(&idle, &busy),
            [
                SoundEvent::Jump,
                SoundEvent::Shoot,
                SoundEvent::HookAnchor,
                SoundEvent::Pickup,
                SoundEvent::DigStart
            ]
        );
    }

    #[test]
    fn events_make_sounds_and_the_dig_loop_stops() {
        let mut audio = Audio::new();
        audio.update(standing());
        assert_eq!(audio.active_voices(), 0);
        audio.update(Snapshot { projectiles: 1, ..standing() });
        assert_eq!(audio.active_voices(), 1, "撃つと鳴る");
        let digging = Snapshot { digging: true, ..standing() };
        audio.update(digging);
        assert_eq!(audio.active_voices(), 2, "掘ると、繰り返す音が加わる");
        let mut out = vec![0.0; 2 * 4_410];
        audio.render(&mut out);
        assert!(out.iter().any(|v| *v != 0.0), "音が出る");
        // 掘る音は繰り返す: 1 秒たっても鳴っている。
        for _ in 0..10 {
            audio.render(&mut out);
        }
        assert!(audio.active_voices() >= 1);
        audio.update(standing());
        assert_eq!(audio.active_voices(), 0, "掘るのをやめたら止まる (ほかの音は鳴り終わっている)");
    }

    #[test]
    fn volumes_silence_the_effects() {
        let mut audio = Audio::new();
        audio.set_volumes(0.0, 1.0, 1.0);
        audio.update(standing());
        audio.update(Snapshot { projectiles: 1, ..standing() });
        let mut out = vec![0.0; 2 * 2_000];
        audio.render(&mut out);
        assert!(out.iter().all(|v| *v == 0.0), "全体の音量 0 は無音");
    }

    struct Recorder {
        frames: std::rc::Rc<std::cell::RefCell<usize>>,
    }

    impl AudioOutput for Recorder {
        fn rate(&self) -> u32 {
            RATE
        }

        fn submit(&mut self, interleaved: &[f32]) {
            *self.frames.borrow_mut() += interleaved.len() / 2;
        }
    }

    #[test]
    fn an_attached_output_receives_one_frame_of_audio_per_pump() {
        let frames = std::rc::Rc::new(std::cell::RefCell::new(0));
        let mut audio = Audio::new();
        audio.pump(); // 出力がなくても何も起きない。
        audio.attach_output(Box::new(Recorder { frames: frames.clone() }));
        for _ in 0..60 {
            audio.pump();
        }
        assert_eq!(*frames.borrow(), 60 * (RATE as usize / 60), "1 秒ぶん");
    }

    #[test]
    fn the_procedural_bank_has_sounds_of_sensible_lengths() {
        let bank = SoundBank::procedural(RATE);
        for (name, sound, lo, hi) in [
            ("jump", &bank.jump, 0.05, 0.3),
            ("shoot", &bank.shoot, 0.05, 0.4),
            ("anchor", &bank.anchor, 0.02, 0.2),
            ("pickup", &bank.pickup, 0.03, 0.3),
            ("dig", &bank.dig, 0.1, 1.0),
        ] {
            let seconds = sound.seconds();
            assert!((lo..=hi).contains(&seconds), "{name}: {seconds}");
            assert!(sound.samples.iter().any(|v| v.abs() > 0.1), "{name} は無音ではない");
        }
    }

    /// 効果音は、置き場のファイルがあればそれ、なければ・壊れていれば生成した音になる。
    #[test]
    fn sound_bank_falls_back_to_generated_sounds() {
        let base = std::env::temp_dir().join(format!("charon-bank-{}", std::process::id()));
        std::fs::create_dir_all(base.join("sounds")).unwrap();
        std::fs::write(base.join("sounds").join("jump.ogg"), b"OggS broken").unwrap();
        let mut assets = crate::assets::Assets::new();
        assets.add_root(&base);
        let bank = SoundBank::from_assets(&mut assets, RATE);
        let plain = SoundBank::procedural(RATE);
        assert_eq!(bank.jump.samples.len(), plain.jump.samples.len(), "壊れたファイルは生成した音");
        assert_eq!(bank.dig.samples.len(), plain.dig.samples.len(), "ないものは生成した音");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn channels_are_arranged_for_the_device() {
        let stereo = [0.2, 0.6, -0.4, 0.4];
        let mut mono = [9.0; 2];
        fill_channels(&stereo, 1, &mut mono);
        assert_eq!(mono, [0.4, 0.0]);
        let mut two = [9.0; 4];
        fill_channels(&stereo, 2, &mut two);
        assert_eq!(two, stereo);
        let mut six = [9.0; 12];
        fill_channels(&stereo, 6, &mut six);
        assert_eq!(&six[..6], &[0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&six[6..], &[-0.4, 0.4, 0.0, 0.0, 0.0, 0.0]);
    }

    /// 音声デバイスがあれば開いて、効果音を鳴らしても落ちない。なければ `Err` で、混ぜるだけで続けられる。
    /// 実際に聞こえるかは、この検査では確かめられない (耳で確かめる)。
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_audio_device_opens_or_reports_why_not() {
        let mut audio = Audio::new();
        match audio.start_device() {
            Ok(()) => {
                assert!(audio.device_open());
                audio.update(Snapshot { grounded: true, ..Default::default() });
                audio.update(Snapshot { projectiles: 1, grounded: true, ..Default::default() });
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(why) => {
                assert!(!why.is_empty());
                assert!(!audio.device_open());
            }
        }
        // どちらでも、混ぜるのは続けられる。
        audio.update(Snapshot { items: 5, grounded: true, ..Default::default() });
        let mut out = vec![0.0; 2 * 1_000];
        audio.render(&mut out);
    }
}
