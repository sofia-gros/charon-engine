# 仕様 07: ゲーム層

対象: M8。実装前の契約。セル・材質・座標と決定性は仕様 01〜03、生成と保存は仕様 04、剛体は仕様 05、描画とプラットフォームは仕様 06 に従う。新しい処理はこれらの順序や保存形式を暗黙に変えない。テストの置き方と命名は `docs/TESTING.md` に従う。

## 1. 導入順と依存

| 節 | 追加するもの | 前提 |
|---|---|---|
| §2 | エンティティとプレイヤー | 仕様 01、02 |
| §3 | バイオームと構造物 | 仕様 04 §4 |
| §4 | 背景レイヤーと建築 | 仕様 01 §6、04 §3、06 §4 |
| §5 | 剛体の温度と爆発 | 仕様 03、05 |
| §6 | MOD のデータ追加とスクリプト | 仕様 01 §3、03 §1 |
| §7 | 入力同期 | §2〜§6 を完了した決定的な tick |
| §8 | Web 対応 | 仕様 04 §2、06 §1〜§7 |

各節はロードマップのタスク順に実装する。後続の節のために空クレートを先に追加しない。プレイヤーの見た目はセル・剛体とは独立したエンティティの描画パスで表し、セルを直接書き換えない。ゲーム層は固定 tick で更新し、描画・壁時計時刻・`HashMap` の反復順に結果を依存させない。

## 2. エンティティとプレイヤー (`charon-game`)

### 2.1 識別、格納、状態

```rust
pub struct EntityId(pub u64); // 1 から単調増加。0 は無効
pub struct Entity {
    pub id: EntityId,
    pub x16: i64, pub y16: i64, // 左上の座標、1/16 セル単位
    pub vx: i16, pub vy: i16,   // 1 tick あたり 1/16 セル単位
    pub width: u8, pub height: u8,
    pub grounded: bool,
}
pub struct EntityWorld {
    // EntityId の昇順で格納・更新する。削除しても ID を再利用しない。
}
```

`EntityWorld::new()`、`spawn(x16, y16, width, height) -> Result<EntityId, EntityError>`、`get(id)`、`remove(id) -> Option<Entity>`、`iter()`、`state_hash() -> u64` を提供する。幅・高さは 1..=32、座標は整数セルへ切り捨てたとき `i32` の範囲内でなければエラー。ID は `checked_add` で払い出し、オーバーフロー時はエラー。`state_hash` は ID、座標、速度、寸法、接地を昇順でバイト列として FNV-1a 64-bit (offset `0xcbf29ce484222325`、prime `0x100000001b3`、各整数はリトルエンディアン) に流す。次の ID も含める。`World::state_hash()` とは別で、M8 の同期ハッシュ (§7) は双方を使う。

### 2.2 入力と移動

```rust
pub struct PlayerInput { pub horizontal: i8, pub jump: bool } // horizontal は -1, 0, 1 のみ
pub fn step_player(entity: &mut Entity, input: PlayerInput, world: &World);
```

プレイヤーは `width = 8`、`height = 16`。入力は tick ごとに 1 回、ID 昇順に適用する。速度・移動は整数のみ:

1. `vx = horizontal * 32`。`grounded && jump` なら `vy = -96`、`grounded = false`。
2. `vy = min(vy + 4, 96)`。上向きの速度はこの更新で減速する。
3. x、その後 y の順に、現在位置から目標位置へ 1/16 セルずつ進める。各段階で、当たり判定にかかるセル全てを検査する。衝突する直前の位置で止め、その軸の速度を 0 にする。y 正方向の衝突なら `grounded = true`、そうでなければ `grounded = false`。入力がない場合も足元の 1/16 セル下を検査して接地を更新する。座標の加算が `i32` セル範囲を超える移動は境界で止める。
4. `world.get(CellPos)` の材質が `BOUNDARY`、あるいは `FLAG_BODY` が立つ、あるいは材質の `phase == Solid` のときだけ衝突する。粉体・液体・気体・空は通り抜ける。エンティティ同士はこの段階では衝突しない。エンティティはワールドのセルを変更しない。

