# E05：セルと流体 — 実装中

## 判定

**E05全体は未完了。Q04は未通過。** この記録を工程の完成証拠として使わない。

作業branchは`codex/e05-cell-and-fluid`、開始点はE04マージ後の`3ae60e20a34f7162f739988d25d332045bc52967`。E05以降の作業順序は[工程表](../../19_IMPLEMENTATION_ROADMAP.md)に従う。

## 実装した範囲

- layerへ所属する物質channelの登録とContentへの所有権記録。
- cell本体と全付随channelの原子的交換・経路輸送。
- 活動域、密度交換、一phaseの再移動防止を扱う局所輸送の参照処理。
- 自由表面、閉領域anchor、重力項を扱う圧力のJacobi緩和。
- 固定件数で進め、World状態として保存・再開できる押出し経路探索と一括公開。

APIの契約・容量費用・未対応範囲は[30_MATERIAL_TRANSPORT](../../30_MATERIAL_TRANSPORT.md)を参照。

## 検証

対象は`tests/material_transport.rs`。輸送中の一channelの容量不足と無視されたエラーの全体rollback、chunk境界での確保回収、負座標と原点変換、blockの拒否、活動域、物質数保存、保存再開の結果一致、圧力式の参照値、検索frontier保存、上向き水路、二重公開、入力失効、record切断、区域退避・復帰、channel所有権と格子の登録検査、channel書込を含む編集予算とWorld全体のchannel byte上限を対象とする。

### ローカル検査

2026-10-16、Windows/MSVC。依存cacheは`target/e01-validation`、TEMP/TMPはAドライブ上の`target/e05-build-tmp`へ指定。Cドライブの空き容量不足に依存してlinkerが失敗しないよう、process単位で作業領域を分けた。

