# 仕様 12: 音 (M15)

音は 2 層に分ける。**混ぜる部分** (`charon-audio`) は依存クレートなしで、画面・デバイスなしで試せる。**デバイスへの出力** は、デバイス用の依存クレート (候補: `cpal`) が要るので、利用者の許可を得るまで実装しない (`AudioOutput` の形だけ決めてある)。

## 1. `charon-audio` (依存なし)

- `Sound { rate, samples: Arc<[f32]> }`: モノラル、-1.0..=1.0。
  - 生成: `tone(shape, hz, seconds, rate)` (正弦・矩形・のこぎり、最後の 1/8 で 0 へ)、`noise(seconds, rate, seed, decay)` (xorshift32、同じ seed は同じ音)、`sweep(shape, from, to, seconds, rate)` (周波数が変わる音)。
  - 読み込み: `Sound::from_wav(bytes)` — RIFF/WAVE の PCM 8・16・24 ビットと 32 ビット float、モノラルとステレオ (ステレオは平均)。壊れた形・切れたデータ・対応しない形式 (ADPCM・3 チャンネル以上)・1 億サンプル超は `WavError` で拒否する。
- `Mixer::new(rate)`: 同時に `MAX_VOICES = 32` まで。
  - `play(&Sound, PlayParams { volume 0..=2, pan -1..=1, pitch 0.25..=4, looped, bus: Sfx | Music }) -> Option<VoiceId>`: 上限・空の音は `None`。NaN・無限大・範囲外は収める。
  - `stop`・`stop_all`・`is_playing`・`active`、`set_volumes(master, sfx, music)` (0..=1、NaN は 0)。
  - `render(out)`: 左右交互のステレオ `f32` を**上書き**する。線形補間で `pitch` を掛け、定 power のパン (`cos`/`sin`)、`master × bus × volume` の音量、-1.0..=1.0 に収める。鳴り終わった音は取り除き、`looped` は止めるまで繰り返す。同じ入力なら同じ出力。
- `AudioOutput { rate(), submit(interleaved) }`: デバイスへの出力。実装は別 (未実装)。

## 2. ゲームの出来事を効果音にする (`charon-app::audio`)

- 出来事は、前のフレームと今のフレームの `Snapshot { grounded, vy, projectiles, anchored, digging, items }` の差から決める (描画に依存しない純粋な `events`)。`Jump` (接地から離れて `vy < 0`)、`Shoot` (弾が増えた)、`HookAnchor` (ワイヤーが着いた)、`Pickup` (持ち物が増えた)、`DigStart`・`DigStop` (掘れる所に当たり始めた・やめた)。同時の出来事は、この順。
- `SoundBank::procedural(rate)`: 計算で作る効果音 (`jump`・`shoot`・`anchor`・`pickup`・`dig`)。ゲームは自分の `Sound` (WAV) に `Audio::set_bank` で差し替えられる。掘る音は `looped` で、やめると止める。
- `Audio::update(snapshot)` が音を鳴らし、`pump()` が (出力があれば) 1/60 秒ぶんを混ぜて渡す。出力がなければ混ぜるだけで、何も鳴らない。
- 音量は設定 (`Settings::{master_volume, sfx_volume, music_volume}`、既定 1.0・1.0・0.7、0..=1 に収める、NaN は既定) とポーズのメニューのスライダー。

## 3. テスト

| テスト | 確認 |
|---|---|
| `tone_has_the_requested_length_and_pitch` / `noise_is_deterministic_and_decays` | 生成した音の長さ・高さ・決定性・減衰 |
| `mixer_plays_once_and_removes_finished_voices` / `looped_voices_repeat_until_stopped` | 鳴り終わると消える・繰り返しは止めるまで |
| `pan_volume_and_buses_scale_the_output` / `pitch_changes_the_speed` | パン・音量・系統・高さ |
| `voices_are_limited_and_values_are_cleaned` | 同時の上限・不正な値・クリップ |
| `wav_files_are_read` / `bad_wav_files_are_rejected` | WAV の読み込みと拒否 |
| `rendering_is_deterministic` | 同じ入力で同じ出力 |
| `events_follow_state_changes` / `events_make_sounds_and_the_dig_loop_stops` / `volumes_silence_the_effects` | 出来事と音量 (`charon-app`) |
| `an_attached_output_receives_one_frame_of_audio_per_pump` / `the_procedural_bank_has_sounds_of_sensible_lengths` | 出力の受け渡しと効果音 |
| `volumes_are_clamped` | 設定の音量 |

## 4. 音声デバイスへの出力 (`charon-app`, `cpal`)

- `Audio::start_device()` が既定の出力デバイスを開く。ミキサーのレートをデバイスに合わせ (`Mixer::set_rate`)、デバイスのスレッドのコールバックから、共有したミキサーで混ぜる。
- サンプル形式は `f32` と `i16` に対応。チャンネル数は `fill_channels` で並べる (モノラルは平均、3 以上は左右だけ)。
- デバイスがない・開けないときは `Err` を返し、起動は無音で続ける。ミキサーは共有 (`Arc<Mutex>`) で、ロックが毒になっても使い続ける。
- 追加の検査: `channels_are_arranged_for_the_device` / `the_audio_device_opens_or_reports_why_not` / `changing_the_rate_keeps_sound_speed`。実際に聞こえるかは耳で確かめる (未確認)。

## 5. 未実装

- 音楽の再生・ループ区間・フェード、空間音 (位置による音量・パン)、OGG などの圧縮形式。