衝突範囲は半開区間 `[x16, x16 + width*16) × [y16, y16 + height*16)` が覆う整数セル。負座標も `div_euclid(16)` を使う。プレイヤーの初期配置に固体が重なる場合は `spawn_player` がエラーを返し、勝手に地形を消さない。`EntityWorld::step_players(inputs, world)` は入力のない ID に中立入力を適用し、存在しない ID、同一 ID の重複、`horizontal` が -1..=1 の範囲外の入力はエラーにする (部分的に更新してはならない)。エンティティはセルと別のレイヤーに描き、色はプレイヤー用の UI 定数とする (材質 ID で判定しない)。

### 2.3 必須テスト

| テスト | 確認 |
|---|---|
| `entity_ids_are_monotonic` | spawn → remove → spawn で ID を再利用せず、昇順に列挙する |
| `entity_hash_is_stable` | 同じ操作列と ID 割り当てでハッシュが等しく、座標または next ID が変わると異なる |
| `player_lands_on_solid` | 落下して固体の床に乗り、何 tick 進めても床を抜けず接地する |
| `player_hits_body_stamp` | `FLAG_BODY` の壁を貫通せず、スタンプを外せば進める |
| `player_jump_and_negative_coords` | ジャンプ後に着地し、負座標の壁もすり抜けない |
| `invalid_input_is_atomic` | 存在しない ID の入力を含むと全エンティティが変わらない |

## 3. バイオームと構造物 (`charon-world`)

`BiomeGenerator` は `ChunkGenerator` を実装する。既存の `NoiseGenerator` は変更しない。`biome(seed, x)` は `hash(seed, 0, x.div_euclid(256), 0, SALT_BIOME) & 1` で 0 を平地、1 を丘とする。平地の地表は既存の `NoiseGenerator` と同じ式、丘では既存の地表値から `12` を引く (y は下向き)。両バイオームとも地表から 8 セルを `dirt`、下を `stone` とし、洞窟の式は仕様 04 §4 のままとする。`SALT_BIOME` は既存 salt と異なる固定値 `0x4249_4f4d`。構造物は各 128 セル幅の帯の中心 `x = band*128 + 64` に候補を 1 個置き、`hash(seed, 0, band, 0, SALT_STRUCTURE) & 7 == 0` のときだけ生成する (`SALT_STRUCTURE = 0x5354_5255`)。構造物は地表上の `wood` の柱 (幅 3、高さ 6)、最上段の `wood` の梁 (幅 9、高さ 1)。同一ワールド座標の生成値だけからどのチャンクにも属するセルを決める。帯は `div_euclid`、構造物がチャンク境界をまたいでも隣を読まず、生成順に依存しない。柱と梁の範囲では洞窟を作らない。材質 ID は生成器に渡す。生成結果は他チャンクを含むワールドに `set` せず、各チャンクを既存の生成 API 経由で組み立てる。

| テスト | 確認 |
|---|---|
| `biome_boundary_is_pure` | 負座標と 256 セル境界を含む生成順の入れ替えで結果が同じ |
| `structure_crosses_chunk_border` | 隣接チャンクを別順で生成しても柱と梁が途切れない |
| `biome_materials_are_named` | ID を並べ替えたテーブルでも、名前で指定した土・石・木になる |

## 4. 背景レイヤーと建築

背景は前景と独立の 64×64 セルのチャンク (`BackgroundChunk`) とし、1 セルは `MaterialId` だけを持つ。`BackgroundWorld` は `ChunkPos` の昇順で反復し、未読み込み位置は `EMPTY` とみなす。`set` は変更があるときだけ描画 dirty を付け、同じ値なら何もしない。物理、前景の休眠、反応、温度には影響しない。建築入力 `BuildOp { pos, layer, material }` の `layer` は `Foreground | Background`。前景では `EMPTY` の位置だけに `world.make_cell(material, pos)` と `World::set` で置き、撤去は指定セルを `World::set(pos, Cell::default())` で空にする。背景では背景 `set` を使う。`BOUNDARY`、`FLAG_BODY` のセルは前景の建築・撤去を拒否する。指定材質 ID が未定義なら変更しないでエラー。描画順は背景色 → 背景レイヤー → 前景セル → エンティティ → 光 → 合成 → 後処理。背景レイヤーは材質パレットの色の RGB に `0.45` を掛け、アルファは保持する。背景の保存は `background.bin` の独立した version 1 形式 (magic `CHBG`、u16 バージョン、昇順のチャンク数 u32、各チャンクの `cx: i32, cy: i32` と 4096 個の材質名表の u16 添字)。材質名は `world.ron` の既存一覧を使い、未知の名前は `EMPTY`、新しいバージョンはエラー、書き込みは一時ファイルから置換する。