- `cargo fmt --all -- --check`：成功。
- `cargo check --all-targets --locked`：成功。
- `cargo clippy --all-targets --locked -- -D warnings`：成功。
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`：成功。
- 全243テスト：最後のnode容量変更API追加後の最終ツリーで再実行して成功。内訳はlib/host 103、assets 24、foundation 19、material_transport 19、runtime 28、world 50。

表記した各cargoコマンドには共通して`--target-dir A:/Project/charon-engine/target/e01-validation -j 1`を指定した。fmtには指定しない。GitHubのE05専用workflowでWindows・Ubuntu・macOSの対象試験を実行するが、ローカル検査をその3 OSの成功として扱わない。

## ローカルレビューでの修正

- 輸送先を順に確保すると、旧chunkが空になる場合にも不要な容量超過となる。旧値退避→解放→再配置へ修正。
- 保存recordの形式検査だけでは地形との整合が不足する。探索の再開・公開時に物質、空き先、物性と圧力条件を実地形と照合する。
- 予約のsorted Vec挿入はcell数に対し二乗の費用になる。活動chunkごとの4096bitへ変更し、chunk検索とbit演算で処理する。
- 地形だけの編集予約では、付随channelの書込数を予算から漏らす。所属channel数を含めて先に全経路を予約し、編集容量とchannel byte容量の両方でrollbackするテストを追加。
- 探索容量を使い切った後もfrontierを維持して再試行できるよう、node上限の明示変更APIを追加。

## 残作業

[30の残作業](../../30_MATERIAL_TRANSPORT.md#e05の残作業)を実装・検証する。今回の変更はE05内の「輸送と圧力」のレビュー単位であり、E05全体の完了を示さない。この単位をcommit/push・PR・外部レビュー待機・指摘対応まで進め、続いてE05内の残りの実装を行う。E05の出口条件を小さく定義し直さず、未達を残してE06へ進まない。

## PR #4の外部レビュー対応

2026-10-16、Linuxクラウド環境・Rust 1.98.1。レビュー対象は`da0159149ec17504ecfacc27428b957bcc5568ae`。そのcommitのRust CIとE03/E04/E05のWindows・Ubuntu・macOS検査はすべて成功していることをGitHubで確認した。

- [空cellの出口圧](https://github.com/sofia-gros/charon-engine/pull/4#discussion_r4289087672)：探索とReady経路の復元後再検査・公開で、空cellの相対圧を0とする。位置固定channelに`i64::MAX`が残る出口の保存・復元・公開を回帰対象に追加。
- [channel確保前の容量検査](https://github.com/sofia-gros/charon-engine/pull/4#discussion_r4289087685)：全channelの輸送後の非既定値数から必要配列数を事前計算する。全体上限と個別上限を検査し、容量が減るchannelから輸送する。経路外の値が残る移動で、容量拒否が非既定値の再配置前に起きることを値の検証callbackで観測する。別channelの解放容量を再利用する取引も追加。

E05全体とQ04は引き続き未完了。今回の対応は輸送・圧力のレビュー修正単位に限る。

### レビュー修正のローカル検証

`CARGO_HOME=/workspace/.cargo`、`RUSTUP_HOME=/workspace/.rustup`、`CARGO_BUILD_JOBS=2`を設定し、リポジトリ直下からRust 1.98.1で実行した。

- 空出口圧と確保前容量拒否の追加テストは、修正前の実装でそれぞれ`NoOutlet`誤判定と非既定値の再配置への到達により失敗することを確認した。
- 修正後の全246テストが成功。内訳はlib/host 103、assets 24、foundation 19、material_transport 22、runtime 28、world 50。無視・失敗なし。doc-testは0件。
- `cargo +1.98.1 fmt --all -- --check`、`cargo +1.98.1 check --all-targets --locked`、`cargo +1.98.1 clippy --all-targets --locked -- -D warnings`、`RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --locked --no-deps`が成功。
- ローカル差分レビューで、事前計算時に大きい値型の経路配列を複製しないよう修正した。計算用scratchはchunk座標と件数差分だけを保持する。公開API・保存形式の変更なし。

### 再レビュー：探索frontierの費用

`14ea27f`の再レビューで[各stepのfrontier全複製・訪問集合の再構築](https://github.com/sofia-gros/charon-engine/pull/4#discussion_r4289350740)を指摘された。同commitのCIは20件すべて成功したが、CI成功のみでレビュー対応完了とはしなかった。

- 探索に訪問bitsetを保持し、各stepで既存node列を複製しない。今回の追記node・訪問bit・cursor・goal・照合cursorだけを失敗時に巻き戻す。
- 保存recordから訪問索引を一度再構築する。復元後の各nodeの実地形照合をstep予算で進め、完了前はSearchingを返す。全体の構造検査はdecode時、全nodeの実地形照合は公開前にも行う。
- 読込済みchunkの活動集合・入力revision照合、Vecの容量拡張、保存・復元・明示的な候補複製・公開取引の費用は残る。step予算がそれらまで一定時間に制限すると説明しない。`CHPUSH01`と公開methodの型は維持する。
- 追加4テストで、予算1を200回進めても既存frontier配列が同じであること、復元時の照合予算、NoOutlet復元時の段階、途中の容量失敗後の訪問bit・cursorの回復を検査した。Ready復元の既存回帰テストも照合完了までstepする形へ更新した。
- Linux / Rust 1.98.1で全250テスト成功（lib/host 107、assets 24、foundation 19、material_transport 22、runtime 28、world 50）。失敗・無視なし。fmt、全target check、警告拒否Clippy・Rustdocも成功した。

E05全体・Q04の未完了と残作業は変わらない。追加commit・push後に再レビューを確認する。

### 再々レビュー：圧力出力の容量再利用

`5039cb6`の再レビューで[圧力chunkを解放する前の確保](https://github.com/sofia-gros/charon-engine/pull/4#discussion_r4289404815)を指摘された。同commitのCIは20件すべて成功した。

- channelの非既定値数から最終byte数を求める処理を輸送と圧力出力で共有する。全出力の座標重複・値・coverage・個別chunk上限・World全体のbyte上限を配列確保前に検査する。
- 既定値への出力を先に適用し、空chunkを解放してから他の出力を適用する。編集の書込数、coverageのrevision更新、失敗時のWorld取引全体の破棄は既存のset_channelを使う。
- 追加2テストで、正圧を新たに得る水柱と0になる自由表面の座標順を反転し、channel上限・World上限・両上限で同じ結果となることを検査した。既定値が5のchannelで、閉領域が既定値に戻って解放され、別chunkの自由表面0を確保できることも検査した。両テストとも修正前のpressure実装で失敗することを確認した。
- Linux / Rust 1.98.1で全252テスト成功（lib/host 107、assets 24、foundation 19、material_transport 24、runtime 28、world 50）。失敗・無視なし。fmt、全target check、警告拒否Clippy・Rustdocも成功した。公開APIと保存形式の変更なし。

E05全体・Q04は未完了。輸送・圧力の外部レビュー対応後に残るE05の実装へ進む。

## 輸送・圧力のレビュー完了と次の単位

[PR #4](https://github.com/sofia-gros/charon-engine/pull/4)の最終commitは`f4c2b550f939d26a9f1c8eb4647037c5e4e8d56d`。CI20件すべて成功し、2026-10-16の再レビューは新規・未解決指摘なしで完了した。PRはマージしていない。

次の単位を同commitから`codex/e05-motion-and-sweep`へ分け、PR #4を基点とする差分として実装する。

### 速度・端数と整数格子の経路検査

- `MaterialMotion`を物質channelへ登録し、符号付きQ16速度と未消化の端数を輸送・保存する。
- `transport_material_motion`が整数変位の途中をDDAで検査し、通過点と角の直交検査点を予約する。壁・World境界、未準備域、予約競合を区別する。
- 逆順の一括経路輸送で全物質channelを追従させる。力・時計の暗黙更新、一般形状の衝突、密度交換は実行しない。
- [31の契約](../../31_MATERIAL_MOTION.md)、root README、実行可能な`examples/material_motion.rs`、3 OSのE05 CIを更新した。

ローカルレビューでは、静止cellを通過予約にせず物理障害と区別すること、非活動域の占有情報から物理応答を決めないこと、連続する交換を逆順の一括経路輸送へまとめて最終容量を検査することを修正した。

### 検証

Linuxクラウド環境、Rust 1.98.1、`CARGO_BUILD_JOBS=2`。リポジトリ直下で以下を実行した。

- `cargo +1.98.1 test --locked`：全264テスト成功。lib/host 107、assets 24、foundation 19、material_motion 12、material_transport 24、runtime 28、world 50。失敗・無視なし。doc-testは0件。
- `cargo +1.98.1 fmt --all -- --check`、`cargo +1.98.1 check --all-targets --locked`、`cargo +1.98.1 clippy --all-targets --locked -- -D warnings`、`RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --locked --no-deps`：成功。
- `cargo +1.98.1 run --locked --example material_motion`：20tick・7cell・衝突1回。壁の直前での停止と速度・端数を含む保存復元をassertで確認した。

12件の対象試験は正負の端数と保存再開、壁・角、全八分象限、経路交差と予約、全物質payloadと位置固定場、未読込・非活動域、有限World境界、誤ったchannel、極端な速度、容量不足の全体rollback、codec切断・不正端数を検査する。

### 残作業

本単位は明示速度の整数格子経路であり、一般shape sweep、重力・抗力・浮力・摩擦の物性接続、広域移動の保存可能Job、providerの統合・認定を実装済みとしない。混合・分離、正規物質粒子とpool、source/sink/変換receiptと収支、気体sourceと広域圧力、dirty/sleepの参照比較も残る。E05全体・Q04は未通過で、次の依存工程へ進まない。
