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

## 同期

`ConnectionInfo::with_definitions(&defs)` は、定義の中身のハッシュ (`Definitions::content_hash`) を接続情報に加えます。
名前が同じでも、上書きで中身が違う相手とは接続できません。
