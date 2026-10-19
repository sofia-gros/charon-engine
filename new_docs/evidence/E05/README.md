# E05：セルと流体 — 実装・総合検査・CI成功、PRレビュー待ち

## 判定

**E05全体の実装、Q04受け入れ証拠の照合、ローカル出口検査、PR #22 head `7727c29` のGitHub CIは成功した。** PR #22に独立した外部レビュー記録はまだないため、レビュー確認・必要な指摘対応が終わるまではE05工程の最終完了とは扱わない。

現在の作業branchは`codex/e05-q04-review-fixes`で、PR #22のE05集約候補を更新している。過去の作業単位ごとの開始commitと検証結果は下記履歴に記録する。E05以降の作業順序は[工程表](../../19_IMPLEMENTATION_ROADMAP.md)に従う。

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

### 正規粒子の実装単位のレビュー・CI通過

対象commitは`5cfb0cf465e051be179c20f9817b268ca6d85214`。[対象commitを明記した実レビュー結果](https://github.com/sofia-gros/charon-engine/pull/10#issuecomment-6225740083)で重大な指摘なしを確認した。レビュー要約のcompleted表示だけを根拠にしていない。帰属する新規inline指摘はなく、既存指摘へ回答・解決済み。

CIは[Rust品質検査](https://github.com/sofia-gros/charon-engine/actions/runs/38974348824)、[E03](https://github.com/sofia-gros/charon-engine/actions/runs/38974348880)、[E04](https://github.com/sofia-gros/charon-engine/actions/runs/38974348734)、[E05](https://github.com/sofia-gros/charon-engine/actions/runs/38974348853)の全workflowが成功した（計10ジョブ）。全354テストとローカル品質検査の結果は直前項を参照する。E05全体は未完了のためPR #4〜#10は未マージのまま、次の収支統合単位へ進む。

### E05出口の未達

#### 全表現収支の追加実装とローカル省メモリ検証（2026-10-17）

作業中の `codex/e05-material-accounting` に、resident・archive・body/reservoirの作者providerを有理量で照合する収支台帳と監査付き世代保存を実装している。この単位は未完了で、E05/Q04の通過を意味しない。前段の正規粒子PR #10と、別ブランチの簡易診断PR #11を不用意に重複マージせず、契約差分を照合する。

今回のレビューでは、region archiveのchannelがhash一致でも別regionの非既定値を含み得る点を修正した。IO入口で格子coverageが完全に対象regionに入ることを検証する。標準terrain/block素材をAIRへremapした場合はmaterial-following channelを同じ候補で消去し、archive内の旧混合参照が消費済みcarrierを再計上しないようにした。小さい格子と粗い格子の境界検査、およびresident+archiveの混合carrierの消費から復帰・保存までの回帰テストを追加した。

- `cargo fmt --all -- --check`、`cargo check --all-targets --locked -j 1`：当初の物質収支変更で成功。AIR/remapと新規archive境界テスト後の再実行結果は追記する。
- `cargo test --locked --test material_accounting -j 2`：LLVMメモリ確保失敗でビルド中断。対象テストは未実行。
- `CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --locked --test material_accounting -j 1`：同様にLLVMメモリ確保失敗。テストが不合格だったと解釈しない。
- `CARGO_BUILD_JOBS=1 CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_TEST_CODEGEN_UNITS=1024 CARGO_INCREMENTAL=0 cargo test --locked --test material_accounting --jobs 1`：Windows/MSVC・Rust 1.98.1で**18件成功、失敗・無視0件**。新規の混合AIR変換テストを含む。3分49秒でビルドとテストを完走した。全体試験・Rustdoc・レビュー結果とは混同しない。

**残課題：** 物性/標準・独自Provider、広域移動の保存可能な継続Job、dirty/sleepの参照比較、RuntimeのQ04統合は未実装。したがってE06への移行条件を満たしていない。

#### Q16物性・標準/独自力学Provider（2026-10-17、別の実装単位）

次の開発branchは `codex/e05-material-forces`。全表現収支ブランチのコミットを土台とする。素材Contentに検証済みの重力・抗力・浮力・摩擦Q16とmanifestの拡張を追加した。標準 `StandardMaterialForces` と作者 `MaterialForceProvider` を同じ `MaterialMotion` channelで切り替える。力積分とDDAの一体型APIは、provider失敗と容量不足で丸ごとrollbackする。正規粒子への力や一般形状衝突を実装済みと誤認しない。

テストは `tests/material_forces.rs`、保存再開作例は `examples/material_forces.rs`。この実装単位の検証は対象CargoコマンドとPRのCI結果を確認して追記する。**E05全体とQ04は引き続き未完了**。

対象 `tests/material_forces.rs`：Windows/MSVCで低メモリ設定（`CARGO_BUILD_JOBS=1`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_PROFILE_TEST_CODEGEN_UNITS=1024`、`CARGO_INCREMENTAL=0`、`--jobs 1`）により**5件成功、失敗0件**。初回はWorldの成功取引でrevisionが進むことをテストが無視したため1件失敗し、その後に修正・再実行して全件通過した。作例 `cargo run --locked --example material_forces` も同様の低メモリdev設定で成功（2tickの連続実行と保存再開が一致）。providerの全候補を同じ候補Worldの読取り状態から評価し、その後まとめて速度channelを書き込むよう修正して候補順依存を抑えた。E05工程全体のQAとは区別する。

#### 正規粒子へ標準・独自力学Providerを接続（2026-10-17）

`codex/e05-particle-forces` では前記力学Providerを正規粒子の連続Q16座標へも接続した。速度更新と既存整数DDA/着地を同じ候補へまとめる公開APIを追加し、active粒子だけを世代付きhandleで評価する。providerが粒子では保存不能な端数を返す場合や速度上限・DDA上限を超える場合は全体rollback。参照する粒子の作者payloadは既存のWorld APIで読める。候補は全件評価後に一括書戻し、先行粒子の速度更新へ依存しない。

- 省メモリ単独テスト `cargo test --locked --test material_forces --jobs 1`：**7件成功、失敗0件**。新たに2件の粒子積分/復帰・容量/端数失敗を追加した。
- Clippy・全ターゲットのチェック・全OS CI・レビューは変更後の成否を確認して追加記録する。この結果はE05/Q04の完成判定ではない。

| 必須項目 | 2026-10-17時点の判定 |
| --- | --- |
| 正規粒子・全payload所有権・DDA・容量失敗・所属region退避 | 本単位で実装・対象検証。5cfb0cfの実レビュー・CI10件を確認済み |
| resident/inactive cell・粒子・body local cell・archive・reservoirの素材別token/mass収支 | PR #14で全表現の実量・理由付き台帳を実装。対象18テストと全OS CI10件成功。全Q04の連携監査は未完了 |
| 重力・抗力・浮力・摩擦と標準／独自provider統合 | PR #15・#16でcellと正規粒子へ追加。対象7テスト成功、両PRのCI各10件成功。Runtimeの作者provider運用・schedule統合は未完了 |
| 広域移動の保存可能な継続Job | 単一tokenの保存可能なMaterialMotionJobを追加、5件対象テスト成功。多数の同時Job・全Q04は未完了 |
| dirty/sleep・wake依存とscalar参照のtick比較 | 標準force+DDAと粉液気scalarの静止証明を追加。毎tickの結果/正規checkpoint一致試験成功。chunk単位の依存距離/圧力/粒子/作者ruleでは未完了 |
| 公開APIとRuntimeのQ04統合・CI証拠 | Materials barrierからWorld全体の監査付き編集が可能。Runtimeの複数Materials System順序、System worker 1/2/4決定性、SameTick保留順、標準Materials輸送+独立Systemのworker profile parity、生成/archive完了順のscalar輸送一致とsynthetic worker benchmarkを検証。Pressure/Job/particle/複数Providerのworker profile差、一般Provider dependency stencil、Q04全体統合は未完了 |

この表は2026-10-17時点の記録であり、後続のQ04作業記録が現在状態を更新する。E05全体の出口を下げず、すべてのQを終えてからE05検査・レビュー・PRを行い、その後E06の熱・反応工程へ続行する。

#### 単一tokenの広域DDA継続Job（2026-10-17）

codex/e05-wide-motion-job では保存可能な MaterialMotionJob を実装した。広域DDAを一交差単位のbudgetで前進させ、途中のWorld StateStore保存・復元を経ても実経路と計数を保持する。Worldを変更するのはReady後の一回のpublishのみ。入力量の失効・容量超過・不正codecはエラーとし、混合などmaterial-following channelは既存のtransport_pathを使用して追従する。保存形式はCHMMJOB1。

- 省メモリ対象テスト material_motion_job：Windows/MSVC Rust 1.98.1、CARGO_BUILD_JOBS=1・CARGO_PROFILE_TEST_DEBUG=0・CARGO_PROFILE_TEST_CODEGEN_UNITS=1024・CARGO_INCREMENTAL=0で **5件成功、失敗0**。途中StateStore保存、multi-chunk、斜め角の障害、codecの改変、revision失効、移動後payloadと参照DDA一致を検証した。
- cargo fmt --all -- --check、cargo check --all-targets --locked -j 1、cargo clippy --all-targets --locked -j 1 -- -D warnings も成功。全OS CIとレビュー結果は別途確認して追記する。単一Jobだけで一般の広域移動・dirty/sleepやRuntime Q04の出口を満たしたと判定しない。

#### 標準force+DDAの保守的Sleep/WakeとRuntime収支統合（2026-10-17）

ブランチ codex/e05-sleep-wake-reference。作者Providerや圧力・正規粒子へ省略の誤用がないよう、全active素材の停止証明がある標準のforce+DDAだけを対象とする MaterialSleepTracker を追加した。chunk revision、World/Content、設定とremap epochを照合して失効時は参照経路へ戻る。decode後のcacheは未検証として信頼せず再計算し、重力側未読込でforceにwaitingがある場合は省略しない。正規Worldが変更されなくても空のWorld取引で参照側と同じrevisionを進める。

RuntimeのMaterials barrierにWorld全体の収支監査を接続した。当初の汎用World全体の編集callbackは独立レビューで、別Worldへの置換と複数編集のエラー握り潰しによる部分公開を許すと判明したため廃止した。代わりに専用の edit_material_accounted がProfile・archive派生入力・台帳・単一WorldEdit callbackを受け取り、1つのpoison可能な候補で地形・台帳・時計を全てrollbackする。従来と同じアクセス宣言とtick制約を適用する。

- Windows/MSVC Rust 1.98.1。CARGO_BUILD_JOBS=1、CARGO_PROFILE_TEST_DEBUG=0、CARGO_PROFILE_TEST_CODEGEN_UNITS=1024、CARGO_INCREMENTAL=0を適用した対象テスト：material_forces **11件成功（失敗0）**、material_runtime **2件成功（失敗0）**。未読込隣接でwaitingを失わない検査、World毎tickのcheckpoint一致、cache保存復元時の参照fallback、source未記録でのRuntime tick rollbackを含む。
- 全OS CI・全体品質チェック・コードレビューはPR提出後に結果を追記する。Q04全体の合格証明ではない。

#### 独立レビューの重大指摘：Runtime原子性と異World分岐Sleep（2026-10-17）

PR #18の独立レビューで2種類の問題が確定した。Runtimeへ導入した汎用World編集callbackは別のWorldへ置換でき、複数World::editの片方のエラーを握り潰して部分commit可能で、未確定候補からSaveRepositoryへ外部IO副作用も起こせた。codex/e05-runtime-accounting-guard で汎用入口を撤回し、MaterialAccountingSubmissionとpoison可能な単一WorldEditを受け付ける WorldAccess::edit_material_accounted へ変更した。callback内で正常な輸送後に不正なcell書込のErrを無視しても、RuntimeはWorld候補・時計・台帳を公開しない回帰を追加した。

もう一つは同WorldId・同World revision・同chunk revisionの枝分かれWorldに、cloneした検証済みSleepTrackerを渡すと、異なる地形のままskipが通る問題。Worldへ保存しないArc cache世代を導入し、try_copyは世代を共有、地形・活動集合を変更したcommitのみ新しい世代へ進め、World::new/restoreでは別世代とした。force sleepとscalar sleepの両方にcache世代を紐づけ、異World分岐の認証済みcacheをwakeさせる回帰を追加した。

- Windows/MSVC・単一ジョブ・debug=0・codegen-units=1024・incremental無効で、material_forces **12件成功**、material_scalar_sleep **4件成功**、material_runtime **3件成功**、失敗0（合計19件）。正常・異分岐World・容量・保存再開・Runtime途中エラーに対する逐tick比較を含む。
- cargo fmt --all -- --check、cargo check --all-targets --locked -j 1、cargo clippy --all-targets --locked -j 1 -- -D warnings、cargo doc --lib --locked --no-deps -j 1 は上記修正の試験状態に対して成功。PR #18のGitHub Actions 10/10成功も確認したが、この新しい修正の全OS CIは別途確認する。
- Q04の粉液気・圧力・混合・粒子・archive・広域Job・外部Provider・Runtime複数Systemとストリーミング交互順を全体で照合する工程は未完了。E05完全実装・E06移行の判定を早めない。

#### 粉液気scalarの休止証明と参照比較（2026-10-17）

codex/e05-scalar-sleep ブランチでは単に最後のtickで交換数が0だったことでは休止を許さず、すべてのactive非空・非block素材が標準の質量条件・対角corner・両側移動・活動域のどれでも交換先を持たないことを確認する派生cacheを追加した。保存形式CHSLSCR1。復元cacheは未認証で、再びWorldで参照輸送を一度評価するまで高速化には使わない。別chunk・活動集合の変更や候補容量変更でwake、参照と同じrevisionを進める。

- Windows/MSVC Rust 1.98.1、省メモリ対象テストmaterial_scalar_sleepは**3件成功、失敗0**。封じ込め粉液気の10tick参照比較、別chunk編集と通路解放によるwake、動く粉体の未睡眠、候補予算の失敗維持、保存cacheの復元時再評価を検査した。
- cargo fmt --all -- --check、cargo check --all-targets --locked -j 1、cargo clippy --all-targets --locked -j 1 -- -D warnings は成功。親PR #18は全OS CI10件成功。ただしこの追加ブランチのCI/レビューは提出後に別途検証し、E05/Q04全体を合格扱いにしない。

#### Q04の全表現所有権連鎖とRuntimeのSource receipt・既定編集互換（2026-10-17）

codex/e05-q04-cross-representation では既存PR #4〜#10と#14〜#20の個別実装を同一Worldの連続処理としてテストした。素材mass 5と3を比率1:3でcarrier mass 4に混合した場合、3carrierの純tokenはa=6/7、b=18/7。比率を純token比1/4,3/4と誤解した初回テストは失敗し、MixtureStoreの実質量換算を確認して期待値を修正した。通常scalar移動で追従するMixtureRef、正規粒子への所有権、archive退避、World保存復元、region復帰をすべて同じbaselineと独自body/reservoir Providerの実量で照合し、候補容量不足が全体rollbackになることも確認した。

Runtimeは専用の監査付き入口でSourceの純増を理由付きreceiptとして保存し、公開後のWorld/Runtime時計と台帳を復元照合した。旧edit_terrainとWorldAccess::setを全台帳Worldで禁止する初回案は、独立レビューで設計書36の「raw編集は可能で、後続verifyで未記録増減を検出する」契約およびUI/debug状態の作者拡張性を壊すと指摘されたため撤回した。既定のraw地形編集は成功するが後続の全表現verifyが失敗すること、独立したu64作者recordは台帳と共存できることをテストする。監査を要求する作者Systemは明示して専用入口を使う。複数台帳を一つの申告で自動認証済みとは扱わない。

- 低メモリWindows/MSVC Rust 1.98.1、単一ジョブ、debug情報無効・1024 codegen units・incremental無効：material_accounting **19件成功、失敗0**、material_runtime **7件成功、失敗0**。同一Materials barrierでraw変更→監査前照合失敗→Runtime tick全体のrollbackも検証。有理量の初期失敗は実装不具合ではなくテスト期待値の解釈違いであり、修正後に全件通過した。
- 変更後の全ターゲットcheckと警告拒否Clippy、CI、コードレビューの再検証結果を別途追記する。Q04完全判定は多数Job・圧力連成・動的依存halo・Runtime複数barrier/worker・ストリーミング順まで必要。

#### Q04横断テストの実質レビュー指摘への修正（2026-10-17）

独立レビューで、当初の混合3carrierの期待token数は正しい一方、massの独立検算が欠落していたこと、候補容量不足の試験は先行する合法編集を行っておらず部分commitへの耐性証明が弱かったこと、World::restore後に派生archive cacheを再利用して外部groupの欠損を見逃す経路があったことを確認した。次の依存ブランチ codex/e05-q04-review-fixes では3件とも同じE05実体試験で補強した。

- 混合massはa=30/7、b=54/7、合計12で検証。合法のMixtureRef付き輸送後に候補予算超過を発生させ、地形・channel・台帳のcheckpointが取引前と一致することを検証。粒子の速度変更を先に行った後の不正cell編集もarenaごとrollbackする。
- World復元後、ArchiveStorageから派生cacheを作り直す。元archiveを消した場合は失敗し、復元して再読み込み後にbaselineを一致させる。region復帰後にも再集計し、退避時groupの二重計上を防止する。
- 対象単独試験 q04_scalar_mixture_to_particle_then_archive_preserves_rational_provider_totals は省メモリWindows/MSVCで修正後に成功。全対象・CI・再レビューはPR提出後に確認する。E05全体/Q04完了の判定ではない。

#### 複数material処理の一体化とRuntime Materials phase（2026-10-17）

codex/e05-material-phase-pipeline では各WorldEdit標準処理を WorldEdit::step_material_phase へ合流させた。pressure、cell force+DDA、scalar、正規粒子force+DDAを明示した設定で順番に実行し、どの後段で容量・入力に失敗しても全体をrollbackする。作者MaterialForceProviderの置換は同じtraitを通じるが、隠れた標準重力・作者state変更を注入しない。既存個別APIの意味を変更しない。

- Windows/MSVC省メモリ指定で tests/material_forces **25件成功（失敗0）**。統合APIと共通予約が不要な条件での個別solverの3tick報告・checkpoint完全一致、圧力の同tick参照結果、force更新後のscalar容量失敗、非simulated layerの拒否、セルから正規粒子位置への侵入防止を含む。
- 独立コードレビューで指定layerとWorld全域canonical粒子処理のスコープ不整合を発見した。従来World全域の力学APIを維持し、粒子DDA・粒子力積分のlayer限定APIを追加して統合phaseから使用するよう修正。2layerを同tickに順次処理する回帰で、他layerの粒子への二重積分・位置変更が発生しないことを検証した。
- 同じ設定の tests/material_runtime **10件成功（失敗0）**。監査付きのRuntime Materials Systemから統合Q16力学+scalarを3tick実行し、World/Runtime時計・収支・各tickのrestoreが一致した。さらに別SystemのSource→Sinkを同一Materials phaseで依存順に処理し、理由付き2 receiptを検証。後段の故意の失敗で前段のWorld・台帳・時計をすべてrollbackする。
- さらに material_particles **30件**、material_motion **12件**、material_transport **25件**も同一省メモリ設定で成功、失敗0。全OS CIは本変更のpush後に再確認する。
- 独立レビューで同tickのcell DDAとscalarが予約を共有せず、同じ砂を二度移動できる不具合を確認した。統合phase専用の共有CellReservationsを導入し、単独solverの既存挙動は維持した。角の横・縦検査点も先行DDAの予約に反した通過を許さない。World全域の正規粒子APIもsimulated=false layerを対象から除外し、非参加layerの所有物を保持する回帰を追加した。
- 本実装の全11対象スイートは、単一ジョブの省メモリ設定で全件成功。E05最終マージ順は [PR_MERGE_PLAN.md](PR_MERGE_PLAN.md) の未確定台帳を参照し、Q04終了時に最後のPR本文へ確定して記載する。
- フォーマット/Clippy・全OS CI・独立レビューは変更提出後に確認する。Q04の局所dirty/haloや広域Jobの同時予約・Provider永続化は未完了、E05全体の完成根拠にはしない。

#### E05統合PRを増やさず修正を集約（2026-10-18）

PR #22を新しいPRへ細分化せず、PR #10をbaseとするE05統合集約のDraftへ再構成した（commit cf8f178）。元PR #14〜#21のブランチとレビュー履歴は保存し、E05完成前に破壊的なCloseを行わない。Q04未達のため最終のマージ番号列は今は確定しない。Codexへの最終指示・除外元・merge後のゲートは [PR_MERGE_PLAN.md](PR_MERGE_PLAN.md) で管理する。

追加の独立監査では、標準scalarの斜め移動で角の予約セルを空きと誤認し得る箇所、World全域粒子stepでnon-simulated layerを更新する余地、混合の全表現収支profileでMixtureRef channelの既定値NONEを検証していない箇所を確認した。セルの予約を角判定に追加し、non-simulated layerの粒子はWorld全域stepでも保持し、混合channelの初期値を必ずNONEと確認する。対象回帰ではnon-simulated layerの粒子不変、MixtureRef不正defaultでのarchive監査入口の拒否を検証した。

- Windows/MSVC（1ジョブ、debug=0、codegen-units=1024、incremental無効）で material_accounting、material_forces、material_motion、material_transport、material_particles、material_runtime の6対象スイートを検査してすべて成功。混合defaultの追加単独回帰テストも成功した。全OSの更新後CIとQ04全工程出口は別途検証する。

#### 旧診断・一様力学APIの移植と広域移動Jobの同tick調停（2026-10-18）

PR #11〜#13固有のresident/archive別carrier診断、layer別混合成分診断、旧一様力学をE05集約候補へ移植した。全表現台帳ではcell/particle・resident/archiveの利用者向け内訳と旧一様浮力式を同じAPI/結果で提供しないため、旧診断は読み取り専用の補助APIとして、UniformMaterialForcesは作者が選択した場合のみの互換Providerとして残す。どちらもE05全体の保存監査・標準式の代替にしない。既存の #11〜#13 を置換済みとしてcloseする判断は、今回の移植部分のコードレビューを終えるまで保留する。

- Windows/MSVCの省メモリ設定で `cargo test --locked --test material_inventory --test material_mixtures --test material_forces --jobs 1` を実行し、material_inventory 6件、material_mixtures 21件、material_forces 28件が成功。これらは互換API・既存の混合/物性回帰で、全体テストではない。
- 独立した複数MaterialMotionJobについて、全Jobの初期入力を事前検証し、同一layerの経路・source/destination・斜め角検査点の重なりを全体拒否する `publish_material_motion_jobs` を追加。独立経路を固定順で一候補に公開する。
- `cargo test --locked --test material_motion_job --jobs 1` は6件成功。独立Job二件の同tick移動と、同一destinationでの競合におけるcheckpoint完全一致rollbackを確認した。
- バッチ予約scratchは活動chunkあたり4096bitを一layerずつ保持し、総経路長に対する走査とJob数に対する順序付けがある。メモリ上限は活動chunk数と同時Jobの経路長で増える。圧力・押出しJobやscalar/particle solverとの横断調停、性能profile比較は未実施であり、最適化の効果やE05/Q04完了を主張しない。
- 最初の `cargo fmt --all` はmanaged worktreeのpath canonicalizationでOS error 5を返した。書式確認は別途manifest指定で再試行する。全target check、Clippy、Rustdoc、全OS CI、独立レビューは未確認。

この後、Q04で明記されている押出し/移動Jobの衝突も部分的に扱うため、同じ一括入口をMaterialMotionJobとPressurePushSearchへ拡張した。両Job群を元の同一World状態から先行検証し、同一layer上のsource・destination・経路・DDA角確認範囲が交差すれば全体を拒否する。PressurePushSearchは完成時の圧力snapshotに従う。PressureJobの同tick再計算・出力を押出し探索へ反映する接続は未実装である。

- 省メモリWindows/MSVCの `cargo test --locked --test material_motion_job --jobs 1` は7件成功。材料DDAと押出しの独立経路同時公開、および同一destinationの競合rollbackを含む。型不一致のfixture初回失敗を `i64` の明示に修正して再実行した。
- `publish_pressure_push`単独入口も一括経路準備helperを再利用し、復元直後でbudget検証が完了していない探索が公開されないことを入口で検査する。`material_transport` 25件も同じ実行設定で成功した。
- 圧力押出し経路の一時Vecは探索node総数ではなくparent chainの実経路長で確保する。同時Job数は `WorldLimits.jobs`、活動chunk照合・保存node・経路検証の保守的な合計単位は `WorldLimits.job_cells` で上限付ける。重複経路は順序済み予約中に早期拒否するため、競合する多数Jobの経路コピーを全件保持しない。
- ユーザー方針に従い、以後の工程中は対象スイートと機能連携テストを中心にする。全target check・全体ClippyはE05実装の最後にまとめる。先行実行したall-target checkは成功したが、レビュー粒度の調整後は再実行していない。Clippyはworktree内の設定読込みでOS error 5となり、再試行を中断した。

#### PressureJob群と移動/押出しJobの同tick原子公開（2026-10-18）

完成済み広域圧力jobを同じ初期Worldから検証して公開する`WorldEdit::publish_pressure_jobs`を追加し、既存`publish_pressure_job`は単job用の互換入口として同じ実装へ委譲する。job数、活動chunk内4096 cellの再採取・保存node照合、公開scratch byte数を`WorldLimits.jobs` / `WorldLimits.job_cells` / `WorldLimits.job_bytes`で制限する。全jobのcontext・Ready状態・保存入力・channel更新を先に検査し、同じ出力channel/座標の重複は全体拒否する。channel単位に複数jobの容量を合算して検証した後、channelと出力座標の固定順で一候補へ反映する。出力Vecと最大channel group分の容量検査scratchは出力cell数に比例する。

- `WorldEdit::publish_material_motion_pressure_jobs_and_pushes`を追加した。圧力出力・広域DDA Job・PressurePushSearchを未変更Worldから準備し、件数と統合work予算を検査した後、移動/押出し、位置固定圧力channelの順で一つのWorldEdit候補へ公開する。PressurePushSearchは従来のpressure snapshotを使い、PressureJob出力から再探索しない。圧力出力scratchはjob_cellsとjob_bytesの両上限を検査する。
- 新圧力fieldで押出し先を再評価する`WorldEdit::publish_pressure_job_then_search_push`も追加した。PressureJob出力後に新しいPressurePushSearchを作り、指定budget内にReadyなら経路輸送も同じWorldEditで公開する。探索が続く場合はpressureとpending searchを返し、呼出側が同じ取引でStateStoreへ保存できる。pressure公開後の探索開始/輸送失敗は全体rollbackする。
- 省メモリWindows/MSVC設定で`cargo test --locked --test material_motion_job --test pressure_job --jobs 1`を実行し、material_motion_job 9件、pressure_job 27件が成功した。新規の結合回帰は、PressureJobのsource pumpが設定した新圧力から出口探索・輸送を行い、予算中断後のStateStore保存・World復元・探索継続も検査する。pressure出力競合時には移動を含むcheckpoint全体が不変であることも検査する。別のscratch上限試験はjob_bytes不足時の失敗とcheckpoint不変を検証する。

最新のarchive境界回帰を加えた後、Windows/MSVC単一jobで`cargo test --locked --test material_motion_job --jobs 1`を実行し、**10件成功、失敗0件**。進行中MaterialMotionJobをStateStoreへ保存し、World lease解放・region archive退避・World復元の後、古いJobのstepとpublishが失敗して退避済みcheckpointを保持することを確認した。PressureJobとPressurePushSearchのarchive切替境界は別途未確認。
- 初回コンパイルでは新規assertの型制約と準備出力struct field漏れ、pump channelの原点変換漏れが見つかり、それぞれ修正後に対象試験が成功した。rustfmtはmanifest指定で実行。all-target check、Clippy、全体テスト、性能比較、独立レビューはまだ行っていない。scalar/particleとの横断予約は未実装。E05/Q04を完了とは判定しない。

#### Materials phaseでcell予約と正規粒子DDAを接続（2026-10-18）

Materials phaseのcell motion/scalar共有予約を、後段の正規粒子DDAにも渡す。粒子の開始cell・通過cell・角の検査cellが同じtickのcell solver予約と重なると、粒子速度を保って待機する。cell solverを含まない粒子単独phaseでは追加scratchを確保しない。

- 回帰 `integrated_particle_motion_respects_cell_motion_and_scalar_reservations` は固定壁から出る粒子がcell motionの移動元だったcellへ進入しないこと、cell motionが1tokenを1回だけ動かすこと、scalar交換がないことを検証する。
- Windows/MSVC・単一job・incremental無効で `cargo test --locked --test material_forces --jobs 1 --target-dir A:\\Project\\charon-engine\\target\\e05-legacy-port-build` を実行し、**29件成功、失敗0件**。個別回帰も修正後に再実行して成功。
- Runtime回帰として、Materials SystemのSameTick中断/再開とScene pause中のWorld-bound Materials System抑止を追加。pause中のWorld tick/物質位置維持、再開後の一回実行、pending中に時計を公開せず完了時に一度進めることを確認した。`cargo test --locked --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-legacy-port-build` は**12件成功、失敗0件**。
- 初回fixtureでは砂を同じ位置へ置いたことでscalar交換が先に成立し、セル移動の期待値を満たさなかった。fixtureを固定壁に変え、さらに保存済み速度を変えないProviderを用いて対象経路を分離した。
- `step_material_phase_with_jobs`によりMaterialMotionJob/PressurePushSearchとscalar/particleの横断予約を追加した。後続のQ作業単位でPressureJob出力もMaterialPhaseへ接続したが、PressureJob後の新圧力再探索、依存halo wake、worker/IO順序、Runtime schedule上での保存可能な進行Job統合は未達。性能benchmark、E単位総合品質検査、全OS CIは未実施で、E05/Q04は未完了。

#### Q04作業単位：広域Jobと標準MaterialPhaseの予約共有（2026-10-18）

`WorldEdit::step_material_phase_with_jobs`を追加し、Ready済みMaterialMotionJob/PressurePushSearchの原子公開と、その後のcell force+DDA・scalar・正規粒子DDAを一つのWorld候補へ統合した。Job経路、source/destination、移動DDAの角確認cellを共通CellReservationsへ登録し、標準solverが再利用しない。Provider力積分も同予約cellを除外し、Jobが移動したtokenへの二重力積分を防ぐ。後段scalarの容量失敗でJobの先行公開を含むWorld checkpointが不変であることを検査した。

単体レビューでは、初回案のままforce integratorを呼ぶと、広域Jobの公開後に同じtokenへProviderが再適用されることを特定した。予約cellをProvider候補から除外する修正を入れ、試験Providerが追加する下向き加速が広域移動tokenへ二重適用されないことを統合回帰で確認した。既存単独solverは空予約で従来どおり実行する。

- 単体/結合試験：Windows/MSVC省メモリ設定（単一job、incremental無効）で `cargo test --locked --test material_motion_job --jobs 1` を実行し、**13件成功、失敗0件**。MaterialMotionJob/PressurePushSearch経路上のscalar再移動防止、Provider二重適用防止、後段失敗時の広域Job rollbackを検査した。
- 関連既存統合試験：`cargo test --locked --test material_forces --jobs 1` を実行し、**29件成功、失敗0件**。標準force/phase共有予約、particle DDA、単独solverとの既存動作を再確認した。
- `cargo fmt --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --check`、`cargo doc --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --locked --lib --no-deps` 成功。全target検査、E単位総合試験、全OS CI、外部独立レビュー、性能benchmarkはこのQ作業単位では実施していない。
- 予約scratchはjob_bytes内でJob batchと共有bitsetを合算する。検証量は既存batch内の再検証も含め二回分をjob_cellsに計上する。追加workはactive chunk照合とjob経路/node数に比例し、scratchはjob数・押出し経路長・active chunk数に比例する。比較benchmark未実施のため性能改善を主張しない。

PressureJob出力・新圧力からの再探索を標準MaterialPhaseへ同時接続する機能、Runtime schedulerによる保存可能Jobのbudget進行、依存halo wake、worker/IO順を含む参照一致は未完了。これはQ04/E05の完了証拠ではない。Q単位の試験・レビュー後に残るQ項目へ進み、E05の全Q完了後にE単位の総合検証・レビュー・PRを行う。

#### Q04作業単位：PressureJob出力とMaterialPhaseの同tick統合（2026-10-18）

`WorldEdit::step_material_phase_with_pressure_jobs`を追加した。PressureJob出力、MaterialMotionJob/PressurePushSearch公開、その後の標準pressure・cell force+DDA・scalar・particleを同じWorld候補へ連結する。移動Job/押出し経路が対象layerにあるときは前段の共有reservationを後続solverとProvider力積分へ渡す。PressurePushSearchは従来どおり元Worldの圧力snapshot経路を使い、PressureJob出力後の再探索は別APIの機能として扱う。

`pressure_jobs_cost`を準備/公開経路とMaterialPhaseの事前上限検査から共用し、出力scratch・Job batch・共有reservationを`job_bytes`へ、pressure検査と移動Jobの必要な再検証を`job_cells`へ合算する。通常phaseと広域Jobの共有予約を使わない構成では追加分の二重検証workやbitsetを加算・確保しない。PressureJob cost計算は空入力時にchunk走査を行わない。

- 単体・結合試験：Windows/MSVC、単一build jobで `cargo test --locked --test material_motion_job --jobs 1` を実行し、**15件成功、失敗0件**。PressureJob出力と移動Jobの同tick公開後にscalarを実行すること、後段scalar容量失敗でpressure・移動Jobの双方がcheckpointへ残らないことを検査した。初回の成功fixtureでscalarが水を1回輸送した結果を確認し、参照結果を期待値に反映した。
- 関連単体試験：`cargo test --locked --test pressure_job --jobs 1` は**27件成功、失敗0件**。PressureJobのbudget/保存再開/出力容量と既存の新pressure PushSearch接続を再確認した。
- `cargo fmt --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --check` と `cargo doc --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --locked --lib --no-deps --target-dir A:/Project/charon-engine/target/e05-legacy-port-build` 成功。
- 単体レビューではPressureJob scratch上限計算の重複実装を共通化し、共有予約不要時に二重のmovement workを予算へ加えないよう修正した。E単位総合試験、全target検査、外部独立レビュー、全OS CI、性能benchmarkは未実施。

PressureJob出力後に新snapshotからPressurePushSearchを作り直して同じMaterialPhaseへ渡す機能、Runtime schedulerによる保存可能Jobのbudget進行、dependency halo wake、worker/IO順の参照一致は残っている。このQ04作業単位だけではE05/Q04を完了扱いにしない。

#### Runtime Materials Systemで各種保存可能Jobをbudget進行（2026-10-18）

Runtimeの`ContinueSameTick`とbarrierを使い、作者Systemの一時fieldへJob状態を隠さずWorld StateStoreを正規進捗にする結合回帰を追加した。`MaterialMotionJob`と`PressureJob`をそれぞれWorld schemaへ登録し、Materials Systemは各resumeで候補Worldからjobを再読込してbudget=1で進め、`WorldAccess::set`とbarrierで保存する。ReadyになるまでWorld tickは公開されず、Ready後の輸送/圧力公開とtick公開は同じRuntime候補の完了時に行う。MaterialMotionJobの3交差とpump入力を持つPressureJobを複数poll間で完了し、最終World checkpointからReady状態の各Jobを復元できることを検査した。

- 単体/結合テストと単体レビュー後、Windows/MSVC省メモリ設定で`cargo test --locked --test material_runtime --jobs 1`を実行し、**14件成功、失敗0件**。既存の監査付きMaterials barrier、SameTick/pause回帰を含む。
- 変更した統合試験はRust 2024版の`rustfmt`で整形した。`cargo fmt`はmanaged worktreeのpath canonicalizationでOS error 5となったため、対象ファイルだけをformatterへ渡した。
- 各Pending中の候補WorldはRuntimeの公開Viewから意図的に見えない。テストは次resumeのSystemがStateStoreからJobを読み直して有限回でReadyへ進むこと、完了前に公開tickが進まないこと、公開後checkpointからJob stateを復元できることを確認する。Pending候補のcheckpoint保存API自体はこのテストで追加していない。
- 変更はテスト・設計文書・README/evidenceの更新に限る。全target check、Clippy、E単位総合試験、全OS CI、独立外部レビュー、性能benchmarkはE05全Q完了後に行う。この記録時点では複数Job間の同tick依存をRuntimeが自動調停する機能、dependency halo wake、PressureJob新snapshot探索と標準MaterialPhaseの一括統合、worker/streaming順の逐tick一致は未達で、E05/Q04は未完了だった。後続作業で統合済みの項目は次の記録を参照する。

#### Q04作業単位：PressureJob新snapshot探索とMaterialPhaseの統合（2026-10-18）

`WorldEdit::step_material_phase_with_pressure_job_search`はPressureJobの出力後に新しいpressure snapshotを読み、budget付きPressurePushSearchを開始する。Searchingならpressure出力とsearchを同じWorld候補へ置き、呼出側がsearchをWorld StateStoreへ保存できる。`resume_material_phase_with_pressure_search`は保存・World復元後のsearchを次budgetで進め、Ready後は押出し経路を標準MaterialPhaseと共有予約へ登録してからcell/scalar/particle処理を行う。terminal searchのStateStore keyは`WorldEdit::remove_state`で除去できる。汎用削除口はMixtureStore・receipt・balance logを拒否する。初回APIはpressure適用前に生成された広域MaterialMotionJobを受け付けず、pressure公開後のWorldを基準に作られたJobはresume APIで合流できる。

- 結合試験はPressureJobがpump pressure 900を新snapshotへ公開し、budget=1のSearchingをStateStoreへ格納、World checkpointから復元後にbudget=1ずつ再開する。圧力公開後のWorldから作ったMaterialMotionJobをresumeへ合流させ、押出し・広域移動を共有予約で同じ候補に公開し、出口を含む流路のtoken数・scalar二重輸送なし・terminal state除去を検査する。別の結合試験では後段scalar候補容量失敗時にpressure出力・地形・作者状態のcheckpointが全て不変であることを検査する。
- Windows/MSVC省メモリ設定（`CARGO_BUILD_JOBS=1`、`CARGO_INCREMENTAL=0`）で候補worktree専用build directoryを使い、`cargo test --locked --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --test material_motion_job --test pressure_job --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-candidate-build` を実行。`material_motion_job` **18件成功**、`pressure_job` **27件成功**、失敗0件。新規回帰で最初に期待した東側空cellより上側の空cellが探索されることが分かり、fixtureの横流路を壁で閉じて出口を一意化した。広域圧力jobと押出し探索をStateStoreから復元し、region archive退避後にstepが失効し、checkpointを保持する回帰も含む。
- `cargo test --locked --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-candidate-build` も実行し、**15件成功、失敗0件**。`SystemDescriptor::after`で順序付けたpressure開始Materials Systemと探索再開Materials Systemから新統合APIを呼び、PressureJob・押出し探索・pressure公開後のMaterialMotionJobを同じphase候補へ渡す。PressureJobと押出し探索をbudget=1でそれぞれ複数barrierに分け、StateStoreを読み戻して完了すること、完了までRuntime tickを公開しないことを確認した。Runtime barrierのedit callbackは提出時に同期結果を返さないため、完了markerはWorld StateStoreを介して次pollへ伝える。
- 完了済みPressureJobのStateStore keyは、新pressure snapshotと押出し探索を開始する同じ原子編集内で削除する。完了後にPressureJobとPressurePushSearchの両terminal keyが残らないassertを追加し、`runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers`を単独実行して成功した（1件成功、失敗0件）。
- 単体レビューでPressureJob出力前に用意したMaterialMotionJobがchunk revision更新後に失効し、探索中にも保持されるAPI引数の問題を発見した。初回APIからその引数を除き、Ready前のsearch再開APIだけがpressure公開後のWorldから生成した移動Jobを受け取る設計へ修正した。修正後、上記2試験群を再実行して成功した。
- 追加レビューで探索生成・revision照合が全loaded chunk mapを走査する点を確認したため、work上限ではactive chunk数ではなくloaded chunk総数を計上するよう修正した。visited/search scratchは従来どおりactive chunk数で見積もる。
- rustfmt実行後、上記3 suiteを候補worktree専用targetでまとめて再実行し、**計60件成功、失敗0件**（material_motion_job 18、pressure_job 27、material_runtime 15）。全target check、Clippy、E05全体テスト、Rustdoc、全OS CI、E単位総合レビュー、比較benchmarkは未実施。探索とphase予約の費用は既存`job_cells`/`job_bytes`およびPressurePushSearch node上限へ事前計上する。実測がないため性能改善は主張しない。
- dependency halo wake、複数providerの横断、Runtime複数System/worker/IO順の参照一致などQ04/E05の残る固定条件があるため、E05/Q04は未完了。E単位の検査・総合レビュー・PRは全Q完了後に行う。

#### Q04作業単位：生成worker完了順後の材料輸送参照一致（2026-10-18）

既存の生成worker完了順回帰を材料状態まで延長した。4chunkの同一World入力を作り、生成を1/2/4 workerで並行処理する。worker結果をcompletion順または逆順でsubmit・公開して得たWorldへ同じ活動leaseと粉体を準備する。標準scalar輸送とtick進行を4回行い、各回の`MaterialTransportReport`と完全World checkpointが4構成で一致することを検査する。これにより生成worker数と生成結果完了順が、後続するQ04材料輸送の正規状態を変えないことを示す。archive IO worker順、simulation system worker count別の結果、複数Providerとの組合せまではこの試験で証明しない。

- 対象回帰：`generated_chunks_wait_for_explicit_publication_and_ignore_worker_completion_order`。`tests/world.rs`だけを候補worktreeで変更した。
- Windows/MSVCの単一build job・incremental無効で、`cargo test --locked --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --test world generated_chunks_wait_for_explicit_publication_and_ignore_worker_completion_order --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-candidate-build`を実行し、**1件成功、失敗0件**。fixtureのChunkRegionは幅ではなく半開端点を取るため、初回の範囲指定で存在しない第三chunkを含めて失敗した。端点を修正して再実行し、成功した。
- 共有`tests/world.rs`fixtureへ粉体定義を追加したため同target全51件も実行した。sandbox内の初回実行では一時save fixture作成の19件がOS error 5となったが、同一コマンドをsandbox外で再実行し、**51件成功、失敗0件**。OS permission failureであってsimulation assertion failureではないことを確認した。
- 単体レビュー：1/2/4 workerは同時に開始し、各batchの完了channel順を保存する。逆順variantはsubmit順だけを明示的に反転する。全構成の生成checkpointと、その後4tickの輸送report/checkpointを比較する。候補容量32768はactive 4chunk内の候補を収める。
- 対象回帰`generated_chunks_wait_for_explicit_publication_and_ignore_worker_completion_order`を変更後に単独実行し、**1件成功、失敗0件**。全`world`スイートは今回未実行。E05全体検査、実filesystemのIO故障、simulation system worker数1/2/4の比較、性能計測は未実施。Q04/E05は未完了。

#### Q04作業単位：archive load/restore順後の材料輸送参照一致（2026-10-18）

二つのresident regionを同じ内容address付きarchiveへ退避する。二つの`RegionLoadTask`を独立worker threadで同時実行し、テスト用ArchiveStorageの読込待ちをregionごとに変えて完了順を両方向へ固定する。完了通知の順に`restore_region`を一つのWorldEditでcommitした後、同じ3chunk活動域へ戻して標準scalar輸送とtick進行を4回実行し、毎tickの`MaterialTransportReport`と完全World checkpointを比較する。fixtureは読込をメモリから返し、threadと遅延を使って順序競合を再現する。実filesystemの障害・遅延特性そのものは対象外。

- 対象回帰：`archive_io_completion_order_preserves_scalar_transport_each_tick`。物質はarchive対象のchunk -1と1へ配置し、読み込み後に両Worldへ同じSimulate leaseを取得する。標準輸送候補の上限8は4候補を十分に収める。
- 単独の回帰実行は**1件成功、失敗0件**。続けて候補worktree専用targetで`cargo test --locked --manifest-path A:/Project/charon-engine/target/e05-runtime-audit/Cargo.toml --test material_transport --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-candidate-build`を実行し、**26件成功、失敗0件**。
- 単体レビューでは二つのWorldのarchive内容、退避順、初期編集、lease、tick進行を共通化し、Barrierで二workerの開始を揃え、worker遅延を入れ替えるとrestoreの完了順が必ず逆になるassertを追加した。workerは個別のStorage cloneから同じ不変archiveを読むため、同一Storageへの並行書込は発生しない。Readyになった二regionはcompletion順で一つのWorldEditへ渡し、同一barrierの原子公開を検査する。各tickでreportとcheckpointの両方を比較し、初回だけの一致に留めていない。
- 修正後に対象試験を再実行し、**1件成功、失敗0件**。続けて全`material_transport`スイートを実行し、**26件成功、失敗0件**。simulation system worker数1/2/4、実filesystemのIO障害、性能計測、E単位総合検証は未実施。Q04/E05は未完了。
- 追加fault回帰では速く完了したregionの復帰を同じWorldEdit内で先に適用した後、遅れて完了する別regionのarchive読込をIO errorにする。取引全体が失敗し、resident/archived状態を含むWorld checkpointが一byteも変わらないことを確認する。修正中に初回の順序assertが失敗したが、原因はfaultを遅らせる前に返していたfixtureの順序であり、遅延後にfaultを返すよう直して再実行し成功した。
- 変更後の全`material_transport`スイートは**27件成功、失敗0件**。simulation system worker数1/2/4、実filesystemのIO障害、性能計測、E単位総合検証は未実施。Q04/E05は未完了。

#### 標準scalar候補cacheと粒子占有の回帰（2026-10-18）

標準scalar輸送では、同じWorldの非保存世代・Content・remap epoch・layer・候補上限とchunk revisionが一致する場合に、非空cellがすべてSolidのchunkから候補座標を再列挙しない派生cacheを追加した。候補数は維持し、solid座標だけをsolver入力から省く。動的chunk候補は再走査して標準のtick parity順にsortする。commitで地形が変化すればWorld世代が更新されるため、cache全体は次回不一致となる。

- `tests/material_scalar_sleep.rs`へ、静止solid chunkと、正規particleが粉体の直下移動先を占有する活動chunkを同時に置く回帰を追加した。particle占有をsleep証明が前提にせず、地形変更のない4tickについてreference輸送reportと完全World checkpointの一致を検査する。さらにcandidate_limitを狭めた再評価が容量errorとなり、World checkpointを変えないことを確認する。既存の4 testも含めテスト対象は5件となった。
- Windows/MSVC、`CARGO_BUILD_JOBS=1`、`CARGO_INCREMENTAL=0`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_PROFILE_TEST_CODEGEN_UNITS=1024`、候補専用targetを使用。`cargo test --locked --test material_scalar_sleep --test material_transport --test material_forces --jobs 1`で**61件成功、失敗0件**（scalar sleep 5、material transport 27、material forces 29）。fixture初回実行はparticle放出元が空cellとなる誤配置で失敗したため、既存の物質所有権契約に合わせて放出元にsandを置き、cell→particle変換後に粉体を上側へ配置して修正した。
- 静止solid chunkの走査省略に対するbenchmarkやP50/P95/P99測定は未実施。性能改善を主張しない。dynamic candidateの再走査・global sort、Runtime simulation worker数1/2/4、dependency halo wake、実filesystem IO fault/delay、E単位総合試験・レビュー・PRも未実施。E05/Q04は未完了である。

#### archive順序・fault回帰をDirectoryArchivesへ接続（2026-10-18）

従来の二つのarchive順序回帰がメモリfixtureを使っていたため、`archive_io_completion_order_preserves_scalar_transport_each_tick`と`archive_worker_failure_rolls_back_earlier_ready_region_publication`を本物の`DirectoryArchives`へ切り替えた。二つのfile-backed RegionLoadTask workerは独立したDirectoryArchives handleから同じdirectoryを読み、異なる待ち時間で完了順を逆転する。fault側では遅れて読むarchiveの`.blob`を実際に削除し、DirectoryArchivesのファイルopenがIO errorになる状態で先にReadyしたregionも含むWorldEdit全体のrollbackとcheckpoint一致を確認する。

- 最初のWindows/MSVC実行では`std::env::temp_dir()`のcanonicalizeがOS error 5となった。workspace内の専用temp folderを`TEMP`/`TMP`に設定しても拒否されたため、sandbox内失敗をテスト不具合と誤認せず同じ限定suiteをsandbox外で再実行した。
- Windows/MSVC、single build job、省メモリ設定、候補専用targetで`cargo test --locked --test material_transport --jobs 1`を実行し、**27件成功、失敗0件**。実filesystemの欠損blobによるIO failureと遅延readの両回帰を含む。
- これは実ファイルread・欠損の検証であり、device unplug、disk-full、power lossなど全OS/device故障を網羅したfault campaignではない。simulation System worker数1/2/4比較、dependency halo wake、benchmark、E単位総合試験・レビュー・PRは未実施。Q04/E05は未完了。

#### Q04作業単位：scalar sleepとMaterials phaseの同tick原子連携（2026-10-18）

`WorldEdit::step_material_phase_with_scalar_sleep`を追加し、標準scalarの休止証明を統合Materials phaseの共通予約へ接続した。trackerは値で受け渡し、scalar後段の粒子処理などを含むphase全体が成功した場合だけ新しい値を返す。World候補のcommit前にtrackerだけを進めない。証明はcommit予定の次revisionと各active layer pageのArc identityへ結びつける。同revisionで異なる内容を持つ分岐から証明を誤用しないためで、page identityは外部保存しない。decodeした証明は未認証のまま参照経路を一度通す。正規粒子が対象layerにいる場合は移動先占有が変化し得るためsleep証明を作らず、参照scalarへ戻す。

- `tests/material_scalar_sleep.rs`へ、封じ込めた粉体・液体・気体の統合Materials phaseを通常phaseと8tick比較する回帰を追加した。初回は証明作成、その後7tickはsleepし、毎tickのscalar reportと完全World checkpointが一致する。正規particleを含む候補上限失敗をscalar処理後へ注入し、後段failure時にWorld checkpointを保持する。
- `MaterialPhaseReport`へsleep/wake診断を追加し、Runtimeの監査付きMaterials callbackからtrackerを同じWorld候補のStateStoreへ戻す回帰を`tests/material_runtime.rs`に追加した。動的粉体の3tickでRuntime時計・地形・収支検証・checkpoint復元を確かめる。このRuntime例では証明が作られず、sleep実行自体は独立のWorldEdit phase回帰で検証する。`new_docs/41_SCALAR_MATERIAL_SLEEP.md`、`new_docs/42_Q04_CROSS_REPRESENTATION.md`、`new_docs/43_MATERIAL_PHASE_PIPELINE.md`、ルート`README.md`を同じ仕様へ更新した。
- 変更したRust source/testへrustfmt `--edition 2024`を実行。Windows/MSVC、単一build job、incremental無効、test debug無効、codegen-units=1024、候補専用targetで`cargo test --locked --test material_scalar_sleep --test material_forces --test material_runtime --jobs 1`を実行し、**51件成功、失敗0件**（scalar sleep 6、material forces 29、material runtime 16）。一度目の新規fixtureは空cellを粒子化しようとして失敗したため、砂を置いた既存の所有権変換手順に直して対象suiteを再実行した。
- benchmark/P50/P95/P99測定、Runtimeで静止証明を再利用するtracker保存復帰のsleep継続、simulation System worker数1/2/4、依存stencil/halo wake、複数Providerの同tick横断、E単位総合テスト・総合レビュー・PRは未実施。Q04/E05は未完了。

#### Q04作業単位：標準scalarのchunk局所sleepと1cell halo wake（2026-10-18）

標準scalar輸送の既存派生cacheへchunk局所停止証明を追加した。各active chunkの非solid候補が一つも合法に動けず、1cell haloに動的な非block候補がなければ、そのchunkの候補座標をsolverから省く。haloは四辺64点と角4点の状態を、既定AIRを省いたsparse signatureとして保持する。隣接chunkの内部だけが動けばsignature一致により局所sleepが継続し、haloが変われば局所をwakeして参照solverへ戻す。World全体の`sleeping`診断と分け、`sleeping_chunks`で局所省略数を返す。対象layerに正規particleがいる場合は局所証明も作らない。

- `tests/material_scalar_sleep.rs`の`chunk_sleep_keeps_a_blocked_region_asleep_while_adjacent_chunk_moves`で、隣接chunk内部の粉体移動が続く間に塞がれた領域の局所sleepを保ち、隣接1cell haloへ粉体を置くとwakeすることを確認する。各tickでreference輸送reportと完全World checkpointを比較する。
- `new_docs/41_SCALAR_MATERIAL_SLEEP.md`、`new_docs/42_Q04_CROSS_REPRESENTATION.md`、ルート`README.md`の状態説明をコードへ合わせた。halo比較は最大132点/active chunk、sparse signatureの実保持は非既定値分。候補座標は動的な非solid候補あたり1点を保持する。benchmarkとP50/P95/P99は未実施で性能改善を主張しない。
- Windows/MSVC、単一build job、incremental無効、test debug無効、codegen-units=1024、候補専用targetで`cargo test --locked --test material_scalar_sleep --jobs 1`を実行し、**7件成功、失敗0件**。局所sleep・隣接移動・halo wakeを含む。続けて`cargo test --locked --test material_scalar_sleep --test material_forces --test material_runtime --jobs 1`を実行し、**52件成功、失敗0件**（scalar sleep 7、forces 29、runtime 16）。
- Q04横断会計の`q04_scalar_mixture_to_particle_then_archive_preserves_rational_provider_totals`を単独実行し、**1件成功、失敗0件**。混合scalar→正規粒子→archive退避・World checkpoint復元・region復帰にわたり、body/reservoirの2所有Providerを含む有理量台帳が一致する。
- 同じ横断試験に、標準scalar移送・粒子化・body所有record変更の後で別Providerの最終会計callbackを失敗させるfault注入を追加した。全20件の`material_accounting` suiteを実行し、**20件成功、失敗0件**。sandbox内の世代保存1件はOSアクセス拒否で失敗したが、同じsuiteをsandbox外で再実行して成功した。失敗後のWorld checkpointも取引前と完全一致する。
- 一般Providerの宣言dependency stencil、Materials複合処理を含むsimulation System worker数別参照比較、複数Provider横断、別表現のsleep証明、Q04/E05総合検証・独立レビュー・PRは未実施。Q04/E05は未完了。

#### Q04作業単位：Runtime System worker数の決定性・性能比較（2026-10-18）

独立を明示した4つのRuntime Systemを同じscheduleで実行し、worker数1/2/4の完全World checkpoint一致を確認した。2つのSystemが同じtickにworker実行を終えた後、先行Systemが`ContinueSameTick`を返すケースも追加し、先行Systemの再開完了まで後続出力を保留し、後続Systemを一度だけ実行することを確認した。別の回帰では2/4 workerをbarrierで同時開始し、Systemごとの遅延を入れ替えながらcheckpoint一致を維持する。

- Windows `Microsoft Windows 10.0.26300`、AMD Ryzen 7 2700 (8 cores / 16 logical processors)、Rust/Cargo 1.98.1。省メモリtest profile（test debug無効、codegen-units=1024、incremental無効、build job 1）。描画なしの合成Runtime計測で、4独立Systemが固定回数の整数演算を行い、各Systemが1つの別StateStore recordを書き込む。各worker条件につきwarm-up 20tick、測定200tick。2回測定し、p50/p95/p99、経過時間、Rust heap allocationを記録した。
- 手動計測コマンド：`cargo test --locked --test runtime_worker_benchmark --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- --ignored --nocapture`。対象Runtime/Materials回帰は`cargo test --locked --test runtime --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`で**47件成功、失敗0件**。追加した`runtime_material_phase_matches_across_worker_counts_with_parallel_sibling`は同じphaseで監査付きpowder輸送と独立Systemを同時開始し、worker 1/2/4のtick、素材位置、audit、checkpointを比較する。

| 1 Systemあたりの整数演算 | worker数 | tick p50 / p95 / p99 | 200tick合計 | Rust heap割当回数/累計byte | heap live peak増分 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 8,192 | 1 | 326–328 / 331–333 / 356–359 µs | 65.5–65.6 ms | 8,603 / 3,254,400 B | 8,384 B |
| 8,192 | 2 | 685–723 / 725–821 / 754–867 µs | 137.7–146.5 ms | 14,003 / 5,552,000 B | 10,520 B |
| 8,192 | 4 | 491–493 / 532–533 / 559–583 µs | 98.7–99.3 ms | 13,003 / 5,539,200 B | 13,160–13,320 B |
| 65,536 | 1 | 2.273–2.290 / 2.336 / 2.350–2.377 ms | 446.5–455.3 ms | 8,603 / 3,254,400 B | 8,384 B |
| 65,536 | 2 | 1.676 / 1.736–1.763 / 1.814–1.902 ms | 337.0–337.8 ms | 14,003 / 5,552,000 B | 10,520 B |
| 65,536 | 4 | 0.970–0.974 / 1.027–1.033 / 1.122–1.178 ms | 196.2–196.4 ms | 13,003 / 5,539,200 B | 13,160–13,320 B |

小さい仕事ではthread起動の費用が計算量を上回り、worker 4のp50は逐次より約1.5倍遅い。大きい合成仕事ではworker 4のp50が約2.3倍短い一方、heap allocation回数・累計byte・peak liveは増える。heap peakはRust allocator経由のlive byte差でありOS thread stackを含まない。これはworker waveの費用と上限感を測るsynthetic benchmarkで、粉液気・Providerを含む代表Q04 workload、GPU描画、実ゲームFPS、全体性能合格を証明しない。性能改善や144/60 FPS達成は主張しない。

Runtime独立System間のworker 1/2/4 checkpoint・SameTick順、単一の監査付きpowder/scalar Materials処理と独立Systemの1/2/4 parity、合成負荷の比較は完了した。Pressure/Job/particleと複数force/inventory Providerを同時に含むworker profileのcheckpoint・収支・rollbackも確認した。統合profileの参照solver differential、一般Provider dependency stencil、別表現のsleep証明、Q04総合検証・独立レビューは引き続き未完了である。複合輸送・archive保存をまたぐ異なるProvider実装の相互移譲は後続試験で確認した。

単体レビューと対象Clippyで、圧力探索Materials APIの引数が過多、広域Jobのprepared enumに重複layer keyを保持、Runtime結果処理に不要なmatchがある点を修正した。開始引数を`MaterialPhasePressureSearchInput`へまとめ、enumから冗長なStableKeyを外してJob scratch計上byteを実体サイズと一致させ、結果は`?`で扱う。`scalar_sleep` optionの不要なderefも除いた。Windows/MSVC対象suite `material_motion_job` 18件、`material_runtime` 17件、`runtime` 30件は**合計65件成功、失敗0件**。対象Clippy `cargo clippy --locked --test runtime --test material_runtime --test runtime_worker_benchmark --test material_motion_job --test pressure_job --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- -D warnings`も成功した。

さらに`q04_distinct_inventory_providers_exchange_material_in_one_atomic_tick`で、異なるbody/reservoir Provider実装の間をa/b物質が同一監査取引で相互移譲すること、後段reservoir Providerの監査失敗で先行body書換えもrollbackすることを検証した。`material_accounting` suiteは**21件成功、失敗0件**。sandbox内では既知のOS error 5により世代保存1件だけが失敗したが、同一コマンドをsandbox外で再実行し全件通過した。複合輸送・粒子・archive保存をまたぐ別Provider間移譲とworker profile比較は引き続き残る。全target check、E05総合試験、独立外部レビューは未実施であり、Q04/E05は未完了。

#### Q04作業単位：異なるProviderの所有交換を混合粒子・archive経路へ接続（2026-10-18）

`q04_scalar_mixture_to_particle_then_archive_preserves_rational_provider_totals`を拡張し、body Providerが純a、別実装のreservoir Providerが純bを所有する初期状態から、監査付き取引で両recordを交換した後も、同じWorld・同じ有理量baselineで混合scalar移動→正規粒子化→region退避→World checkpoint復元→archive再構築→region復帰を行う。baselineはa=9/7、b=13/7 token、massはa=45/7、b=39/7であり、保存・復帰まで一致する。これで異なるProvider実装間の所有交換と複合archive経路を同一テストへ接続した。先行段落に記載した「複合輸送・粒子・archive保存をまたぐ別Provider間移譲」はこの変更で解消した残件である。

- `cargo test --locked --test material_accounting --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`をWindows/MSVC省メモリ設定で実行し、**21件成功、失敗0件**。拡張したQ04統合ケース単独でも成功した。
- `cargo clippy --locked --test material_accounting --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- -D warnings`と`cargo fmt --all`が成功した。
- Pressure/Job/particleを合わせたworker profile別参照比較、一般Provider dependency stencil、別表現のsleep証明、Q04総合検証・独立レビュー・E05 PRは未実施であり、Q04/E05は未完了。

#### Q04作業単位：PressureJob・押出し・移動Jobのworker profile比較（2026-10-18）

`runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers`を拡張し、worker数1/2/4のRuntimeでPressureJob→PressurePushSearch→MaterialMotionJob→標準Materials phaseと正規粒子の力積分を実行した後の完全World checkpointを比較する。独自`NoGravity` MaterialForceProviderで1cell移動する正規粒子がphase後にも保持されることを確認する。pressure Systemと独立probe Systemを同じparallel waveへ置き、worker数2/4ではbarrier同期して並行開始を確認した。SameTick budget継続とPressureJob/PressurePushSearchのStateStore再読込は既存のbarrier経路を維持する。

- 対象試験単独で成功し、`tests/material_runtime.rs`全**17件成功、失敗0件**。`cargo clippy --locked --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- -D warnings`と`cargo fmt --all`も成功した。
- 単体レビューでは、StableKey順でprobeをbarrier付きSystemの直前へ置くこと、work budgetを2にして二つの独立Systemが同一waveへ入ること、barrierをSystemの初回呼出しだけで消費することを確認した。粒子はphase後の生存数と移動先を照合する。今回の差分は検証コードと資料のみで、simulation hot pathは変更していない。
- この比較はpressure、複数Job、粒子と単一独自force Providerを含む特定の統合profileのcheckpoint parityであり、複数inventory/force Providerを加えたprofile、各tickのscalar/particle reference differential、性能比較を証明しない。Q04/E05の総合検証・独立レビュー・PRは未実施である。

#### Q04作業単位：作者MaterialForceProviderの依存stencil sleep（2026-10-18）

`MaterialForceDependencyStencil`と`MaterialForceProvider::sleep_invariant`を追加し、`World::step_material_force_motion_sleeping_with_provider`を実装した。作者Providerは候補cellごとの空間依存を各軸64 cell以内の矩形半径で宣言し、作者状態・tick・その他非空間入力に対する不変条件をsleep中も毎tick判定する。宣言がない場合、参照経路を使う。証明はWorld枝、Content/epoch、profile、active chunk revision、依存chunk集合に結びつけ、tracker decode後は未認証として参照実行から再証明する。sleep証明をWorldの正規状態と分離した。

- `author_provider_sleep_wakes_only_inside_declared_dependency_and_revalidates_restore`は通常参照実装とのforce/motion reportおよび完全World checkpointを比較する。初回参照後の継続sleep、tracker保存復元後の参照実行、依存範囲外chunk変更でのsleep継続、依存chunk変更と作者StateStore値変更でのwakeを確認する。依存内のpressure値変更後、砂が同じ候補tickで移動することも検査する。`provider_without_sleep_contract_always_uses_reference_execution`は、stencilを宣言しないProviderが2tickとも通常参照経路を使うことを照合する。
- 実装後レビューで、decode証明を未認証にすること、World枝/Content/epoch/profile/active集合/依存変更の照合、stencil scratchの最大9依存chunk/active chunk上限、作者状態不変条件の毎sleep tick呼出し、stencil未宣言時の参照fallbackを確認した。レビュー時に不足していた未宣言Providerのfallback回帰を追加した。
- Windows/MSVC、単一build job、incremental無効、test debug無効、codegen-units=1024、`e05-runtime-worker-build`で`cargo test --locked --test material_forces --jobs 1`を実行し、**31件成功、失敗0件**。`cargo clippy --locked --test material_forces --jobs 1 -- -D warnings`も成功した。Clippyはsandbox内で設定ファイルへのOS error 5となったため、同じ限定コマンドをsandbox外で再実行した。追加テスト時にはsandbox内でrustcのメモリ確保失敗も一度発生したが、権限付き再実行では同じ省メモリprofileでsuiteが通過した。
- 関連高頻度経路の合成比較として、`tests/material_provider_sleep_benchmark.rs`にignoredの手動計測を追加した。Windows/MSVC・Rust 1.98.1、AMD Ryzen 7 2700（8 core / 16 logical）、単一active chunkに静止solid候補4,096個、no-op作者Provider、20tick warmup・200tick測定で、毎tickのforce+motionと`advance_tick`を計る。参照はtotal 6.197 s / p50 30.800 ms / p95 33.699 ms / p99 34.331 ms、sleepはtotal 166.367 ms / p50 0.829 ms / p95 0.864 ms / p99 0.896 ms。初回一回の測定では参照33.596 msに対しproof作成付きsleepは44.596 ms（proofによる追加約11.0 ms）。Rust heap allocationは200tick区間で参照4,109,400回・25,868,883,200 byte・peak live delta 64,475,112 byte、sleep8,600回・25,724,612,800 byte・peak 64,311,352 byteだった。これは単一chunkの静止合成fixtureであり、動的材料、複数chunk、Runtime/Provider profile、描画FPSを代表しない。手動計測testはClippy指摘を修正し、対象Clippyが成功した。
- API docsを`new_docs/37_MATERIAL_FORCES.md`と`39_MATERIAL_SLEEP.md`へ反映し、Q04の残件・README状態も更新した。依存宣言の正しさは作者Providerの契約に依存する。proof作成単独の費用、動的材料・複数active chunk・参照solverとの各tick differential、圧力・正規粒子など別表現のsleep、Q04総合試験・独立レビュー・E05 PRは未実施である。Materials Runtime内の複数force/inventory Provider profileは後続節で追加した。合成benchmarkの結果を他の実行profileや全体FPSへ一般化せず、Q04/E05は未完了とする。

#### Q04作業単位：複数force/inventory Providerを含むRuntime worker profile（2026-10-18）

同じRuntime統合fixtureへ素材別force Providerと二つのinventory Providerを加えた。水用`NoGravity`と砂用`SandDrift`を`RuntimeForceProviders`で振り分ける。body用とreservoir用inventory Providerはそれぞれ異なるWorld StateStore recordから固有material tokenを一つずつ集計する。PressureJob→PressurePushSearch→MaterialMotionJob→cell force+DDA→正規粒子の処理をRuntimeの監査付き取引に接続し、各Provider所有量を含む全表現収支を同じWorldで照合する。押出し予約外の孤立水cellを使いforce callbackが両方呼ばれること、inventory結果に両unique tokenが含まれること、指定砂の1cell移動とworker数1/2/4の完全World checkpoint一致を確認する。別実行ではphase完了後にProvider所有recordを不正値へ変更し、最終inventory scanを失敗させる。Runtime tickがエラーになり、Pressure・押出し・移動・Materials・Provider状態を含むcheckpointが開始時点と完全一致することを検査する。

- Windows/MSVC、省メモリ設定（build job 1、test debug無効、codegen-units=1024、incremental無効）で、`cargo test --locked --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`を実行し**17件成功、失敗0件**。変更後の整形確認も通過した。
- 対象Clippy `cargo clippy --locked --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- -D warnings`は、今回の再実行ではsandbox内で成功した。
- 単体レビューではforce素材StableKeyの振分け、未知素材のmotion保持、両force callbackの実呼出しcounter、押出し予約外cellの分離を確認した。inventory Providerは異なるProvider StableKey・record keyを使い、型付きu64 state recordから固有tokenを集計する。固有tokenがprofile集計に含まれること、Runtime callbackが`reason=None`で保全を要求すること、最終Worldでledgerを再検証することを確認した。
- 参照solverとの各tick比較、別表現のsleep、Q04総合試験・独立レビュー、全target checkは未実施。E05/Q04は未完了。

#### Q04作業単位：同期圧力Jacobiの固定点sleep（2026-10-19）

`PressureRelaxation`の参照solverが最終pressure fieldを入力と同じ値へ戻した場合のみ、`PressureSleepTracker`へsleep証明を記録する`World::relax_pressure_sleeping`を追加した。World枝世代・Content fingerprint・remap epoch・layerと全PressureRelaxation設定・活動chunk集合/各revisionを照合する。pressure channelを含むchunk変更はwakeし、StateCodec復元後の証明は未認証として必ず一度参照solverへ戻す。成功sleepでは空のWorld取引を実行し、参照経路とWorld revisionを揃える。参照solverも出力が現在値と同じchannelを再書込せず、不要なchunk revision更新と容量予約を避ける。

- `tests/material_transport.rs`の圧力sleep2件：固定点tickのreport/完全checkpoint一致、channel編集直後のwake、fieldが再固定するまでの各tick一致、設定変更wake、tracker/World保存復元後の参照fallbackを確認した。前回suite実行後に追加した設定変更wake検査は、`cargo test --locked --test material_transport pressure_sleep --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`で**2件成功、失敗0件**。全29件suiteはこの追加変更後に未再実行。
- `cargo clippy --locked --test material_transport --test material_provider_sleep_benchmark --jobs 1 -- -D warnings`はsandbox内のClippy config OS error 5後、sandbox外の限定再実行で成功。`cargo fmt --all`も成功。
- ignored手動benchmark `pressure_sleep_comparison`：Windows/MSVC・Rust 1.98.1・AMD Ryzen 7 2700（16論理CPU）、静止圧力field 4,096 cell・単一active chunk・gravity 0・Jacobi 4回、20tick warmup後の200tick比較。reference合計6.046秒、p50/p95/p99=30.129/31.021/33.370 ms、sleep合計0.168秒、0.838/0.874/0.895 ms。200tickの割当はreference 1,650,800回・25,889,476,800 byte・peak live delta 64,872,944 byte、sleep 9,000回・25,724,836,800 byte・64,311,912 byte。初回証明付きsleep 29.597 ms、reference 29.588 ms。固定点の単一chunk合成条件であり、dynamic pressure、複数chunk、RuntimeやFPS性能へ一般化しない。
- `33_WIDE_PRESSURE.md`、`39_MATERIAL_SLEEP.md`、`42_Q04_CROSS_REPRESENTATION.md`とREADMEを更新した。同期pressure sleepは今回の作業単位で対象検証済みだが、統合profileとの各tick reference differential、正規粒子sleep、Q04総合試験・独立レビュー・E05 PR・全target checkは未実施で、E05/Q04は未完了。

#### Q04作業単位：Materials phaseの4tick参照比較（2026-10-19）

既存の`integrated_material_phase_pressure_and_scalar_match_reference_same_tick`を複数tick試験へ拡張した。独立solverで構成したpressure→scalar→正規particle経路と`step_material_phase`を同じ初期checkpointから4tick連続実行し、各tickのreportと完全World checkpointを比較する。双方で`advance_tick`を行い、tick境界を含めて検証する。

- `cargo test --locked --test material_forces integrated_material_phase_pressure_and_scalar_match_reference_each_tick --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**1件成功、失敗0件**。
- `runtime_material_phase_matches_across_worker_counts_with_parallel_sibling`を3tickへ拡張し、監査付きMaterials処理のtickごとのcheckpointをworker 1/2/4で比較した。対象1件成功、失敗0件。
- 変更後の`cargo test --locked --test material_forces --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**48件成功、失敗0件**（force 31、Runtime 17）。
- `cargo fmt --all`と対象Clippy（`material_transport`、`material_provider_sleep_benchmark`）は成功。差分は既存試験の拡張・名称変更と検証記録の更新で、production hot pathは変更していない。
- この回帰は単一Worldの統合phaseと個別参照solverの逐tick一致、および監査付きRuntime Materials処理のworker数別逐tick一致を検証する。複数force/inventory Provider、PressureJob/PushSearch/MaterialMotionJobまで同時に含む複合Runtime profileの参照一致、正規粒子sleep、Q04総合試験・独立レビュー・E05全体検査は残る。Q04/E05は未完了。

#### Q04作業単位：正規粒子force/DDAの静止sleep（2026-10-19）

`MaterialParticleSleepTracker`と`World::step_material_particle_force_motion_sleeping`を追加した。標準Providerはgravityが0のprofileにだけ参加し、作者Providerは`particle_sleep_invariant`で継続静止を保証できる。初回参照stepの候補・force/DDA結果、対象layer、設定、Provider/version、World枝、Content/remap epoch、全active chunk revisionが一致する場合に後続tickのforce callbackとDDAを省略する。chunk editまたはparticle velocity editはwakeし、tracker decode後は証明を未認証として参照経路へ戻す。sleep中も空のWorld取引でtick/revisionの進みを維持する。

参照比較中に、無変更粒子tickが粒子状態を変えていないにもかかわらずchunk revisionを更新し、sleep経路との差を作っていることが判明した。`step_material_particles`は粒子位置/速度に変化があるときだけ所属chunk revisionを更新するよう修正した。予約・上限検査は維持する。この高頻度経路差分の性能影響は未計測であり、速度改善は主張しない。

- `sleeping_static_particles_match_reference_each_tick_and_wake_after_velocity_edit`は地形に接して移動しないparticleを3tick参照stepと比較し、各tickのreport/checkpoint一致、velocity編集によるwake、tracker codec復元直後の参照fallbackを確認する。`cargo test --locked --test material_forces --test material_particles --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`は**62件成功、失敗0件**（force 32、particle 30）。同じ二suiteへの`cargo clippy --locked --test material_forces --test material_particles --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- -D warnings`と`cargo fmt --all`も成功。
- `material_particle_sleep_comparison`はWindows/MSVC・Rust 1.98.1・AMD Ryzen 7 2700（8 core/16 logical）、単一active chunk内の静止正規粒子4,096個、unoptimized `cargo test` profile、20tick warmup・200tick計測で参照solverとsleepを比較した。200tick合計は参照10.591秒/sleep 5.027秒、p50/p95/p99は参照52.787/54.087/54.587 ms、sleep 24.817/27.919/29.769 ms。Rust heap割当は参照4,239,400回・26,233,105,600 byte・peak live delta 65,524,208 byte、sleep 3,286,000回・26,131,163,200 byte・65,327,736 byte。初回は参照40.940 ms、proof作成付きsleep 41.738 msだった。この測定は粒子静止・単一chunkの相対比較であり、release profile、動的粒子、複数chunk、Runtime合成負荷、FPS目標を証明しない。
- `cargo test --locked --test material_provider_sleep_benchmark material_particle_sleep_comparison --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build -- --ignored --nocapture`：**1件成功、失敗0件**。粒子編集がないtickでchunk revisionを変えない修正後も、`material_forces`と`material_particles`の62件suite、対象Clippyは成功した。Rustdocの`cargo doc --lib --no-deps --locked --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`も成功。
- 粒子sleepのMaterials phase/Runtime統合、複数profileの性能比較、Q04全体レビューは未実施。これらとE05総合検証が残るため、Q04/E05は未完了。

#### Q04作業単位：正規粒子sleepのMaterials phase/Runtime接続（2026-10-19）

`step_material_phase_with_particle_sleep`を使うRuntime Materials Systemの継続を確認する試験を追加した。RuntimeはSystemの`run`からcommandをbarrierへ提出し、WorldEdit callbackは後で実行されるため、callback内で更新したtrackerを同じ`run`の戻り前に読むことはできない。試験Systemはtrackerを共有cacheへ置き、次tick開始時に取得する。World StateStoreにもtrackerを保存し、checkpoint復元、物質台帳、正規粒子の存在を各tickで検証する。初回tickは参照実行し、続く2tickはsleep経路へ入り、3tickすべてのRuntime tickを完了する。

- `runtime_particle_sleep_phase_persists_tracker_and_skips_later_force_steps`：3tickのtracker継続、sleep診断、1粒子の所有維持、監査台帳一致、各tick checkpointのrestore roundtripを検査。**1件成功、失敗0件**。
- 変更後に`cargo test --locked --test material_runtime --test material_forces --test material_particles --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`を再実行し、**80件成功、失敗0件**（Runtime 18、force 32、particle 30）。
- 同範囲の`cargo fmt --all -- --check`、Clippy（`-D warnings`）、`cargo doc --lib --no-deps --locked --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`が成功した。Clippyで見つかった不要なOption dereferenceとphase内部引数数は修正し、対象suiteを再検証した。
- Runtime統合済みだが、複合Pressure/Job/複数force/inventory Provider profileの個別参照solverに対する逐tick differential、Q04総合試験・独立レビュー・E05全体確認は引き続き残る。Q04/E05は未完了。
#### Q04作業単位：複合Runtime profileと個別参照経路の照合（2026-10-19）

`runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers`に、Runtime実行前のcheckpointから復元した参照Worldを追加した。参照側はPressureJobをbudget=1で直接完了し、圧力公開後のPressurePushSearchで押出しを確定し、その後にMaterialMotionJobを生成して共有予約付きMaterials phaseへ渡す。Runtime側のbudget=1、複数barrier、複数force/inventory Provider、正規粒子を含む結果と、tick・64×64の全cell/pressure/motion値・粒子位置・Provider所有record・inventory量を3tick連続で照合する。worker 1/2/4すべてで同じ比較を行い、既存の最終checkpoint worker parityも維持する。

当初は参照側でMaterialMotionJobを単独公開してからforce/DDA phaseを呼び、同じcarrierが同tickに二度移動する比較不一致を検出した。これはjob予約を通常phaseへ渡していない組み立て上の差だったため、参照経路を`step_material_phase_with_jobs`へ変更し、移動Jobと後段solverが同一予約を共有する正しい契約に合わせた。

- 2026-10-19、Windows/MSVC Rust 1.98.1で`cargo test --locked --test material_runtime --test material_forces --test material_particles --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**80件成功、失敗0件**（Runtime 18、force 32、particle 30）。3tick参照比較とRuntime particle sleep接続を含む現行suite。
- 同じ3suiteの対象Clippy（`-D warnings`）と`cargo fmt --all -- --check`が成功。関連の`cargo doc --lib --no-deps --locked --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`も既存実行で成功。
- これは複合profileの3 Runtime tickに対する個別参照経路との広域状態比較と、worker 1/2/4 parityを示す。archive IO完了順を含む標準scalar Runtime経路の比較は次の作業記録で検証する。Q04全体の失敗相互作用レビュー、E05全体検証・レビューは残るため、Q04/E05は未完了。

#### Q04作業単位：archive IO順序後のRuntime参照比較（2026-10-19）

既存のarchive worker試験をRuntime経路まで延長した。二つのregionを独立workerから異なる遅延で読み、逆転したcompletion順でReady結果を集めて一つのWorldEditから原子的に復帰する。復帰済みWorldをRuntimeへ登録し、標準scalar輸送Systemを4tick実行する。各tickでRuntimeの完全checkpoint hashをWorldEdit参照経路と比較し、さらにIO完了順を逆にしたRuntime同士の完全checkpointも比較する。参照側はRuntimeと同じく材料処理とtick進行を別のWorld取引で確定する。

- `cargo test --locked --test material_transport --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**29件成功、失敗0件**。新規回帰`archive_io_completion_order_preserves_scalar_transport_each_tick`を含む。
- 同suiteへの対象Clippy（`-D warnings`）と`cargo fmt --all`が成功。
- これはarchive IO完了順を含む標準scalar Runtime経路の4tick参照同値性を示す。Pressure/Job/複数Provider profileの3tick参照同値性は別の作業記録にある。Q04の全出口照合・独立レビューとE05全体検証は引き続き必要であり、Q04/E05は未完了。

#### Q04作業単位：複合Runtime profileの粒子pool容量失敗（2026-10-19）

既存の複合Runtime profileへ、粒子pool上限を既存粒子数の1に制限する失敗条件を追加した。PressureJob、押出し探索、MaterialMotionJob、標準Materials phaseとProvider積分が完了した後、同一取引の後段で追加粒子放出を試す。pool容量エラーを受けてRuntime tickが失敗し、開始時World checkpoint、PressureJob/Search StateStore、material ownership、Runtime tickが全てrollbackすることを確認する。通常worker 1/2/4の参照差分と、既存のinventory Provider fault injectionも同じ回帰に維持した。

- `cargo test --locked --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**18件成功、失敗0件**。`runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers`は容量失敗、Provider fault、正常profileを含む。
- 4つのQ04関連suite（`material_runtime`、`material_transport`、`material_forces`、`material_particles`）を一回のコマンドで統合実行し、**109件成功、失敗0件**。同じ4suiteへのClippy（`-D warnings`）と`cargo fmt --all -- --check`も成功。
- この追加は粒子pool上限をPressure/Job/Runtime取引へ接続した失敗rollbackを証明する。Q04の全出口照合・総合レビューとE05全体検証は引き続き必要である。加えて[35](../../35_MATERIAL_PARTICLES.md)に記載した保存可能な件数分割粒子Jobは未実装・未検証のE05残件であるため、Q04/E05は未完了。

#### Q04作業単位：正規粒子の保存可能な件数分割Job（2026-10-19）

`MaterialParticleJob`を追加した。作成時に活動中のsimulated layerの候補粒子、全resident粒子のoccupancy、active chunk revisionを記録する。`step(world, budget)`は候補budget単位で標準整数DDAとphase予約を計算し、全候補が完了するまでWorldを書き換えない。StateCodec復元を挟んだ後もslot順・予約・診断が継続し、Ready時の`publish`だけが全移動と停止粒子の着地を一つのWorldEditで公開する。候補particle状態、occupancy、World/Content/remap、active chunk revisionのいずれかが失効した場合は公開を拒否する。Runtime Systemの例ではWorld StateStoreへJobを保存し、barrierを越えて同tick再開し、完了時にprogress recordを同じ取引で削除する。

- `saved_particle_job_resumes_by_candidate_count_and_matches_reference_atomically`は、処理途中のStateCodec往復後も通常particle solverとreport・完全checkpointが一致すること、未完了時にWorldを変更しないことを検査する。
- `saved_particle_job_rejects_stale_particle_inputs_without_partial_publish`は速度変更後の失効とWorld checkpoint不変を検査する。
- `saved_particle_job_checks_candidate_step_and_byte_budgets`は候補数、累積DDA境界、初期job byte上限でcapacity errorとなること、候補の部分計画を保持せずWorldも不変であることを検査する。
- `saved_particle_job_codec_rejects_every_truncation`はJob codecの全byte prefixがdecode時に拒否されることを検査する。
- `runtime_resumes_saved_particle_job_from_world_state_by_candidate_budget`はRuntime StateStore/barrier間のJob復元、pending中のtick・粒子不変、Ready後の一度だけの公開を検査する。
- barrierごとのStateStore encodeで全plansを再走査していたため、保存時はprivateな進捗状態のO(1)検査に限定し、全要素の整合検査はdecode後と最終publish時に行うよう整理した。これにより保存のたびの追加構造検査が累積O(N²)になる経路を除いた。StateStoreがJob全recordをencodeする費用自体は残る。当時は複合Runtime性能が未計測だったが、後続の2026-10-19作業単位で代表profileを測定した。極小budgetでの全record codec費用単独は未測定である。
- 再確認：`cargo test --locked --test material_particles --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`：**54件成功、失敗0件**（粒子35、Runtime19）。対象2suiteのClippy（`-D warnings`）、`cargo fmt --all -- --check`、`cargo doc --lib --no-deps --locked --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`、`git diff --check`も成功した。差分検査にはWindowsのLF→CRLF警告が出たが、空白エラーはない。
- 性能目標の計測は未実施。Job開始とpublishの全粒子snapshot走査、極小budgetでStateStoreへJob全体を繰り返し保存する累積codec費用、複合Runtime profileへの性能影響をQ04出口レビューで評価する。これはparticle-only予約を持つ独立Jobであり、統合Materials phaseのcell reservationとの結合は未提供である。

この作業単位で粒子Jobの実装・対象検証を追加したが、Q04の全出口条件照合・独立レビュー、およびE05全体の検証・レビューは未完了である。

#### Q04出口前の横断回帰と単体レビュー（2026-10-19）

設計書42/43とE05の横断契約を照合し、accounting、force/provider、inventory、mixture、広域Job、particle、Runtime、scalar sleep、transport/pressure、Worldの12 integration suiteをまとめて実行した。合計**296件成功、失敗0件**。省メモリ設定は`CARGO_BUILD_JOBS=1`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_PROFILE_TEST_CODEGEN_UNITS=1024`、`CARGO_INCREMENTAL=0`、`--jobs 1`。同じ12 targetへのClippy（`-D warnings`）、`cargo fmt --all -- --check`、`git diff --check`も成功した。all-target検査はE05全Q完了後に行う。

横断回帰の初回実行では`pressure_job::legacy_profile_matches_scalar_for_every_budget_and_preserves_terrain`がbudget=1で失敗した。pressure値とWorld revisionは参照solverと一致していたが、PressureJobが値の変わらないchannelも書き戻すためchunk revisionが一つ余分に進んでいた。`prepare_pressure_jobs`で現在値と異なる出力だけを公開対象にし、重複出力の競合検査は従来どおり全対象座標に対して行うよう修正した。failure診断も巨大checkpoint配列全体ではなく最初の異なるbyte位置とpressure値・revisionを示すよう改善した。対象回帰の単独再実行と12 suite統合再実行はともに成功した。

統合Materials phaseの費用を`integrated_material_phase_comparison`で手動比較した。Windows/MSVC、Rust 1.98.1、AMD Ryzen 7 2700（8 core/16 logical）、unoptimized test profileで、単一active chunk内の液体4,096 cell、pressure Jacobi 4回（gravity 0）とscalar輸送を、個別solver参照経路と`step_material_phase`経路で比較した。両方20tick warmup後の200tick計測とcheckpoint完全一致を確認した。参照は合計8.118秒、p50/p95/p99=40.509/43.779/46.539 ms、統合は8.023秒、39.856/43.114/43.944 ms。heap割当は双方1,649,000回・13,040,275,200 byte・peak live delta 64,872,944 byte。近い結果だが、この静止単一chunk・WorldEdit workloadだけでは差を性能改善と判定しない。force、particle、広域Job、Runtime barrier、動的multi-chunkと描画はこのWorldEdit比較の計測対象外である。複合Runtime profileは後続の2026-10-19作業単位で別計測した。

MaterialParticleJobの単体レビューでは、slot順snapshot、resident occupancy、active chunk revision、整数DDAと角guard、候補・DDA・byte予算、失効時の拒否、Ready後の一括WorldEdit、保存復元の構造検査を確認した。barrierごとのencode前に全plansを再走査する経路はO(1)進捗検査へ変更し、decode・publish時の全体検査は維持した。追加codec回帰は全byte prefixのdecode拒否を確認する。particle-only Jobは統合Materials予約との結合を提供せず、Ready時の一括編集は`edit_cells`制限を受ける。このレビュー時点ではRuntimeで極小budgetを繰り返す全record codec費用と複合Runtime profileの性能は未計測だった。後続の限定複合profileを測定したが、全record codec費用単独の測定は残る。

この横断実行と単体レビューは、Q04のテスト証拠と既知の制約を整理するものである。Q04/E05の出口判定、E05全体テスト・総合レビュー、GitHub上の独立レビュー/CIは未完了であり、全条件の証拠監査後までE05を完了扱いにしない。

#### Q04作業単位：複合Runtime性能計測とArena候補複製の縮小（2026-10-19）

`runtime_resumes_pressure_job_search_phase_from_world_state_at_barriers`が実行するPressureJob→PressurePushSearch→MaterialMotionJob→MaterialPhase、素材別force Provider、複数inventory Provider、正規粒子を含むRuntime profileに、tick時間・割当回数/要求byte・peak live増分の手動計測を加えた。Windows/MSVC、Rust 1.98.1、AMD Ryzen 7 2700（8 core/16 logical）、64×64 World、work budget=2。worker 1/2/4ごとに同profileを新規processで20回計測し、初回processをwarmupとして除外したため、各worker・各profileで60tick標本を得た。allocator計測hookを含む値であり、描画・複数chunk負荷・dynamic workloadを含まない。

計測中、各tickの累積heap割当要求が約1.999GB、peak live増分が約129.0MB、最大単一割当が30MiBと判明した。`Arena::try_copy`が論理上限までslots/free-listを予約し直していたため、`World::try_copy`のたびに最大262,144件のMaterialParticle slot領域（約30MiB）を確保していた。候補Arenaの物理容量を保持中slot/free-list数に合わせて複製し、候補内insert/removeが必要量をfallible reserveするよう修正した。論理capacity、世代、free-list順序、失敗時の非公開契約は保つ。

同一のunoptimized test profileを修正前後で各worker 60tick比較した。p50/p95/p99（ms）は、worker 1が32.555/34.029/40.191から30.218/31.158/32.661、worker 2が32.963/36.300/38.686から30.445/32.354/38.487、worker 4が32.756/35.654/36.390から28.271/31.465/34.237へ変化した。tickあたり累積割当要求はworker 1で約1.999GBから5.204〜6.015MB、worker 2/4で約1.999GBから5.208〜6.019MB、peak live増分は約129.0MBから最大約0.40MBになった。修正前に繰り返された30MiB割当は修正後に発生しない。比較対象の他のコード・World fixture・計測hookは一致しており、この特定Runtime profileでの改善として記録する。

最適化release test profileでも同じfixtureをworker 1/2/4各60tick計測した。p50/p95/p99はworker 1で4.087/4.352/4.537ms、worker 2で4.514/4.967/5.083ms、worker 4で2.353/4.533/5.481ms。tickあたり割当要求は5.204〜6.019MB、peak live増分は最大397,253 byte。これはPressure/Job/複数Provider/particleを含む当該小規模Runtime workloadのみで、全World規模、全E05 solver、144 FPS表示、全ハードウェアでの性能達成を示さない。

変更後に`cargo test --locked --test foundation --test material_particles --test material_runtime --jobs 1 --target-dir A:/Project/charon-engine/target/e05-runtime-worker-build`を実行し、**74件成功、失敗0件**（foundation 20、particle 35、Runtime 19）。同じ3suiteへのClippy（`-D warnings`）、`cargo fmt --all -- --check`、`git diff --check`も成功した。新しい`arena_candidate_copy_preserves_logical_capacity_with_live_slots_only`は複製後の世代再利用、元候補の独立性、論理上限までの追加と満杯拒否を検査する。

この計測で複合Runtime性能は対象profileについて評価済みとなった。Q04全受入条件の証拠監査と独立レビュー、E05出口の総合検証・PRは引き続き残る。極小budgetでStateStoreが粒子Job全recordを繰り返しcodecする費用、他の動的/複数chunk profile、粒子Jobとcell予約を同tick統合する経路は別の残件であり、今回の計測から合格・改善を推論しない。

#### E05全体の出口前検証と総合レビュー（2026-10-19）

Q04の受け入れ条件を`new_docs/18_VALIDATION_AND_RELEASE.md`、`42_Q04_CROSS_REPRESENTATION.md`、`43_MATERIAL_PHASE_PIPELINE.md`の記述と照合した。質量交換、粉体・液体・気体、圧力押出し、混合、sidecar追従、全表現の保存・復元、参照経路とのtick単位一致、worker/生成/IO完了順、失敗時の原子rollbackについて、対応する実装ページ・integration test・上記作業記録を特定した。E06/Q05の熱・反応は本工程へ含めていない。

変更したArena候補複製を単体レビューした。複製は使用slot/free-listだけをfallible reserveし、論理capacity・slot世代・退役状態・free-list順序を保持する。候補insertはslot追加前、removeは値を取り出す前に必要領域をfallible reserveし、確保失敗で論理状態を変えない。境界テストとE05全体テストでこの経路を確認した。Q04統合では各受け入れ項目の対応試験と失敗境界を照合し、E06機能を混在させないことを確認した。性能証拠は限定した比較profileの範囲に限り、動的multi-chunk、全World規模、描画やFPSの主張へ拡張しない。

監査時に`MaterialParticleJob`の説明と実APIの差を修正した。通常のMaterialPhaseにおける粒子DDAはcell/scalarと共有予約を使う一方、保存可能な粒子Jobを同じ共有予約へ合成するAPIはない。Q04設計出口にあるPressureJob/MaterialMotionJob/PressurePushSearchと通常Materials phaseの連携・参照一致は対象試験で確認済みであり、粒子Jobとの同tick合成はその受け入れを満たすものとして扱わない。`35_MATERIAL_PARTICLES.md`に現状の境界と誤用禁止を記載した。将来その組合せを要求する場合は別の統合API実装が必要であり、性能・正しさを今回の証拠から主張しない。

Windows/MSVCでE05全体の最終ローカルゲートを実行した。`cargo test --locked --jobs 1`は成功し、通常のunit/integration/doc testに失敗はない（手動benchmark testは通常実行から除外される）。`cargo check --all-targets --locked --jobs 1`、`cargo clippy --all-targets --locked --jobs 1 -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check`も成功した。Cargoには`CARGO_BUILD_JOBS=1`、`CARGO_PROFILE_TEST_DEBUG=0`、`CARGO_PROFILE_TEST_CODEGEN_UNITS=1024`、`CARGO_INCREMENTAL=0`を設定した。GitHubの最終headに対するCIとPR上のレビューは、更新push後に別途確認する。

#### PR #22 独立レビュー指摘への対応（2026-10-19）

PR reviewは二つのP1欠陥を指摘した。PressurePushSearchが同layer正規粒子のいるAIR cellを経路・出口に選べる点と、MaterialMotionJobの広域DDAが同じ粒子占有を無視する点である。どちらもcell tokenとparticle tokenが同じcellを所有し得るため、指摘を妥当と判定して修正した。圧力探索では開始時の粒子索引とnode再検査を追加し、広域DDAではsource、各交差先、斜め角検査点を粒子占有に含め、保存経路と公開入口でも再確認する。MaterialMotionJob全20件（粒子経路・斜めguard回帰2件を含む）、PressureJob全28件、PressurePushSearch単体5件が成功した。Clippyは対象2 suiteと`--lib`で`-D warnings`成功し、`cargo fmt --all -- --check`と`git diff --check`も成功した。

粒子索引を持つJobの今回変更について関連benchmarkは未実施である。広域DDA索引は対象layerの活動粒子P件に対して構築・公開前再確認がO(P log P)、交差ごとの照合がO(log P)、保持領域がJobごとにO(P)となる。PressurePushSearchの粒子索引・codec追加も別の高頻度経路比較は未実施であり、性能改善は主張しない。

commit `d8582af45a5216eb55d40a01f0826d44639da5e4`をPR #22へpushした。GitHubのRust CI、E03、E04、E05 workflowは全てsuccess。MaterialMotionJobのinline P1 threadは修正・対象検査後にresolveし、PressurePushSearchのP1 review commentには同じcommitと試験結果で返信した。PR #22はopen・mergeable、head `d8582af`、base `codex/e05-material-particles` / `5cfb0cf465e051be179c20f9817b268ca6d85214`のままで、再レビュー結果を待つ。PR本文には採用順と除外PRを更新した。再レビューの問題なし確認と最新headの最終ゲートを得るまではE05完了と判定しない。

再レビューは別のP2を検出した。`MaterialParticleJob`の構造検査はplanのposition/cellと両端の予約を確認するだけで、保存されたplanが元velocityのDDA・壁・占有・先行予約と一致するかを公開前に再構築しなかった。二つのparticleを使った再現では、壁の向こうの先行予約cellへplanを改ざんしても復元・publishできた。公開境界で現Worldから候補をslot順に再計画し、plan、DDA総数、経路予約集合を比較するよう修正した。違いがあればWorldEdit候補を変更する前に拒否する。回帰試験は修正前に不正公開を再現して失敗し、修正後は拒否・checkpoint不変を確認した。影響するmaterial_particles 36件、material_runtime 19件、対象Clippy、fmt、diff checkが成功した。再計画は保存済み累積`step_limit`と経路scratchの`job_cells`/`job_bytes`に制限する。今回の公開前再計算専用benchmarkは未実施。PR #22上のP2 thread返信・resolve、修正commit、CI、再レビューは残る。

#### PR #22再レビュー：保存済みPressurePushSearch粒子索引の照合（2026-10-19）

`62affac`を対象にした独立再レビューは、`CHPUSH03`として構造的に有効な保存recordでも、`occupied_particles`から実粒子座標だけを除去できることを指摘した。保存済み入力revisionをWorldに一致させれば、復元後の経路・出口検証が偽の索引を信頼し、粒子が存在するAIR cellへ流体を公開できる。単体回帰は空の索引を保存・復元し、粒子のいる出口をReadyにできることを修正前に確認した。修正後は公開境界で現在のWorldから同layer・活動粒子索引を再構築し、保存値との差を拒否するとともに、再構築した索引で全経路を検査する。拒否はWorldEdit候補変更前に起き、完全checkpoint不変を確認した。

`pressure_push::tests`は6件、`tests/pressure_job.rs`は28件成功。`cargo clippy --locked --lib --test pressure_job -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check`も成功した。追加の粒子索引再構築は公開境界一回あたりO(P log P)、node検査はO(N log P)。stepごとのparticle arena全走査は行わない。この追加公開検証の専用benchmarkは未実施で、性能改善は主張しない。

#### E05出口のローカル最終確認（2026-10-19）

ユーザーの工程方針変更に従い、GitHub上の自動PRレビュー待ちを完了条件から外し、`87edd16`のE05差分をローカルで再レビューした。PressurePushSearchの公開経路が`prepared_publish_path`を通り、`validate_inputs`で保存索引と現World索引を照合してから地形・粒子占有を全node検証することを確認した。粒子arena走査は公開境界一回に限定し、stepホットパスには追加していない。改ざんrecord回帰、checkpoint不変、関連する旧P1/P2修正後の対象テストに未解決のローカル指摘はない。

対象単体6件、`pressure_job`結合28件、対象Clippy、fmt、diff checkは成功。GitHub Actionsの最新head `87edd16`はRust CIとE03/E04/E05 matrixの全10 check runがsuccess。E05/Q01〜Q04の実装・ローカル出口検査・総合レビューは完了と判定する。PR #22はopen・未マージであり、mainへの取り込みを意味しない。GitHub上で起動済みの自動review結果はこの判定の証拠として使用しない。