| テスト | 確認 |
|---|---|
| `building_does_not_overwrite` | 前景の既存セルと剛体を上書きしない |
| `background_does_not_affect_simulation` | 背景あり・なしで前景の `state_hash` が同じ |
| `background_roundtrip` | 材質順序が変わっても名前で復元し、新版は拒否する |

## 5. 剛体の温度と爆発 (`charon-physics`)

剛体のセルの `temp` は既存の `PixelBody.cells` に保持する。仕様 05 §2 の 2 (セル → 剛体の力) の直後に、各ボディを添字順、各ボディセルを行優先順で処理する。現在スタンプされているセルが隣接する前景セルと、仕様 03 §2 の熱伝導式で熱を交換する。交換の双方は読み取り時の温度を使い、変化した前景セルだけを `World::set` で書き、剛体側は `PixelBody.cells` を更新する。保存は既存 `bodies.bin` の各セルの `temp` を使う。剛体セルに対する相変化と反応は、この節では行わない。温度変化のあるボディは次のスタンプ時に新しい温度を反映する。爆発 `explode(world, physics, center, radius)` の半径は 1..=16、`dx*dx + dy*dy <= radius*radius` に入る位置を y, x の昇順で処理する。前景の非剛体・非境界セルは既存のパーティクル生成経路へ移す (上限で収容できない場合は元のセルを残す)。移したセルは `World::set` で空にする。剛体セルは既存の損傷・分割の経路に通知し、次 tick の分割を使う。爆発の材質生成はしない。パーティクル速度は `(dx,dy)` の各成分を `clamp(-8,8)` した値 (セル/tick) とし、中心セルは `(0,-1)`。温度交換・爆発とも `rng::hash` 以外の乱数を使わない。

| テスト | 確認 |
|---|---|
| `body_heat_is_conserved` | 熱交換前後で対象セルと剛体セルの熱量が一致する |
| `body_heat_roundtrip` | 保存・読み込みで剛体セルの温度が一致する |
| `explosion_conserves_cells` | 前景 + 剛体 + パーティクルの数が破棄なしで一致する |
| `explosion_damages_body` | 爆発の次 tick に接触したボディが分割される |

## 6. MOD のデータとスクリプト

データ MOD は明示したディレクトリの `manifest.ron` と `materials.ron`、`reactions.ron` だけを読む。`manifest.ron` は `id: String` と `version: u32` (1)、`id` は ASCII 英数字・`_`・`-` の 1..=64 文字。ディレクトリ名の昇順で読み、起動時に一度だけ組み込み材質の後ろへ追加する。材質・反応の相互参照は全 MOD の読み込み後に解決する。名前は `id:name` に名前空間化し、組み込みと MOD 同士の重複はエラー。読み込み失敗は MOD だけを無視せず、起動全体をエラーにする。保存では既存の材質名の対応を使う。スクリプトはネットワーク・ファイル・時計・スレッド・浮動小数点・任意のネイティブコードを実行しない。version 1 のスクリプトは `script.ron` に定義する `Rule { every: u16, x: i32, y: i32, from: String, into: String }` の列のみ (`every` は 1..=1024)。tick 開始時、MOD の ID 順・規則の記載順に、`tick % every == 0` かつその座標の材質名が `from` なら、仕様 03 §1 の `convert` を呼ぶ。1 tick に最大 1024 規則を実行する。ユーザーによる `script.ron` の指定がない MOD は規則なし。これより広い言語処理は version 1 の範囲外とし、ネイティブコードを走らせない。

