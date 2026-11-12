# 仕様 01: 核となるデータ

対象クレート: `charon-core`。この文書の型名・定数・フィールド名はそのまま使うこと。

## 1. 座標系

- 単位は「セル」。1 セル = 画面上の 1 ピクセル (拡大前)。
- **x は右が正、y は下が正**。重力は +y 方向。
- ワールド座標は `i32`。

```rust
// crates/charon-core/src/coords.rs
pub const CHUNK_SHIFT: u32 = 6;
pub const CHUNK_SIZE: i32 = 1 << CHUNK_SHIFT;        // 64
pub const CHUNK_MASK: i32 = CHUNK_SIZE - 1;          // 63
pub const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize; // 4096

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct CellPos { pub x: i32, pub y: i32 }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChunkPos { pub x: i32, pub y: i32 }

impl CellPos {
    /// 算術右シフトは負数でも床方向に丸める。割り算を使ってはいけない。
    pub const fn chunk(self) -> ChunkPos {
        ChunkPos { x: self.x >> CHUNK_SHIFT, y: self.y >> CHUNK_SHIFT }
    }
    /// チャンク内の添字 (0..4096)。行優先: `ly * 64 + lx`。
    pub const fn local_index(self) -> usize {
        ((self.y & CHUNK_MASK) * CHUNK_SIZE + (self.x & CHUNK_MASK)) as usize
    }
}
```

必須テスト: `CellPos{x:-1,y:-1}.chunk() == ChunkPos{x:-1,y:-1}`、`local_index() == 4095`。
`CellPos{x:64,y:0}.chunk() == ChunkPos{x:1,y:0}`、`local_index() == 0`。

## 2. セル

1 セルは 8 バイトの `Copy` 型。ヒープを指すフィールドを追加してはいけない。

```rust
// crates/charon-core/src/cell.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[repr(transparent)]
pub struct MaterialId(pub u16);

impl MaterialId {
    pub const EMPTY: MaterialId = MaterialId(0);    // 何もない (空気)
    pub const BOUNDARY: MaterialId = MaterialId(1); // 読み込まれていない領域。動かず壊れない固体
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(C)]
pub struct Cell {
    pub material: MaterialId, // 2 バイト
    pub flags: u8,            // 下の FLAG_* のビット和
    pub shade: u8,            // 色のばらつき。生成時に乱数で決め、以後変えない
    pub temp: i16,            // 温度。単位は摂氏 1 度
    pub vy: i8,               // 落下速度。単位は 1/16 セル毎 tick。気体は負 (上向き)
    pub life: u8,             // 残り寿命。単位は 4 tick。0 は「寿命なし」
}
const _: () = assert!(std::mem::size_of::<Cell>() == 8);

pub const FLAG_PARITY: u8 = 1 << 0;   // この tick で更新済みかの判定に使う (仕様 02 §3)
pub const FLAG_FALLING: u8 = 1 << 1;  // 動いている途中。粉体は仕様 02 §7、液体・気体は仕様 02 §8 の「勢い」
pub const FLAG_FLOW_RIGHT: u8 = 1 << 2; // 液体・気体の水平移動の向き。立っていれば +x
pub const FLAG_BURNING: u8 = 1 << 3;  // 燃焼中 (仕様 03 §4)
pub const FLAG_BODY: u8 = 1 << 4;     // 剛体が書き込んだセル (仕様 05)。保存しない
pub const FLAG_BOUNCED: u8 = 1 << 5;  // 液体・気体が水平移動で 1 回はね返った (仕様 02 §8)
pub const FLAG_SMOOTH: u8 = 1 << 6;   // 滑らかな剛体が書いたセル。セルの描画は描かない (仕様 05 §11)。保存しない
```

`Cell::default()` は「空」(全フィールド 0) である。

## 3. 材質

材質の性質はすべて `Material` に持たせる。シミュレーションのコードに特定の材質名を書いてはいけない
(`EMPTY` と `BOUNDARY` を除く)。

