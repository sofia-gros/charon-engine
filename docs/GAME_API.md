# ゲームからエンジンを使う

このリポジトリは**ゲームエンジン**です。エンジンは (1) 材質・呪文・杖・敵の**既定の定義**を RON で持ち、
(2) それを読み込み・追加・上書きする **API** を提供します。ゲームはその API を呼んで、
自分の武器・敵・材質を足します。仕様は [10 §7](spec/10-noita-like-game.md)・[07 §6](spec/07-game-layer.md)。

## 定義の層

| 層 | 内容 | 場所 |
|---|---|---|
| エンジンの既定 | 材質・反応・呪文・杖・敵 | `assets/materials.ron`、`reactions.ron`、`spells.ron`、`wands.ron`、`enemies.ron` |
| ゲーム (MOD) | 追加と上書き | 各 MOD のディレクトリの同名ファイル (`spells.ron`・`wands.ron`・`enemies.ron` は省略可) |

MOD は ID の昇順で 1 層ずつ重なります。後の層が勝ちます。

- 追加 (既定): 名前が `MOD の ID:名前` になります。例 `fse_demo:rainbow_shot`。同じ名前が既にあればエラー。
- 上書き: 項目に `replace: true` を書くと、名前をそのまま使って、既にある同名の定義の中身を置き換えます。
  呪文の位置 (`SpellId`) は保つので、すでに作った杖は上書き後の中身で撃てます。置き換える対象がなければエラー。
- 杖・敵が呪文を参照するときは、`MOD の ID:名前` を先に、なければそのままの名前を探します。

```ron
// 自分の MOD の spells.ron
[
    (name: "rainbow_shot", mana_cost: 8, cast_delay: 8, speed: 110, gravity: 2, life: 90,
     damage: 0, spread: 24, payload: Deposit(material: "rainbow_sand_3")),
    // エンジンの既定の bomb を、このゲームでは大きな爆発にする。
    (name: "bomb", replace: true, mana_cost: 30, cast_delay: 30, speed: 96, gravity: 2, life: 120,
     damage: 24, spread: 8, payload: Explode(radius: 8)),
]
```

```ron
// wands.ron
[(name: "starter", spells: ["spark", "rainbow_shot", "bomb"], mana_max: 100, mana_regen: 1)]
```

## Rust から呼ぶ

```rust
use charon_game::mods::{ModDirectory, ModPack};
use charon_game::game::GameState;

// エンジンの既定に、MOD の定義を重ねて読む。
let pack = ModPack::load(&[ModDirectory { path: "assets/mods/my_game".into(), script: false }])?;
let defs = &pack.definitions;

let wand = defs.wand("my_game:starter")?;          // 名前から杖を作る
let shooter = defs.enemy("shooter");               // エンジン既定の敵の種類
let mut game = GameState::from_definitions(defs);  // 体力・杖・弾の状態
```

コードから層を足すこともできます (ファイルを使わない場合)。

```rust
use charon_game::defs::{DefinitionFiles, Definitions};

let mut defs = Definitions::engine_defaults(&materials)?;
defs.layer(
    DefinitionFiles { spells: Some(spells_ron), wands: None, enemies: None },
    &materials,
    Some("my_game"),
)?; // 失敗したときは defs を変えない
```

`charon-app` で動かすときは、`AppOptions` に `definitions` と `player: PlayerLoadout { max_hp, wand }` を渡します。
エンジンの実行部分に呪文名や体力の値は書かれていません。デモ (`charon-demo`) も同じ手順で、
`assets/mods/fse_demo/` の RON と `crates/charon-app/src/demo.rs` の `demo_loadout()` だけがゲーム側です。

## 背景のさらに奥の shader (動き回る虹色)

ゲームは、背景のさらに奥で動き回る shader を足せます (`charon-render` の `DeepBackgroundPass`)。光の影響を受けず、前景・背景のセルの後ろに隠れ、洞窟の空間にだけ見えます。`AppOptions` に渡します。

