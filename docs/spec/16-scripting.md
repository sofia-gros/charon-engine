# 仕様 16: MOD のスクリプト (Rhai)

`charon-app::script::ScriptGame`。Rhai (純 Rust の組み込みスクリプト) の小さな部分を、`Game` (仕様 14) として動かす。MOD のディレクトリの `script.rhai` がそれ。

## 1. 関数 (スクリプトが定義する)

| 関数 | 呼ばれるとき |
|---|---|
| `on_start()` | プレイヤーが生まれた最初の tick に 1 回 |
| `on_event(kind, a, b)` | 出来事 (仕様 10 §15) ごと。`kind` は `"hit"` / `"defeated"` / `"dug"`、`a, b` は `hit` と `dug` ではセルの x, y、`defeated` では 0 |
| `on_tick()` | 毎 tick |

どれも省略できる。

## 2. 使える関数 (エンジンが登録)

命令 (あとでまとめて行う): `shake(強さ)`、`play("jump"|"shoot"|"anchor"|"pickup")`、`give("材質", 個数)` (プレイヤーの持ち物へ)、`set_cell(x, y, "材質")`。
読み取り (tick の初めの写し): `tick()`、`has_player()`、`player_x()`、`player_y()`、`hp()`。
`print(..)` はログに溜まる (`take_log`)。

## 3. 安全のための決まり

- スクリプトは世界を直接触らない。命令は `on_*` が終わったあとに、エンジンが順に行う。知らない材質・音は無視し、境界のセルは書き換えない。
- 1 回の呼び出しで `MAX_OPERATIONS` (5 万) 命令まで。1 tick に溜められる命令は `MAX_COMMANDS` (256) まで (超えた分は捨てる)。文字列 256 文字、配列 256 要素、呼び出しの深さ 16 まで。
- ファイル・時間・ネットワークなどの関数は登録しない (`no_time`、`no_module`)。
- 実行時エラー (命令の超過を含む) は報告して続け、`MAX_ERRORS` (10) 回で、そのスクリプトを止める。文法のエラーは読み込みで `Err`。
- 同期 (Lockstep) のゲームでは、スクリプトの結果が全員で同じになるよう、整数だけで書くこと。`shake` / `play` は描画側なので自由。

## 4. デモ

`assets/mods/fse_demo/script.rhai`: 敵を倒すと音と揺れ、掘ると小さく揺れる。デモは Rust の `DemoGame` のあとに、同じ出来事でこのスクリプトを呼ぶ。

| テスト | 確認 |
|---|---|
| `scripts_read_the_snapshot_and_queue_commands` | 写しを読み、`give`・`play`・`shake`・`set_cell` が効く |
| `events_reach_on_event_with_kind_and_position` | 出来事の種類と位置が渡る |
| `bad_scripts_are_contained` | 文法エラー、無限ループ、エラーで停止、知らない材質、命令の上限 |
| `the_demo_mod_script_works` | 同梱のスクリプトが読めて動く |
| `scripts_cannot_reach_time_or_files` | 時間などの関数がない、`print` がログに溜まる |
