# テストの書き方

Charon Engine は、仕様の「必須テスト」が通ることを完了条件にしている。
この文書は、テストをどこに・どう書くかを決める。

## 1. 置き場所

| 種類 | 場所 | 内容 |
|---|---|---|
| 単体テスト | 各ソースファイルの末尾の `#[cfg(test)] mod tests` | 1 つの関数や型の検査 (座標変換、乱数、読み込みなど) |
| 場面テスト | `crates/<クレート>/tests/*.rs` | 世界を作って何 tick か進め、結果を確かめる。仕様の表にあるテストはほぼこれ |
| ベンチマーク | `crates/<クレート>/benches/*.rs` | criterion。結果は `docs/BENCHMARKS.md` に記録 |

場面テストのファイル分け (`charon-core`):

| ファイル | 仕様 | 追加するタスク |
|---|---|---|
| `tests/powder.rs` | 02 §7 | T1.9 |
| `tests/fluid.rs` | 02 §8 | T1.10 |
| `tests/sleep.rs` | 02 §9 | T1.11 |
| `tests/invariants.rs` | 02 §10 (`mass_is_conserved`) | T1.12 |
| `tests/determinism.rs` | 02 §10 (`deterministic`、`golden_hashes`) | T1.12 |
| `tests/reactions.rs` | 03 §1、§4、§5 | T2.2〜T2.6 |
| `tests/heat.rs` | 03 §2、§3 | T2.3、T2.4 |
| `tests/parallel.rs` | 02 §2 (`parallel_matches_serial`) | T3.1、T3.2 |
| `tests/benchmark_fixture.rs` | 08 §2 (`benchmark_fixture_updates_cells`) | T9.1 |

テスト関数の名前は、仕様の表にある名前と同じにする (検索できるようにするため)。

## 2. `testkit` モジュール

場面テストは必ず `charon_core::testkit` を使って書く。世界を手で組み立てるコードをテストごとに書かない。
`testkit` は `charon-core` の公開モジュールで、常にコンパイルする (他のクレートのテストからも使うため)。

```rust
// crates/charon-core/src/testkit.rs

/// 文字の絵から世界を作る。絵の左上が CellPos { x: 0, y: 0 }。
/// 絵を覆うのに必要なチャンクを作り、絵の外側のセルはすべて石で埋める。
/// 材質は `MaterialTable::builtin()` を使う。知らない文字は panic。
pub fn world_from_ascii(art: &str, seed: u64) -> World;

/// 世界の矩形 (x0..x1, y0..y1。終端は含まない) を文字の絵に戻す。行は改行で区切る。
pub fn to_ascii(world: &World, x0: i32, y0: i32, x1: i32, y1: i32) -> String;

/// 上が開いた箱の絵を作る。内側は幅 width、高さ height の空間で、左右と下が石の壁。
/// 内側の左上は (1, 0)、床の上の行は y = height - 1。
pub fn box_art(width: usize, height: usize) -> String;

/// 仕様 08 §2 の 100 万セル場面。生成後、計測する tick 1 に進める。
pub fn falling_million() -> World;

/// 文字から材質へ。`world_from_ascii` と `place` が使う。
pub fn material_of(world: &World, ch: char) -> MaterialId;

/// (x, y) に材質を 1 個置く (`World::make_cell` と `World::set`)。
pub fn place(world: &mut World, x: i32, y: i32, ch: char);

/// 矩形 (両端を含む) を材質で埋める。
pub fn fill(world: &mut World, x0: i32, y0: i32, x1: i32, y1: i32, ch: char);

/// (x, 0) から材質を n 個、1 tick に 1 個ずつ落とす (置くたびに 1 tick 進める)。
/// 注ぎ口が塞がって置けない状態が続いたら panic。
pub fn pour(world: &mut World, x: i32, ch: char, n: usize);

/// 世界全体で、その材質のセルを数える。
pub fn count(world: &World, ch: char) -> usize;

/// 列 x0..x1 のそれぞれについて、その材質のセル数を返す。水面の高さの比較に使う。
pub fn column_heights(world: &World, ch: char, x0: i32, x1: i32) -> Vec<u32>;

/// n tick 進める。
pub fn run(world: &mut World, ticks: u64);

/// 世界全体が休眠する (`awake_chunks() == 0`) まで進める。かかった tick 数を返す。
/// `max_ticks` を超えたら None。
pub fn run_until_sleep(world: &mut World, max_ticks: u64) -> Option<u64>;

/// `run_until_sleep` が None なら、世界の絵と起きているチャンクの位置を表示して panic。
pub fn assert_sleeps(world: &mut World, max_ticks: u64);

/// n tick 進め、毎 tick の `state_hash()` を集めて返す。
pub fn hash_trace(world: &mut World, ticks: u64) -> Vec<u64>;
```

