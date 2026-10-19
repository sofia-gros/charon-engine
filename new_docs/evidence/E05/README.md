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

## PR #5の手動レビュー確認

[PR #5](https://github.com/sofia-gros/charon-engine/pull/5)の対象commitは`25d522f13888073473d47797431184b51704ad1d`。CI20件はすべて成功した。

コードレビューの実施状態について確認が必要となり、後続のreceipt作業を退避して[手動再依頼](https://github.com/sofia-gros/charon-engine/pull/5#issuecomment-6207919520)した。2026-10-16 13:17 JSTの[新しいレビュー結果コメント](https://github.com/sofia-gros/charon-engine/pull/5#issuecomment-6207963869)で対象commitと「重大な問題は見つからなかった」との結果を確認した。要約のcompleted表示だけを根拠にせず、この実行の結果コメントを記録する。PRはマージしていない。

## cell変更と理由付きreceipt

`codex/e05-material-receipts`を`25d522f`から分け、次のレビュー単位として実装する。

- `apply_material_changes`が全cellの期待値・出力・payload方針を検査し、Source・Sink・Conversionの理由と変更前後をWorld StateStoreへ地形とともに記録する。
- 消費・発行を安定した素材キーへ集計し、cell token数はu64、換算massはu128で保存する。作者が意図したmass差を禁止しない。
- World・Content・journalの所属、保存順、過去の変更列と量の一致を照合する。blockの暗黙消費、未読込、重複、期待値失効、容量不足を拒否する。
- payloadのKeepChannels・ResetChannelsを明示する。位置固定場と別layerを巻き込まず、参照先storeの個体を自動破棄しない。
- [32の契約](../../32_MATERIAL_RECEIPTS.md)、root README、`examples/material_receipts.rs`、3 OSのE05 CIを更新した。

### ローカルレビューと検証

ローカルレビューで、receiptの戻り値も外側のWorld取引が成功するまでは候補であることを明示した。外部recordの形式検査とWorld参照・集計の照合を区別し、監査用の`validate_world`を公開した。journalの容量を超えたときに古いreceiptを自動削除せず、変換を停止する。

Linux / Rust 1.98.1、`CARGO_BUILD_JOBS=2`で次を実行した。

- 全279テスト成功：lib/host 107、assets 24、foundation 19、material_motion 12、material_receipts 15、material_transport 24、runtime 28、world 50。失敗・無視なし。doc-testは0件。
- fmt、全target check、警告拒否Clippy・Rustdocが成功。コマンドは`cargo +1.98.1 fmt --all -- --check`、`cargo +1.98.1 check --all-targets --locked`、`cargo +1.98.1 clippy --all-targets --locked -- -D warnings`、`RUSTDOCFLAGS="-D warnings" cargo +1.98.1 doc --locked --no-deps`。
- `cargo +1.98.1 run --locked --example material_receipts`が成功。発行mass10、mass10→10の変換、消費mass10の3receiptと、空になった地形を保存・復元してassertで確認した。

15件の対象試験で、Source・Conversion・Sink、payload移行、意図したmass差、複数cell、失効と無視されたエラー、件数・codec・通常編集容量、World・Content・journal、block、codec切断、集計の不一致、区域退避・復帰を検査した。

### 未完了の境界

これはcell表現のreceiptであり、raw編集・生成器・全providerの監視、粒子・剛体local cell・退避領域・reservoirを合わせた総量の参照比較ではない。外部に永続化したreceiptの整理、非cell表現のbridge、熱energyの収支も残る。保存recordは既存StateStoreへ含め、World・区域archiveの構造は変更しない。新規schemaでContent manifestが変わることと、既存保存の自動互換を混同しない。

E05全体・Q04は引き続き未通過。混合・分離、正規粒子とpool、全体収支、気体source・広域圧力、dirty/sleep、物性・provider統合を残してE06へ進まない。

## PR #6のレビュー確認と継続指示

[PR #6](https://github.com/sofia-gros/charon-engine/pull/6)の対象commitは`01ba6facf1bbe98cc7ce9cf5054e1c6db73ce4ce`。CI20件すべて成功し、[対象commitへのレビュー結果](https://github.com/sofia-gros/charon-engine/pull/6#issuecomment-6208143360)が2026-10-16 13:33 JSTに追加された。「重大な問題は見つからなかった」との結果を確認し、次の単位へ進んだ。

その後のユーザー指示により、E05全体の出口条件を満たし、CI・レビューに問題がなければマージし、その後E06へ進む。E05未達を残して先行マージ・E06着手を行わない。

## 気体sourceと広域圧力job

`codex/e05-wide-pressure`を`01ba6fa`から分けた。[33の契約](../../33_WIDE_PRESSURE.md)と`examples/pressure_job.rs`を追加した。

- `PressureJob`が隣接構築、連結探索、連結量の反映、Jacobi反復とbuffer切替を固定件数で進める。途中の圧力を公開しない。
- 気体profileは気体・空cellの連結容積と気体mass・絶対温度から圧力sourceを計算する。液体と気体の連結を分離し、活動外・未読込・block・World境界をno-flowとする。
- 作者のポンプchannelを流体cellの最低圧境界にできる。圧力の指定だけで物質やenergyを発行・消去しない。
- 入力scope・Content・epoch・活動集合とrevisionを照合し、完成後に全出力を原子的に公開する。channelの解放容量を先に再利用する。
- 保存recordを復元した場合は、入力の再採取と決定的再計算でscratchを照合する。計算再実行もstep予算を使い、未照合のReadyを公開しない。

### ローカルレビューと検証

ローカルレビューで、有限Worldの部分chunkの外を容積から除外すること、探索queueの全件走査をstageの一件処理に含めないこと、公開時に全反復をやり直さないこと、復元状態を照合する前の公開を禁止することを確認・修正した。通常stepでfrontierを複製せず、new・公開・保存・復元の一括費用と復元照合用の追加scratch・再計算費用は33へ明記した。

Linuxクラウド環境・Rust 1.98.1、`CARGO_BUILD_JOBS=2`。

- `cargo fmt --all -- --check`、`cargo check --all-targets --locked`、`cargo clippy --all-targets --locked -- -D warnings`、`RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`：成功。
- `cargo test --locked`：全293テスト成功。lib/host 107、assets 24、foundation 19、material_motion 12、material_receipts 15、material_transport 24、pressure_job 14、runtime 28、world 50。失敗・無視なし。doc-testは0件。
- `cargo run --locked --example pressure_job`：mass2・温度3・容積3でQ16圧力131072、計算48件。途中保存からの再開と連続実行のcheckpoint一致をassertで確認した。

追加14テストで、従来solverとの保存byte一致、予算1・3・65536、全段階のcodec復元、Worldとjobの同時保存、温度と容積、連結の分離、chunk境界、非活動域、有限境界、ポンプ、入力失効、負値・数値上限、scratch容量・channel容量、無視されたエラーを含むrollback、切断・診断量改変を検査した。3 OSのE05 workflowへ対象テストと作例を追加した。

### 残作業

広域圧力jobとsourceのAPI追加は、混合・分離、正規物質粒子とpool、全表現の収支、dirty/sleep、物性とproviderの統合を完了するものではない。初期採取・最終公開まで一定時間で分割するschedulerも別途費用設計が必要である。E05全体・Q04は引き続き未通過。commit/push・PRの実レビューを確認し、指摘へ対応してから次の実装単位へ進む。

### 追加照合：気体targetの緩和

初版commit`181da8e`をレビューへ出した後、22の気体式との追加照合で、targetの直接代入を重みlambda付きのJacobi緩和へ修正した。`GasPressureSource.relaxation_weight`を保存し、気体cellでは近傍の圧力・重力項と`lambda × target`を同時に評価する。閉じた液体だけに平均anchorを適用し、気体の最小座標をanchorへ固定しない。空間のtarget境界は派生値で、物質cellの前回圧を直接置き換えない。

lambda0・1・3、隣接気体2cell、1反復の参照式を検査するテストを追加した。全294テスト成功（pressure_job 15、他の内訳は上と同じ）、fmt・全target check・警告拒否Clippy・Rustdoc成功。保存形式を`CHPRJOB2`へ更新し、初版recordを誤って現形式と解釈せず拒否する。旧recordの自動移行はない。作例と修正後の対象テストも再実行した。初版へのレビュー結果を、この修正を含むcommitのレビュー結果として扱わない。

### PR #7のレビュー対応：空cell境界とlambda

`99cb429`のCI20件はすべて成功したが、[空cell境界からtargetを注入してlambdaを迂回するP1](https://github.com/sofia-gros/charon-engine/pull/7#discussion_r4291060260)を指摘された。22の標準式を維持し、空cellの旧圧・固定境界を相対0に保つよう修正した。targetの加算経路を気体cellのlambda項に限り、空間量の集計と圧力の境界条件を混同しない。

空cell2つと気体cell1つを連結し、lambda0・1・3で1反復の参照値を検査する回帰テストを追加した。同テストを修正前`99cb429`で実行し、lambda0の結果が期待0に対して131072となって失敗することを確認した。修正後は0・65536・98304で成功する。温度と容積の他の試験・作例も、target圧と公開される緩和後圧を区別して更新した。

修正後は全295テスト成功（pressure_job 16、他の内訳は上と同じ）。fmt、全target check、警告拒否Clippy・Rustdoc成功。作例はtarget Q16=131072、lambda1・空境界0からの緩和後Q16=65536、計算48件と保存再開一致を確認した。保存recordの構造は`CHPRJOB2`のままで、古い誤ったscratchは復元後の決定的照合で拒否する。修正をpushして再レビューを確認するまで、本単位もレビュー完了とは判定しない。

### PR #7の再レビュー：素材別のlambdaと保存進捗

`1152352`のCI20件はすべて成功したが、[lambdaをjob共通値にしていたP1](https://github.com/sofia-gros/charon-engine/pull/7#discussion_r4291099530)を指摘された。`MaterialBuilder::gas_pressure`と`GasPressureProperties`で係数・lambda・上限を気体素材ごとに登録し、`MaterialDef`から各cellの入力へ解決する形へ修正した。同じ連結領域の気体2種類にlambda1・3を与え、1反復でそれぞれ3・4.5となる参照値を検査した。係数によるtargetの変化、上限超過、非気体・負上限の登録拒否、Content fingerprintの変化も検査した。明示物性はContentの物理manifestへ含め、異なる物性を互換と扱わない。

併せて、毎phaseでStateStoreからjobを取得して一件進め再保存する試験を追加した。修正前の復元時全再計算方式では256phase後も完了せず失敗した。過剰な全再計算・照合用の二重scratchを除去し、保存した数値配列・queue・cursorをそのまま継続する。decodeでは数値・段階・隣接索引・queueと不可能な診断件数を検査し、公開前にはWorldの全入力を再採取して照合する。保存bufferのchecksumは既存保存層が担い、反復履歴を再実行した証明とはしない。これは上記初版の再計算方式を置き換える。`CHPRJOB3`として以前の未マージrecordを拒否し、自動移行は行わない。

対象19テストと全298テストが成功。内訳はlib/host 107、assets 24、foundation 19、material_motion 12、material_receipts 15、material_transport 24、pressure_job 19、runtime 28、world 50。失敗・無視なし。fmt・全target check・警告拒否Clippy・Rustdoc、保存再開の作例も成功した。

リソース使用の確認では、同じcommitに4 workflowがpushとpull_requestで二重起動し、計20ジョブとなっていた。開発ブランチはPR、pushはmain、手動実行は維持する設定へ統一した。検査項目と3 OSを削らず、以後の開発PRは計10ジョブにする。変更ごとの検査をまとめてからpushし、最新commitの実レビュー結果とCIで判定する。

### 最新の停止条件

ユーザーの最新指示により、**E05全体の完了・CIとレビューの問題解消後にマージして停止する。E06には着手しない。** PCへ戻すため、最後にmain/branchのcommit、PR、検証結果と引き継ぎ手順を記録する。これは以前の「E06へ進む」指示を置き換える。現時点はE05実装中である。

### PR #7のP2対応：連結成分と訪問集合

`777fc9b`は重複抑制後のCI10件すべて成功したが、[queue外の訪問済みnodeを受理するP2](https://github.com/sofia-gros/charon-engine/pull/7#discussion_r4291179686)を指摘された。

decodeで連結成分を一度構築し、stage・cursor・rootに対応する訪問集合、BFS順のqueueと発見prefix、反映済みsourceとanchor、部分集計・target・診断件数を照合する。過去のJacobi反復は再実行しない。未処理の別成分のseenだけをtrueにしたrecordと、訪問bitを保ってqueue順だけを入れ替えたrecordを拒否する試験を追加した。前者は修正前に不整合を受理して失敗することを確認した。正しい保存は液体・気体・空間の複数連結成分について、全計算段階で復元可能なことを検査する。

修正後は全300テスト成功（pressure_job 21、他の内訳は前節と同じ）。fmt・全target check・警告拒否Clippy・Rustdoc・保存再開作例が成功した。保存形式はCHPRJOB3を維持し、不整合recordへの検査を強化する。復元の一時scratchと連結探索の費用は33へ追記した。最新commitへの再レビューを待ち、指摘対応を終えてから次の実装単位へ進む。

### PR #7の追加P2：Jacobiとbuffer切替の数値prefix

`95698a7`の再レビューで[Jacobi・buffer切替のcursor改変](https://github.com/sofia-gros/charon-engine/pull/7#discussion_r4293119194)を指摘された。現反復の計算を`next_pressure`へ共有し、stage4の処理済みprefixのnext、未処理suffixの保持値と初回oldを照合する。stage6はコピー済みprefixのold==next、初回の未コピーsuffix、コピー開始前のnextを検査し、完成時もold==nextを要求する。過去の全反復を再実行しない。

cursorを0から全node数へ書き換える回帰試験を追加した。修正前はstage4の不整合を受理して失敗し、修正後はstage4・6の両方を拒否する。複数成分・全段階の正しい保存と毎phaseの復元も成功した。対象22テスト、全301テスト、fmt・全target check・警告拒否Clippy・Rustdocと保存作例が成功。保存形式CHPRJOB3は維持する。最新commitのCIと実レビュー結果を確認してから次の実装へ進む。

## PR #7のレビュー・CI完了

[PR #7](https://github.com/sofia-gros/charon-engine/pull/7)の対象commitは`25e486db8e13396a40ec1cba5527d3f04b7b46e4`。重複抑制後のCI10件すべて成功し、[対象commitを明記した実レビュー結果](https://github.com/sofia-gros/charon-engine/pull/7#issuecomment-6212775166)で重大な指摘なしを確認した。指摘スレッドは対応結果を回答して解決した。E05全体が未完了のためマージはしていない。

## 混合・分離と共有組成

`codex/e05-mixtures`を`25e486d`から分けた。契約は[34](../../34_MATERIAL_MIXTURES.md)、作例は`examples/material_mixtures.rs`。

- MixtureStateの素材キーと整数token比率を正規化し、Worldの共有storeへ登録する。cellにはWorld付きの固定長MixtureRefだけを置く。定義に物質所有権を持たせず、appendしたindexを再利用しない。
- carrierの質量を純物質別に正確な有理数量へ展開する。mass5・3の素材をmass4の二cellへ混合する場合、各cellに純物質tokenを各1/2ずつ持ち、前後の成分量を保持する。
- PreserveComponentsは素材別token・massの完全一致を要求し、RecordConversionは作者が理由付きで指定した発行・消費・変換差を認める。接触だけで液体を自動混合しない。
- 全cell・payload方針・組成参照と、cell receiptへ結び付く組成witnessを同じWorld取引で保存する。容量不足や期待値失効を無視しても外側の取引全体を破棄する。
- World・Content・schema・channel所有権・参照indexを照合する。共有定義とwitnessは既存StateStore、参照channelは既存の輸送・保存・区域退避を使う。

### ローカルレビューと検証

ローカルレビューで、同じ組成を再登録するno-opでもschema不一致を拒否すること、成分数上限をdecodeの確保前に検査すること、監査でjournalキーを照合して同じjournalを一度だけ読み込むことを確認・修正した。組成数と全履歴・入出力の成分展開に依存する費用、u128中間値で拒否する範囲、外部store個体とenergyの移行を作者が指定する境界を34へ明記した。

Linux / Rust 1.98.1、CARGO_BUILD_JOBS=2。

- 全316テスト成功。lib/host 107、assets 24、foundation 19、material_mixtures 15、material_motion 12、material_receipts 15、material_transport 24、pressure_job 22、runtime 28、world 50。失敗・無視なし。doc-testは0件。
- fmt、全target check、警告拒否Clippy・Rustdocが成功。`cargo fmt --all -- --check`、`cargo check --all-targets --locked`、`cargo clippy --all-targets --locked -- -D warnings`、`RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`を実行した。
- `cargo run --locked --example material_mixtures`が成功。水token1/2・mass5/2、油token1/2・mass3/2、総mass8の発行→混合→分離、witness2件と途中保存からの再開のcheckpoint一致をassertした。

15テストで正規化・再登録、正確な端数・混合分離、詰めたcarrier、意図した変換差、Source/Sink、payload保持・初期化、World・Content・schema・layer所有権、未登録素材、種類数・定義数・receipt件数・codec byte・journal・channel容量、期待値・重複・block・未読込、無視されたエラー、輸送・保存・区域退避、codec切断と有理数演算上限を検査した。3 OSのE05 CIへ対象テストと作例を追加した。

### 残作業

この単位は現在の全表現を監視する総量台帳ではない。正規物質粒子とpool、粒子・剛体local cell・reservoir・退避領域を含む全体収支、dirty/sleepの参照比較、物性とproviderの統合、広域移動とQ04を続ける。E05の出口を満たすまではマージせず、満たした後はマージしてPCへ引き継ぎ、E06には着手しない。

### PR #8のP1対応：古い共有storeからの巻戻し

[指摘](https://github.com/sofia-gros/charon-engine/pull/8#discussion_r4293458391)は、`MixtureStore::new`の既存record検査だけでは、あらかじめ作った空storeや過去のsnapshotを汎用`set_state`で書き戻せる点だった。登録indexを再利用して既存参照の意味を変えるためP1に該当する。古い定義前・receipt前のsnapshotで上書きする回帰試験が修正前に失敗することを確認した。

Worldの固定キーへのtyped・encoded書込みで型とWorld/Contentを検査し、現在の全定義とwitnessが候補の同一prefixとして残ることを取引内で要求する。別型への置換も拒否し、エラーを無視しても外側をrollbackする。古い候補二つを先に準備する場合も拒否し、容量変更と正規の追記は許す。CHMIX001の形式は変えない。全履歴の再decode・prefix比較費用を34へ記録した。

Linux / Rust 1.98.1、CARGO_BUILD_JOBS=2で全318テスト成功（混合物17、他の内訳は前節と同じ）。fmt・全target check・警告拒否Clippy・Rustdocと混合分離の保存再開作例が成功した。

この再開直後にはGitHub APIが401、Gitのreadが認証入力エラーとなったが、ローカル検証後のpushは成功し、APIの再試行でもPRのhead更新を確認できた。一時的な失敗として認証の追加設定は不要だった。最新CI・再レビュー・指摘返信を続け、完了と見なすのは実結果確認後とする。E05全体は未完了、E06は未着手である。

### PR #8の追加P1：journalと分岐履歴の書戻し

`1faeea5`のCI10件は全成功したが、再レビューで[journalの巻戻し](https://github.com/sofia-gros/charon-engine/pull/8#discussion_r4293579504)と[分岐Worldのwitness追記](https://github.com/sofia-gros/charon-engine/pull/8#discussion_r4293579511)を指摘された。prefix保持だけでは、対応receiptが現Worldにない履歴を追記できる。最初の再レビュー実行はサービスのUnknown errorで終了し、再依頼した実行の実指摘に対応した。

汎用typed・encoded書込みではMixtureStoreの定義・witnessとMaterialReceiptLogのreceipt列を現在の全履歴と完全比較し、初回は空履歴だけを受理する。容量変更は許す。定義と履歴の追記は公開commandから呼べない専用内部経路へ移し、登録またはcell編集の同じ候補内だけで実行する。全checkpoint復元はこの差分書戻しとは別の経路である。

journal snapshotの受理を修正前の失敗で確認した。修正後は古いjournal、分岐Worldのwitness・journal・定義の持込み、容量変更と拒否後の正常な混合を検証する。既存receipt試験3件も、不整合を保存後に検出する想定から、保存自体の拒否と世界の不変を検証する形へ更新した。

Linux / Rust 1.98.1、CARGO_BUILD_JOBS=2で全320テスト成功（混合物19、receipt15、他は前節と同じ）。fmt・全target check・警告拒否Clippy・Rustdoc・保存再開作例が成功した。CHMIX001とreceiptの保存形式は変えない。最新commitのCI・再レビューはpush後に確認する。E05全体とQ04は未完了である。

## PR #8のレビュー・CI完了

対象は`6533710d977715970fa970651c3ba11f6e68b7a8`。CI10件すべて成功し、[対象commitを明記した実結果](https://github.com/sofia-gros/charon-engine/pull/8#issuecomment-6213560690)で重大な指摘なしを確認した。3件のP1スレッドに対応内容を回答し、解決した。E05全体が未完了のためマージしていない。

## 押出し探索の毎phase保存・復元

`codex/e05-push-resume`はPR #8の完了済みheadから分けた。毎phaseでStateStoreから新たに復元すると照合カーソルが0へ戻り、budget1では探索が100回でも完成しないことを公開APIの回帰試験で確認した。保存は`CHPUSH02`を出力し、照合カーソルを末尾u32として保持する。旧`CHPUSH01`は引き続き読め、未照合から始める。旧形式の照合途中を新形式へ保存しても進捗を保持する。入力contextのrevision検査と公開前の全node照合を維持し、cursorの範囲外値を拒否する。

新形式は照合済みReady・NoOutletを復元直後も保持するため、既存試験のReady期待値を更新した。旧形式の再照合・予算共有も継続して検証する。新payloadは4byte増え、schemaのbyte上限は自動で増えない。30に形式・費用・進捗値と実地形監査の違いを記録した。

Linux / Rust 1.98.1、CARGO_BUILD_JOBS=2で全322テスト成功（lib/host108、material_transport25、他は前節と同じ）。fmt・全target check・警告拒否Clippy・Rustdocが成功した。広域圧力作例も成功した。作例名を誤って`material_transport`として一度実行したコマンドは対象不存在で失敗し、存在する`pressure_job`で確認し直した。公開APIの押出し保存再開そのものは新しい回帰試験で検証している。最新commitのCI・外部レビューはPR提出後に確認する。E05全体は未完了である。

## ローカルへの引継ぎとPR #9のP2対応

2026-10-17（日本時間）、`A:/Project/charon-engine`で再開した。元のE04ブランチの追跡済み変更15ファイルと未追跡1ファイル（計16件）を、`local-before-e05-cloud-handoff-20261017`というstash（`1c1f742397a082da2556c52b45fd2f9b3d0cdafa`）へ保護した。元のoriginは保持し、指定リポジトリを`handoff` remoteとして追加した。fetch後の`codex/e05-push-resume`のHEADが`5ba1c92c14757cee67e42dc5ae5145ebde31ca72`に一致することを確認した。Cloud固有のCARGO_HOME/RUSTUP_HOMEは設定していない。ローカルのRust/Cargoは1.98.1である。

[READMEのP2](https://github.com/sofia-gros/charon-engine/pull/9#discussion_r4293732418)は、REST APIの`original_commit_id`でも上記`5ba1c92`への指摘と照合した。`a9d7a4490b9976cc8962a4fb9b25de0fa23e1ccf`で252件をPR #4時点の履歴と明示し、押出し修正の最新322件・品質検査成功をREADMEへ反映してpushした。[対応結果を回答](https://github.com/sofia-gros/charon-engine/pull/9#discussion_r4296298930)し、スレッドを解決した。

同headの[実レビュー結果](https://github.com/sofia-gros/charon-engine/pull/9#issuecomment-6218825222)で対象commitと重大な指摘なしを確認した。要約のcompleted表示だけで判断していない。[Rust CI](https://github.com/sofia-gros/charon-engine/actions/runs/38923773560)、[E03](https://github.com/sofia-gros/charon-engine/actions/runs/38923773546)、[E04](https://github.com/sofia-gros/charon-engine/actions/runs/38923773587)、[E05](https://github.com/sofia-gros/charon-engine/actions/runs/38923773734)の計10ジョブすべて成功。READMEのみの修正なので全検査は重複実行せず、Windowsで`pressure_push_progress_survives_a_fresh_state_read_each_phase`の1件を実行して成功した。E05全体未完了のためPR #4〜#9はマージしていない。

## 正規物質粒子の実装単位

`codex/e05-material-particles`をPR #9の確認済みheadから分けた。[35の公開契約](../../35_MATERIAL_PARTICLES.md)に、所有権・単位・費用・失敗時の保持と保存移行を記載した。

- 一cell分のtokenとflags/variation・全物質channelを粒子へ移し、空きcellへの着地時に戻す。位置固定場は保持し、block個体の暗黙分解を拒否する。
- Q16の連続位置と速度、整数DDA、同時交差の両直交cell検査、layer別の通過予約、未準備域の待機、非活動域の凍結を追加した。着地不能でも寿命で消さない。
- pool slot・全payload byte・着地channel・編集・DDA予算不足で外側取引をrollbackする。エラーを無視しても入力を失わない。作者の型付きpayload更新も同じ取引へ接続した。
- 粒子の所属regionを現在位置から決め、地形と一緒にpayloadを退避・復帰する。退避後はmetadataとslotだけ保持する。保存中の速度変更でも古い退避receiptを拒否し、復帰時はID・世代・全metadata・所属と件数を照合する。
- World形式はCHWORLD2、区域形式はCHREGN02へ更新した。CHWORLD1・CHREGN01は粒子ゼロとして読める。押出し・混合・圧力jobの形式は維持する。素材ownerはengine:particlesで、archiveの未適用epochは復帰時に処理する。

### ローカルレビュー

辞書キー・長さheaderもparticle_bytesへ計上して、値が小さいchannelを多数登録した場合の保存上限の過小計算を防いだ。着地失敗後に0速度となった粒子も再試行し、空cellに作者payloadが残る場合は上書きしない。復帰時はcell単位の位置だけでなくQ16位置・速度もmetadataと比較する。元のcell内で停止する場合に端数位置を中心へ戻さないようにした。

### 粒子単位の最終ローカル検証

Windows/MSVC・Rust 1.98.1、`-j 2`、既存cacheの`--target-dir target/e01-validation`を使用した。Cドライブの空きが約278MiBのため、TEMP/TMPだけをプロセス内で`A:/Project/charon-engine/target/e05-build-tmp`へ指定した。Cloudのhome設定は移植していない。

- `cargo fmt --all -- --check`：成功。
- `cargo check --all-targets --locked`：成功。
- `cargo clippy --all-targets --locked -- -D warnings`：成功。
- `cargo test --locked`：全342テスト成功、失敗・無視なし。内訳はlib/host108、assets24、foundation19、mixtures19、motion12、particles20、receipts15、transport25、pressure_job22、runtime28、world50。doc-testは0件。
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`：成功。
- `cargo run --locked --example material_particles`：成功。energy=900のまま放出・途中保存・壁直前の着地を行い、連続実行とcheckpointが一致した。

最後のコード変更後に全検査をまとめて実行した。未実装の全体収支・provider・Q04を、この342件で検証済みとは扱わない。GitHubの3 OS CI・実レビューはpushとPR提出後に確認する。

### PR #10のP1対応：退避粒子への空気remap

対象`e3a76f28d8e3ad4c496db62c9812a84a1d6e6571`はCI10件すべて成功したが、[退避中だけの粒子へ空気向けmappingを登録できるP1](https://github.com/sofia-gros/charon-engine/pull/10#discussion_r4296566162)を指摘された。REST APIのoriginal_commit_idも同commitと照合した。

退避粒子だけを残してremapのエラーを無視する回帰試験を追加し、修正前に規則が受理されて失敗することを確認した。engine:particlesの空気向けmappingは現在の粒子数・所属・活動状態と関係なく規則登録時に拒否する。保存復元にも同じ規則検査を適用する。拒否後にWorld・epochが不変で、保存済み区域を正常復帰してpayload・速度を保持できること、粒子が存在しない場合とresidentの場合も拒否することを検査した。正常な素材変更・退避復帰の既存試験は維持する。

合わせて、初期pool上限を08の既定profileで指定された262,144へ合わせた。保存済みWorldの容量を自動拡張する変更ではない。Windows/MSVC・Rust 1.98.1で対象22件、全344テストが成功し、失敗・無視なし。内訳は上記342件のparticles20が22となる。fmt・全target check・警告拒否Clippy/Rustdoc・粒子保存再開作例も成功した。最新commitのpush後に再レビューとCIを確認するまで、本単位の対応完了とは判定しない。

### PR #10の追加P2：保存上限とarena容量の整合

`6253655e0ef8722c63c96aa5046e30f01909f1e0`のCI10件は成功したが、[多数channelのcheckpoint上限](https://github.com/sofia-gros/charon-engine/pull/10#discussion_r4296665660)と[外側slot容量とarena容量の不一致](https://github.com/sofia-gros/charon-engine/pull/10#discussion_r4296665666)を指摘された。original_commit_idで両方の帰属を照合した。

- 64個の最大長キーの物質channelと、上限8個すべての粒子を持ち、他の容量を小さくした公開APIの試験で、実保存165,723byteに対し上限146,432byteとなる失敗を修正前に再現した。粒子payloadの8byteはキーと値の各4byte長を既に含む。実際の不足は、RAM配列が0byteでも保存するchannelのキー・格子・既定値・revision等である。Worldの上限はRAM用channel_bytesではなく、各channelのcodec byte上限とWorld側のキー・payload長を合算する。修正後は同じ上限APIで保存し、容量を満たしたまま復元して8粒子を保持する。
- 外側の粒子容量を残したままCHARNA01内部の容量だけ1へ変更した保存が、修正前は受理された。Worldの不変条件に実arena容量と明示slot容量の完全一致を追加し、復元時と通常編集の検証で拒否する。保存形式はCHWORLD2を維持する。

Windows/MSVC・Rust 1.98.1で対象24件、全346テスト成功。失敗・無視なし。fmt・全target check・警告拒否Clippy/Rustdoc・粒子保存再開作例も成功した。変更のないツリーで全検査を繰り返さず、2件の修正後にまとめて実行した。push後の最新headへの再レビュー・CIを確認してから次の実装単位へ進む。

### PR #10の追加P2：全管理情報を含む保存量の計数

`48536bbf121231afff06345160f0ffcd046a94d8`の[lease arenaの保存上限不足](https://github.com/sofia-gros/charon-engine/pull/10#discussion_r4296761521)をoriginal_commit_idで確認した。CI10件は成功していたが、指摘があるため完了扱いにしていない。

概算の加算式を置き換え、World・Arena・StateStoreの通常保存と共通のcodecを、bufferを確保しない計数用Encoderで通す。leaseだけでなく、生成要求のReady page、Jobの途中page、blockの作者状態、全arenaのslot世代・free list、remap・archive・粒子payloadを同じ経路で数える。channelは各codecの登録上限を合算する。保存形式のbyte列は変えない。`checkpoint_limit`は現在の確定状態に対する上限となり、将来の変更後まで使い回さず再計算する。Runtimeは従来どおり保存直前に呼ぶ。計数の走査費用はあるが、完全snapshotの一時確保はしない。

公開APIの回帰試験で、最大長キーのLoadOnly leaseを1,024件、World作者状態とblock状態を各128件、途中Jobと未完了生成要求を同時に保持し、算出上限で保存・復元して全管理状態を保持することを確認した。channelがない場合の算出量は実保存量と完全一致する。既存の生成Ready・block・途中Jobの保存試験にも同じ一致検査を追加した。64個の最大長channelと満杯の粒子poolの既存試験も通過した。計数Encoderのheader長・u32長上限と失敗時の不変性も検査した。

Windows/MSVC・Rust 1.98.1で全348テスト成功（失敗・無視なし）。lib109、assets24、foundation19、mixtures19、motion12、particles24、receipts15、transport25、pressure_job22、runtime28、world51。fmt・全target check・警告拒否Clippy/Rustdoc・粒子保存再開作例も成功。反復中は対象試験を使い、最終コードで全体検証を一度実行した。最新commitの実レビューとCIの確認はpush後に行う。

### PR #10の追加P2：未処理粒子の開始位置の予約

`ca2426e5170d0d9b161806fd6600814b1bd9d677`でCI10件は成功したが、[未処理粒子の現在位置も予約する指摘](https://github.com/sofia-gros/charon-engine/pull/10#discussion_r4296932890)があり、original_commit_idで帰属を確認した。レビュー要約のcompleted表示を通過とは判定していない。

phase開始時の全resident粒子のlayer・cellを一度だけ索引化し、未処理粒子の現在位置もDDAの通過点・角の検査で占有として扱う。先に動いた粒子の開始位置もphase中は解放しない。同じlayer・cellへの重複放出を拒否し、直接着地も別粒子へ重ならないよう確認する。旧保存に既存の重なりがある場合も一方の着地で他方を埋めず、待機または空き先への移動で両方の所有物を保持する。

索引はfallibleな一括確保とソートを使い、経路中は二分探索する。活動粒子ゼロでは索引を確保しない。公開の直接着地はpoolを走査するが、標準phaseの着地は開始位置の索引と経路予約で安全性を保証し、各着地でpool全体を再走査する二乗費用を避ける。保存形式は変更しない。

6件の回帰を追加した。対向粒子は両ID順・毎phase保存復帰で同じ位置とpayloadを維持する。追従は同phaseに空いた始点へ進入しない。斜めの角にいる未処理粒子、高速8方向の途中停止と保存復帰、同位置放出の失敗時rollback、旧保存の重なりで直接着地を拒否して両粒子を分離・着地できることも検証した。

Windows/MSVC・Rust 1.98.1で粒子対象30件、全354テスト成功（失敗・無視なし）。直前の348件からparticles24が30となり、他の内訳は同じ。fmt・全target check・警告拒否Clippy/Rustdoc・粒子保存再開作例も成功。反復中は対象試験を使い、最終変更後に全体検証をまとめて1回実行した。push後の最新commitについて実レビューとCIを確認する。

### E05出口の未達

| 必須項目 | 現在の判定 |
| --- | --- |
| 正規粒子・全payload所有権・DDA・容量失敗・所属region退避 | 本単位で実装・対象検証。外部レビューとCIは提出後に確認する |
| resident/inactive cell・粒子・body local cell・archive・reservoirの素材別token/mass収支 | 未完了。混合成分・理由付きreceiptを含めた全表現の保存監査が必要 |
| 重力・抗力・浮力・摩擦と標準／独自provider統合 | 未完了。型付き状態更新の入口だけで統合済みとしない |
| 広域移動の保存可能な継続Job | 未完了。phaseの件数超過rollbackを継続処理としない |
| dirty/sleep・wake依存とscalar参照のtick比較 | 未完了 |
| 公開APIとRuntimeのQ04統合・CI証拠 | 未完了。対象テストや作例のみでQ04通過としない |

E05全体の出口を下げず、各単位のレビュー・CIを確認しながら残りを実装する。すべて完了してから積み上げPRを依存順に統合し、E06へ進まず停止する。