```rust
use charon_render::DeepBackgroundParams;

let options = AppOptions {
    deep_background: Some(DeepBackgroundParams {
        seed: 7,
        parallax: 0.5,   // カメラの動きに対する動きの割合 (奥ほど小さい)
        grid: 90.0,      // 1 区画の大きさ (セル)。1 区画に最大 1 つ
        density: 0.55,   // 塊のある区画の割合
        min_radius: 6.0,
        max_radius: 16.0,
        drift: 0.02,     // 動き回る速さ (ラジアン / tick)
        roam: 0.3,       // 動き回る範囲 (区画の大きさに対する割合)
        brightness: 1.0,
        empty: [0.012, 0.014, 0.02], // 背景のクリア色。この色の画素にだけ描く
    }),
    ..Default::default()
};
```

アプリを使わず自分で `PassList` を組むときは `DeepBackgroundPass::new` を合成のあと・後処理の前に追加し、返ってくる `DeepBackgroundClock` に毎フレーム `set_tick(world.tick())` を渡します。

## 手と道具 (左手 = 武器、右手 = 道具)

操作は 2 つの手に割り当てます。**左手は武器 (杖、左クリック)、右手は道具 (ワイヤーフック、右クリック)** で、独立して同時に使えます (手は割り当ての概念で、見た目には反映しません)。

```rust
// 入力: fire = 左クリック (武器)、tool = 右クリック (道具)、aim は共通のマウスの方向。
let input = GameInput { fire: true, tool: true, aim: (3, -4) };

// 道具を持たせる (杖を持つキャラクターに)。名前は hooks.ron の定義。
game.equip_hook(player, &defs, "grapple")?;

// 出ているワイヤー (描画用)。
if let Some(rope) = game.rope(player) { /* rope.points() は点列 (セル座標) */ }
```

ワイヤーフックは `hooks.ron` で定義します (エンジンの既定は `assets/hooks.ron`、MOD で追加・`replace: true` で上書き)。

```ron
[(name: "long_grapple", max_length: 320, min_length: 4, speed: 320, segments: 32, gravity: 6,
  iterations: 8, reel_speed: 24, pull_accel: 8, pull_max: 48, cooldown: 20)]
```

右クリックを押した瞬間にワイヤーが出て、先端が地形に着くとプレイヤーを引きます。押している間は巻き取り、離すと外れます。ワイヤーは重力でしなり、地形に食い込みません。`charon-app` では `PlayerLoadout { max_hp, wand, hook: Some("grapple".into()) }` で渡し、`B` キー (または `Draw` の `Paint`) で左ボタンをブラシに切り替えられます。物理は浮動小数点で、同一ビルド・同一プラットフォームでだけ決定的です。

## 敵

敵の**種類**は `enemies.ron` (エンジンの既定とゲームの追加・上書き) で決め、**生成**はゲームが API を呼びます。動き (視界と視線で目標を選び、追う・跳ぶ・撃つ・接触ダメージ) と、倒れた敵の撤去はエンジンが行います。

```ron
// 自分の MOD の enemies.ron
[(name: "bat", width: 6, height: 6, max_hp: 12, walk_speed: 24, jump_speed: 60, sight: 100,
  attack: None, contact_damage: 8)]
```

```rust
// 直接呼ぶ (左上のセルを指定)。失敗したら何も変えない。
let id = game.spawn_enemy(&defs, &mut entities, &world, "my_game:bat", (120, 40))?;

// tick ごとに、プレイヤーの入力のあとで呼ぶ。
let report = game.step(&mut world, &mut physics, &mut entities, &actions, tick, seed)?;
for id in &report.defeated { /* 倒した敵の後始末 */ }
```

`charon-app` で動かすときは `AppOptions::enemies` に `EnemySpawn { name, cell }` を並べます。周囲のチャンクが読み込まれたものから生成されます。

## 同期

`ConnectionInfo::with_definitions(&defs)` は、定義の中身のハッシュ (`Definitions::content_hash`) を接続情報に加えます。
名前が同じでも、上書きで中身が違う相手とは接続できません。