文字と材質の対応 (固定。増やすときはこの表にも足す):

| 文字 | 材質 | 文字 | 材質 |
|---|---|---|---|
| `.` | empty | `#` | stone |
| `s` | sand | `d` | dirt |
| `w` | water | `m` | slime |
| `o` | oil | `l` | lava |
| `g` | methane | `t` | steam |
| `W` | wood | `i` | ice |
| `a` | acid | `f` | fire |
| `k` | smoke | `h` | ash |

### 例

```rust
use charon_core::testkit::*;

#[test]
fn sand_sinks_in_water() {
    let mut world = world_from_ascii(
        "
        #......#
        #..ss..#
        #wwwwww#
        #wwwwww#
        ########
        ",
        1,
    );
    assert_sleeps(&mut world, 2000);
    // 砂が最下段にあり、数が変わっていない
    assert_eq!(count(&world, 's'), 2);
    assert_eq!(to_ascii(&world, 3, 3, 5, 4), "ss");
}
```

絵は、各行の先頭と末尾の空白を取り除き、空行を捨ててから読む (上の例のようにインデントできる)。
全行の長さが同じでなければ panic。

## 3. テストを書くときの規則

1. **乱数に頼る結果は、複数の seed で確かめる。** 場面テストは `for seed in 1..=8` で回す。
   1 つの seed でしか通らないテストは、仕様かテストのどちらかが間違っている。
2. **「何 tick 後にこうなっている」より「眠った後にこうなっている」を使う。**
   `assert_sleeps` で世界を落ち着かせてから結果を調べる。tick 数に依存するテストは壊れやすい。
3. **数を必ず確かめる。** 移動のテストでは、最後に `count` で材質の数が変わっていないことを調べる。
4. **休眠を必ず確かめる。** 新しい規則のテストには、最終的に `assert_sleeps` が通る場面を 1 つ入れる
   (仕様 02 §9)。
5. **失敗したら絵を出す。** `assert!` のメッセージに `to_ascii` の結果を入れる。
6. テストを `#[ignore]` にしない。消さない。通らないときは `CLAUDE.md` の
   「仕様と実装が合わないとき」に従う。

## 4. 決定性と基準ハッシュ

`tests/determinism.rs` には 2 種類のテストを置く。

**`deterministic`**: 同じ絵と seed で世界を 2 つ作り、`hash_trace` が完全に一致することを確かめる。

**`golden_hashes`**: 決まった場面を決まった tick 数だけ進めた `state_hash()` を、
`tests/golden/<場面名>.txt` に保存した値と比べる。CI は Linux、Windows、macOS で実行するので、
OS やコンパイラの版によって結果が変わらないことを確かめられる。

- 場面は `tests/determinism.rs` の中に絵で書く。最低 3 つ: 砂と水、全種類の液体、(M2 以降) 反応と熱。
- 環境変数 `CHARON_BLESS=1` を付けて実行すると、ファイルを現在の値で書き直す。
- 基準を書き直してよいのは、仕様を変えて挙動が変わるコミットだけ。
  コミットメッセージに「基準ハッシュ更新」と理由を書く。
  **理由なくハッシュが変わったら、それは不具合である** (並列化や最適化のコミットでは絶対に変わってはいけない)。

```bash
CHARON_BLESS=1 cargo test -p charon-core --test determinism golden_hashes
```

## 5. 性質テスト

`mass_is_conserved` や `everything_sleeps` のような「どんな世界でも成り立つ」テストは、
`rng::hash` を使って世界を乱数で作る (外部の乱数クレートを使わない)。

```
for seed in 1..=32:
    64×64 の箱を作り、各セルを hash(seed, 0, x, y, 0) で 材質に割り当てる
    (空 50%、残りを対象の材質で等分)
    数を記録 → 進める → 数を比べる
```

失敗したときに再現できるよう、panic のメッセージに seed を入れる。

## 6. ベンチマーク

```bash
cargo bench -p charon-core
```

結果は `docs/BENCHMARKS.md` に、日付、コミット、CPU、各ベンチマークの中央値を表で追記する
(上書きせず、履歴を残す)。性能に関わるタスク (M3) では、変更の前後の値を必ず記録する。

## 7. 試作との照合

`tools/prototype/fluid.py` は、仕様 02 §7、§8 の移動規則を Python で書いた試作である。
仕様の規則が必ず眠ること、水が平らになることは、この試作で確かめてある。

```bash
python3 tools/prototype/run.py
```

Rust の実装で場面テストが通らないときは、同じ場面を試作で動かして比べる。
試作で通って Rust で通らないなら実装の誤り、試作でも通らないなら仕様の問題である。
乱数の `hash` は同じ式なので、走査の順序を合わせれば同じ結果になる (ただし試作にはチャンクがないため、
厳密な一致は求めない)。