```rust
// crates/charon-core/src/material.rs
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize)]
pub enum Phase { Empty, Solid, Powder, Liquid, Gas }

#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub phase: Phase,
    pub density: u16,          // 大きいほど重い。入れ替わりの判定に使う
    pub color: [u8; 4],        // RGBA
    pub color_variation: u8,   // shade による明るさの振れ幅 (0 = 単色)

    // 移動 (仕様 02)
    pub gravity: u8,           // 毎 tick の vy 増分 (1/16 セル)。既定 4
    pub max_speed: u8,         // |vy| の上限。既定 96、最大 127
    pub friction: u8,          // 粉体: 0 = さらさら (45 度の山)、大きいほど急な山になり崩れにくい
    pub dispersion: u8,        // 液体・気体: 1 tick に水平に進める最大セル数
    pub viscosity: u8,         // 液体: 0 = 水、240 = スライム。移動を見送る確率 (x/256)

    // 熱と変化 (仕様 03)
    pub initial_temp: i16,     // 生成時の温度。既定 20
    pub conductivity: u8,      // 熱の伝わりやすさ。0 = 断熱
    pub melt: Option<Transition>,    // 温度がこれ以上で変化
    pub freeze: Option<Transition>,  // 温度がこれ以下で変化
    pub boil: Option<Transition>,    // 温度がこれ以上で変化
    pub ignite_at: Option<i16>,      // この温度以上で燃え始める
    pub burn_time: u8,               // 燃焼の長さ (life と同じ単位)
    pub burn_into: MaterialId,       // 燃え尽きた後の材質
    pub burn_temp: i16,              // 燃焼中に自分を加熱する目標温度
    pub lifetime: Option<(u8, u8)>,  // 寿命の最小・最大 (life と同じ単位)
    pub decay_into: MaterialId,      // 寿命が尽きた後の材質

    // 剛体 (仕様 05)
    pub rigid_friction: f32,   // 剛体同士・剛体と地形の摩擦係数。既定 0.6
    pub restitution: f32,      // 反発係数。既定 0.1

    // 描画 (仕様 06)
    pub emission: [u8; 3],     // 発光色。光らないなら [0,0,0]
}

#[derive(Clone, Copy, Debug)]
pub struct Transition { pub at: i16, pub into: MaterialId }

pub struct MaterialTable {
    materials: Vec<Material>,                  // 添字 = MaterialId.0
    by_name: std::collections::HashMap<String, MaterialId>,
}
```

`MaterialTable` の API:

| 関数 | 動作 |
|---|---|
| `MaterialTable::from_ron(src: &str) -> Result<Self, MaterialError>` | RON を読む。下の規則に従う |
| `get(&self, id: MaterialId) -> &Material` | 添字で引く。範囲外は `BOUNDARY` を返す |
| `id(&self, name: &str) -> Option<MaterialId>` | 名前から ID |
| `len(&self) -> usize` | 材質数 (組み込みの 2 つを含む) |

読み込みの規則:

1. ID 0 は `empty` (phase `Empty`、density 0、color 透明)、ID 1 は `boundary` (phase `Solid`、
   density 65535)。この 2 つは RON に書かれていなくても必ず先頭に作る。RON に同名があれば上書きではなくエラー。
2. RON の材質は書かれた順に ID 2 から割り当てる。
3. `into` などの参照は名前で書く。全材質を読んだ後で ID に解決する (2 パス)。未定義の名前はエラー。
4. 名前の重複はエラー。材質数が 65535 を超えたらエラー。
5. 省略されたフィールドは上の「既定」の値、既定がないものは 0 / `None` / `EMPTY`。

RON の形式 (抜粋):

```ron
[
    (name: "stone", phase: Solid, density: 2600, color: (110, 110, 115, 255), color_variation: 12,
     conductivity: 40, melt: Some((at: 1200, into: "lava"))),
    (name: "sand", phase: Powder, density: 1600, color: (218, 196, 128, 255), color_variation: 16,
     friction: 30, conductivity: 20),
    (name: "water", phase: Liquid, density: 1000, color: (48, 110, 220, 190), dispersion: 5,
     conductivity: 80, boil: Some((at: 100, into: "steam")), freeze: Some((at: 0, into: "ice"))),
    (name: "slime", phase: Liquid, density: 1100, color: (120, 200, 80, 230), dispersion: 1,
     viscosity: 240),
    (name: "oil", phase: Liquid, density: 800, color: (60, 50, 30, 220), dispersion: 3,
     viscosity: 60, ignite_at: Some(200), burn_time: 30, burn_temp: 700),
    (name: "steam", phase: Gas, density: 1, color: (200, 200, 210, 90), dispersion: 3,
     freeze: Some((at: 80, into: "water")), initial_temp: 110),
]
```

