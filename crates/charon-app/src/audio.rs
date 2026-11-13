//! 仕様 12: ゲームの出来事 (ジャンプ・撃つ・ワイヤーが着く・掘る・拾う) を効果音にする。
//!
//! 出来事は、前のフレームと今のフレームの状態 (`Snapshot`) の差から決める (描画に依存しない純粋な計算)。
//! 音は `charon-audio` のミキサーで混ぜる。音声デバイスへの出力 (`AudioOutput`) は、デバイス用の依存クレートで実装して
//! `Audio::attach_output` で渡す (このクレートの既定は出力なし = 混ぜるだけ)。

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

/// 出来事を音にして、混ぜる。
pub struct Audio {
    mixer: Mixer,
    bank: SoundBank,
    previous: Snapshot,
    dig_voice: Option<VoiceId>,
    output: Option<Box<dyn AudioOutput>>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Audio").field("active", &self.mixer.active()).finish_non_exhaustive()
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
            mixer: Mixer::new(RATE),
            bank: SoundBank::procedural(RATE),
            previous: Snapshot::default(),
            dig_voice: None,
            output: None,
        }
    }

    pub fn set_volumes(&mut self, master: f32, sfx: f32, music: f32) {
        self.mixer.set_volumes(master, sfx, music);
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
        self.mixer.active()
    }

    /// 今のフレームの状態から出来事を決めて、音を鳴らす。
    pub fn update(&mut self, now: Snapshot) -> Vec<SoundEvent> {
        let events = events(&self.previous, &now);
        for event in &events {
            let play = |mixer: &mut Mixer, sound: &Sound, volume: f32| {
                mixer.play(sound, PlayParams { volume, bus: Bus::Sfx, ..Default::default() })
            };
            match event {
                SoundEvent::Jump => {
                    play(&mut self.mixer, &self.bank.jump, 0.5);
                }
                SoundEvent::Shoot => {
                    play(&mut self.mixer, &self.bank.shoot, 0.6);
                }
                SoundEvent::HookAnchor => {
                    play(&mut self.mixer, &self.bank.anchor, 0.7);
                }
                SoundEvent::Pickup => {
                    play(&mut self.mixer, &self.bank.pickup, 0.4);
                }
                SoundEvent::DigStart => {
                    self.dig_voice = self.mixer.play(
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
                        self.mixer.stop(id);
                    }
                }
            }
        }
        self.previous = now;
        events
    }

    /// `out` (ステレオ) に混ぜる。
    pub fn render(&mut self, out: &mut [f32]) {
        self.mixer.render(out);
    }

    /// 出力があれば、1 フレーム (1/60 秒) 分を混ぜて渡す。
    pub fn pump(&mut self) {
        let Some(output) = self.output.as_mut() else { return };
        let frames = (u64::from(output.rate()) / 60) as usize;
        let mut buffer = vec![0.0; frames * 2];
        self.mixer.render(&mut buffer);
        output.submit(&buffer);
    }
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
}