**追加 (仕様 10 §7、T11.7):** MOD のディレクトリは、上のファイルに加えて**省略できる** `spells.ron`・`wands.ron`・`enemies.ron` を読む。`ModPack::load` はエンジンの既定の定義 (`assets/spells.ron` ほか) を作り、MOD を **MOD ID の昇順**で、名前を `id:name` にして重ねる。`replace: true` の項目は既定または前の層の同名の定義を置き換える。結果は `ModPack::definitions`。形式・検査・エラーは仕様 10 §7。

| テスト | 確認 |
|---|---|
| `mods_load_in_sorted_order` | 配置・列挙の順番に依存せず同じ ID と材質名になる |
| `duplicate_mod_material_is_rejected` | 名前衝突と未定義の相互参照をエラーにする |
| `script_rules_are_deterministic` | 同じ seed と tick の反復で毎 tick のハッシュが一致する |
| `script_rule_limit_is_enforced` | 1025 件目は実行されず、明示的な上限エラーになる |

## 7. 決定的な入力同期

`TickInput { tick: u64, player: EntityId, input: PlayerInput }` を送る。`Lockstep` は参加者の ID を昇順で保持し、tick ごとに全参加者の入力が揃うまでシミュレーションを進めない。同じ tick・ID の二重入力はエラー。揃ったら ID 昇順で入力を適用し、§6 のスクリプト → §2 のエンティティ → 仕様 05 §2 の物理/セル tick を 1 回進める。各 tick の末尾で `World::state_hash()` と `EntityWorld::state_hash()`、剛体の保存可能な状態の昇順バイト列、背景の昇順バイト列を FNV-1a 64-bit で連結ハッシュ化する。参加者の材質名一覧、MOD ID と version、seed、実行バイナリと feature 構成が異なる接続は開始前に拒否する。データはバージョン番号 1 と固定長リトルエンディアンで符号化し、サイズ・tick・ID の範囲を検証してから投入する。初版は同一プラットフォーム・同一ビルドのみを対象とする。Rapier の浮動小数点を含む異種 CPU / OS の決定性は別途実測とクロスプラットフォームのハッシュ照合が通るまで主張しない。欠損入力時に予測やロールバックを行わない。

| テスト | 確認 |
|---|---|
| `lockstep_waits_for_all_inputs` | 1 人の入力が欠けた tick は進まない |
| `lockstep_order_is_stable` | 到着順が違っても同一ビルドで毎 tick のハッシュが一致する |
| `lockstep_rejects_mismatch` | 二重入力、古い tick、バージョン・材質名・MOD 構成の差を拒否する |

## 8. Web

Web では 1 スレッドで `parallel` feature を無効にして構築する。`Streamer` は仕様 04 §2 の同期生成と同じ固定 30 tick の挿入時刻を使い、`std::thread` を生成しない。描画は仕様 06 §2 の WGSL / 頂点 + フラグメントのみ、`Features::empty()` と WebGL2 下限を維持する。保存はブラウザーの永続ストレージが利用できないときもクラッシュせず、未保存を UI に表示する。ネイティブの `PathBuf` とファイル API は Web 用アダプター境界の外に出さず、`charon-core` にブラウザー依存を入れない。ブラウザーで起動、マウス/タッチで配置、ポーズ・復帰、1 回保存・再読込を確認し、WebGPU と WebGL2 の可用性を分けて記録する。

| テスト | 確認 |
|---|---|
| `web_core_compiles` | wasm32 で `charon-core` を非並列 feature でビルドできる |
| `web_streaming_tick_is_deterministic` | 同期生成でもデスクトップとチャンク挿入 tick が同じ |
| `web_save_roundtrip` | ブラウザー上で保存・再読込後の材質名と前景・背景・剛体が同じ |
| `web_input_and_resume` | 実ブラウザーでポインター操作と復帰後の描画が動く |

Web の実ブラウザー確認は自動コンパイル確認とは別の完了条件であり、実施できない場合は未検証と記録する。Android 実機確認についても従来の免除を継続し、成功とは記録しない。