実際のデータは `assets/materials.ron` にある (全 15 材質。この文書の作成時に、下の構造体で読めることを確認済み)。

RON はまず次の構造体に読み、その後 `Material` に変換する (参照の名前を ID に解決するため)。
**この定義をそのまま使うこと。**

```rust
#[derive(serde::Deserialize)]
struct RawTransition { at: i16, into: String }

fn d_gravity() -> u8 { 4 }
fn d_max_speed() -> u8 { 96 }
fn d_temp() -> i16 { 20 }
fn d_empty() -> String { "empty".to_string() }
fn d_friction() -> f32 { 0.6 }
fn d_restitution() -> f32 { 0.1 }

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMaterial {
    name: String,
    phase: Phase,
    #[serde(default)] density: u16,
    color: [u8; 4],
    #[serde(default)] color_variation: u8,
    #[serde(default = "d_gravity")] gravity: u8,
    #[serde(default = "d_max_speed")] max_speed: u8,
    #[serde(default)] friction: u8,
    #[serde(default)] dispersion: u8,
    #[serde(default)] viscosity: u8,
    #[serde(default = "d_temp")] initial_temp: i16,
    #[serde(default)] conductivity: u8,
    #[serde(default)] melt: Option<RawTransition>,
    #[serde(default)] freeze: Option<RawTransition>,
    #[serde(default)] boil: Option<RawTransition>,
    #[serde(default)] ignite_at: Option<i16>,
    #[serde(default)] burn_time: u8,
    #[serde(default = "d_empty")] burn_into: String,
    #[serde(default)] burn_temp: i16,
    #[serde(default)] lifetime: Option<(u8, u8)>,
    #[serde(default = "d_empty")] decay_into: String,
    #[serde(default = "d_friction")] rigid_friction: f32,
    #[serde(default = "d_restitution")] restitution: f32,
    #[serde(default)] emission: [u8; 3],
}
```

読み込み時の追加の検査 (違反はエラー): `max_speed <= 127`、`lifetime` は `1 <= min <= max`、
`ignite_at` が `Some` なら `burn_time >= 1`。

`MaterialTable` には、テストとアプリが使う補助を 1 つ用意する:
`MaterialTable::builtin() -> Result<Self, MaterialError>` は `include_str!` で `assets/materials.ron` を埋め込んで読む。

依存クレートは `serde` (derive) と `ron`。導入時点の最新安定版を `cargo add` で入れる。

## 4. 決定的な乱数

状態を持つ乱数生成器を使ってはいけない。乱数は必ず次の関数から得る。
入力が同じなら、スレッド数や実行順に関係なく同じ値になる。

```rust
// crates/charon-core/src/rng.rs
/// `salt` は呼び出し箇所ごとに異なる定数にする (同じセルで複数回引くため)。
pub fn hash(seed: u64, tick: u64, x: i32, y: i32, salt: u32) -> u32 {
    let mut z = seed
        ^ tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ ((x as u32 as u64) << 32 | (y as u32 as u64)).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (salt as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

/// 確率 `p / 256` で true。
pub fn chance(h: u32, p: u8) -> bool { (h & 0xFF) < p as u32 }
```

salt の定数は `rng.rs` に `pub const SALT_*: u32` として連番で定義し、重複させない。

```rust
pub const SALT_SHADE: u32 = 1;      // make_cell: 色のばらつき
pub const SALT_LIFE: u32 = 2;       // make_cell / convert: 寿命
pub const SALT_FLOW: u32 = 3;       // make_cell: 流れる向きの初期値
pub const SALT_SCAN: u32 = 4;       // 行を走査する向き
pub const SALT_DRAG: u32 = 5;       // 液体の中を沈むときの抵抗
pub const SALT_FALL: u32 = 6;       // 落下の歩数の端数
pub const SALT_DIAG: u32 = 7;       // 粉体が斜めに落ちる向き
pub const SALT_WAKE: u32 = 8;       // 隣の粉体を起こすか
pub const SALT_VISC: u32 = 9;       // 液体・気体が移動を見送るか
pub const SALT_REACT_DIR: u32 = 10; // 反応を試す方向
pub const SALT_REACT: u32 = 11;     // 反応が起きるか
pub const SALT_FIRE_DIR: u32 = 12;  // 炎を出す方向
pub const SALT_FIRE: u32 = 13;      // 炎を出すか
pub const SALT_STICK: u32 = 14;     // 着地した粉体がその場に止まるか
pub const SALT_TERRAIN: u32 = 15;   // 地表の値ノイズ (M5)
pub const SALT_CAVE: u32 = 16;      // 洞窟の値ノイズ (M5)
```

