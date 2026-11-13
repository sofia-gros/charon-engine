# 仕様 14: ゲームの入口 (`Game` トレイト)

エンジンの上でゲームを作る人が最初に触る API。描画・入力・保存・音・同期はエンジンが受け持ち、ゲームは「毎 tick 何をするか」だけを書く。

## 1. `Game` と `Context`

```rust
pub trait Game: Send {
    fn on_start(&mut self, ctx: &mut Context<'_>) {}          // プレイヤーが生まれた最初の tick に 1 回
    fn on_tick(&mut self, ctx: &mut Context<'_>, events: &[GameEvent]);  // 毎 tick、ゲーム状態を進めたあと
}
```

- `GameLogic::new(game)` に包んで `AppOptions::logic` に渡す。`AppOptions` は `Clone` のままで、`GameLogic` は共有される。
- `Context` は借り物: `world`、`game` (`GameState`)、`entities`、`player: Option<EntityId>`、`tick`。
- `events` はその tick に起きた出来事 (仕様 10 §15)。複数 tick を 1 フレームで進めても、各 tick の分だけが渡る。
- 呼ぶ位置は、エンジンの 1 tick (チャンク読み込み → プレイヤー → ゲーム状態 → 物理) の **あと**。
- ゲームの中で panic しても、ロックが毒になるだけで、次の tick から呼び続ける。

## 2. 演出の依頼

`Context::shake(amount)` (0..=1、足し合わせて 1 で頭打ち、NaN と負は無視) と `Context::play(SoundEvent)` (1 フレームに `SOUNDS_MAX` = 64 まで) は、頼むだけで、描画のフレームの終わりにエンジンがまとめて行う (カメラは仕様 06 §11、音は仕様 12)。シミュレーションの結果には影響しない。

## 3. 決定性の注意

ロジックが世界やゲーム状態を変えるなら、同期 (Lockstep) のために、乱数は `rng::hash`、入力は同期された値だけを使うこと。画面の揺れ・音は描画側なので自由。

## 4. デモ

`charon-app::demo::DemoGame` が使用例: 敵を倒すと大きく、弾が当たると小さく揺らす。

| テスト | 確認 |
|---|---|
| `game_logic_is_started_once_and_ticked_with_events` | プレイヤーが生まれるまで `on_start` を呼ばない、1 回だけ呼ぶ、毎 tick と出来事が渡る、演出が足される |
| `effects_are_clamped_and_bounded` | 揺れは 1 で頭打ち、NaN・負は無視、音は上限つき |