新しく乱数が必要になったら、この一覧の末尾に足す。

## 5. チャンクとワールド

```rust
// crates/charon-core/src/chunk.rs
/// チャンク内座標 (0..64) の矩形。min と max は両端を含む。空は `None`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect { pub min_x: u8, pub min_y: u8, pub max_x: u8, pub max_y: u8 }

pub struct Chunk {
    pub cells: Box<[Cell; CHUNK_AREA]>,
    pub dirty: Option<Rect>,       // この tick で処理する範囲
    pub next_dirty: Option<Rect>,  // 次の tick で処理する範囲 (書き込みのたびに広げる)
    pub collider_dirty: bool,      // 固体の形が変わった (仕様 05)
    pub render_dirty: Option<Rect>,// 描画側へ再送が必要な範囲 (仕様 06)
}
```

```rust
// crates/charon-core/src/world.rs
pub struct World {
    chunks: std::collections::HashMap<ChunkPos, Box<Chunk>>,
    materials: std::sync::Arc<MaterialTable>,
    seed: u64,
    tick: u64,
}
```

`World` の API:

| 関数 | 動作 |
|---|---|
| `new(materials: Arc<MaterialTable>, seed: u64) -> Self` | 空のワールド |
| `insert_chunk(&mut self, pos: ChunkPos)` | 全セルが空のチャンクを追加。既にあれば何もしない |
| `get(&self, pos: CellPos) -> Cell` | チャンクがなければ `material: BOUNDARY` のセルを返す |
| `set(&mut self, pos: CellPos, cell: Cell)` | 書き込み、§6 の「書き込み時の処理」を行う。材質が変わったら広い帯も起こす (仕様 02 §9 の 5)。チャンクがなければ何もしない |
| `keep_awake(&mut self, pos: CellPos)` | §6 の `keep_awake` |
| `make_cell(&self, material: MaterialId, pos: CellPos) -> Cell` | 新しいセルを作る (下記) |
| `step(&mut self)` | 1 tick 進める (仕様 02) |
| `tick(&self) -> u64` | 現在の tick |
| `awake_chunks(&self) -> usize` | `dirty` または `next_dirty` が `Some` のチャンクの数。0 なら世界全体が休眠している |
| `chunk_positions(&self) -> Vec<ChunkPos>` | 読み込まれているチャンクの位置。昇順に並べて返す |
| `state_hash(&self) -> u64` | 全チャンクを `ChunkPos` の昇順に並べ、全セルの 8 バイトを FNV-1a 64 でハッシュ。`FLAG_PARITY` のビットは 0 にしてから混ぜる |

`make_cell` の規則: `shade = hash(seed, tick, x, y, SALT_SHADE) as u8`、`temp = initial_temp`、
`vy = 0`、`flags = FLAG_FALLING` (置いた直後は動ける状態)。`FLAG_FLOW_RIGHT` は
`hash(.., SALT_FLOW) & 1` で決める (置いた液体が左右均等に広がるようにする)。`lifetime` が `Some((min, max))` なら
`life = min + hash(.., SALT_LIFE) % (max - min + 1)`、`None` なら 0。

## 6. 書き込み時の処理 (dirty の規則)

セルを書き換える関数は必ず次を行う。直接 `cells[i] = ...` と書いてよいのはこの関数の中だけである。

1. 書き込んだセルを中心に上下左右 2 セル広げた 5×5 の範囲を、該当する各チャンクの `next_dirty` に加える
   (範囲がチャンク境界をまたぐ場合は隣のチャンクにも加える)。
2. 書き込んだチャンクの `render_dirty` にそのセルを加える。
3. 書き換えの前後どちらかの材質が `Solid`、または動いていない `Powder` (`FLAG_FALLING` なし) なら
   `collider_dirty = true`。

`keep_awake(pos)`: セルの値は変えずに 1 だけを行う。「今回は動かなかったが次は動くかもしれない」
セルが必ず呼ぶ (仕様 02、03 の各所で指示する)。
