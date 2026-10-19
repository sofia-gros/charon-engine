# E05：停止証明に基づく標準物性のDirty/Sleep/Wake

## 正規状態とsleep証明を混同しない

MaterialSleepTracker はWorldの正規保存状態ではなく、標準の力積分と速度DDAが次tickにも完全に恒等になることを実際の全候補から証明した派生cacheである。非空・非blockの全active素材についてMaterialMotionの速度と端数が0、物性の重力倍率・浮力倍率が0（または入力重力が0）、直前の標準force/DDAの候補数と一致し移動・衝突・待機が一件もないことを必要条件とする。

Worldの変化は公開revisionとactive chunkごとのrevision、Content fingerprint、material remap epoch、layer、motion channel、および物性とDDAの設定一式へ紐づける。通常のWorld編集、速度channel変更、chunkの生成・退避・復帰、leaseによる活動集合変更では証明を失効させ、参照実装に戻る。sleep中でも空のWorld取引を一回実行してrevisionを進め、通常実行と保存データを同一にする。

独立レビューで**同WorldId・同World revision・同chunk revisionの別枝World**を作れる問題が見つかった。異なる枝でそれぞれ同回数の編集を実行すると、revisionの数値だけでは正規内容の相違を検知できない。これを防ぐため、Worldが保持する保存対象外のArc世代を睡眠証明へ記録する。World::new / restoreは別世代、try_copyは世代を共有し、実際に地形・活動集合の変更を含むcommitだけ新世代を割り当てる。単なるadvance_tickは派生cache世代を変えない。この参照一致により、cloneした認証済みsleep証明を別の分岐へ渡しても必ずwakeする。Arc世代は保存形式へ入らず、decode後は従来通り再証明する。

## 対象を限定する理由

標準力学以外の作者Providerは、前tickのcell状態が変わらなくてもWorld内の保存済み作者状態やtickに応じて次tickへ速度を発行できる。標準専用入口`World::step_standard_material_force_motion_sleeping`は従来通り具体的な`StandardMaterialForces`へ限定する。別のopt-in入口`World::step_material_force_motion_sleeping_with_provider`では、作者が空間依存を`MaterialForceDependencyStencil`で宣言し、`MaterialForceProvider::sleep_invariant`で作者状態・tick・その他の非空間入力を毎sleep tick検査する場合に限り、そのProviderについて独立した証明を作る。どちらも直前のforce/DDAで待機や移動があった候補はsleepしない。作者Providerは宣言した矩形内に全てのcell/channel/隣接素材依存を含める責任があり、宣言を守れないProviderや証明を返さないProviderは通常の参照経路を使う。

MaterialSleepTracker はCHSLEEP1で明示的に保存できるが、外部から復元した証明はそのWorldの正規cell/channelをまだ照合していない。decode後には必ず参照実装を一度通し、新しい証明を作成する。未検証のcodec値で正規simulationを省略しない。

## 現段階の最適化範囲

標準Provider向け入口は**全active chunk集合のforce + 速度DDAに限った静止skip**である。最初の証明時には全非block候補の検査が必要だが、再利用できる間はactive chunk revisionと証明条件を確認して全cell走査とforce callbackを省略する。作者Provider向け入口は宣言stencil周辺の変更と作者不変条件を確認し、これらが保たれるtickだけforce callbackとDDAを省略する。いずれの入口も参照実装との結果一致が必要で、標準のMaterials phase全体や正規粒子の自動sleepを意味しない。

標準scalarのchunk局所sleepは[41](41_SCALAR_MATERIAL_SLEEP.md)で、候補停止と1cell移動haloに限って実装する。作者Providerの力積分については、read依存の宣言stencilと作者不変条件を使う専用sleep入口を追加した。圧力Jacobiにも`World::relax_pressure_sleeping`を追加した。参照反復の最終fieldが入力fieldと完全一致したときだけ証明を作り、World枝・Content/remap・設定・活動chunkとchunk revisionを照合する。圧力channel変更などで失効すれば参照solverへ戻り、StateCodecから復元した証明も参照実行まで使わない。正規粒子、作者behavior/timer/熱/進行中Jobには恒等証明なしにsleepを適用しない。複数force/inventory Providerを含むMaterials Runtime worker profileのcheckpoint・収支・rollbackは確認済みである。統合profileのworker別参照solver differentialと、正規粒子等の他表現sleepは**E05全体の残課題**である。chunk.activeをsleep bitへ転用しない。

## 検査と失敗時

tests/material_forces.rsでは標準入口の既存条件に加え、作者Providerについて空間stencil外の変更ではsleep継続、stencil内変更とWorld作者状態の変更ではwake、tracker decode後の参照再実行、各tickのreference report/完全checkpoint一致を検証する。

`tests/material_transport.rs`の圧力sleep試験では、固定点の3tick参照比較、圧力channel編集後のwake、保存trackerの復元後に参照solverを一度通すことを検証する。`tests/material_provider_sleep_benchmark.rs`には静止4,096流体cell・1 active chunk・4 Jacobi反復の手動比較を置く。Windows/MSVC・Rust 1.98.1・AMD Ryzen 7 2700（16論理CPU）で20tick warmup後に200tickを測り、参照は合計6.046秒、p50/p95/p99が30.129/31.021/33.370 ms、sleepは0.168秒、0.838/0.874/0.895 msだった。200tick区間の割当は参照1,650,800回/25,889,476,800 byte、sleep9,000回/25,724,836,800 byte、allocatorのpeak live増分は64,872,944/64,311,912 byte。証明作成の初回は参照29.588 ms、sleep29.597 msでほぼ同じだった。これは重力0の静止field・単一chunkの測定であり、動的圧力場、広域複数chunk、Runtime全体、144 FPSや60 Hzの達成を示さない。

4,096個の静止solid候補を置く単一chunk比較では、Windows/MSVC・Rust 1.98.1・AMD Ryzen 7 2700（16論理CPU）で20tickのwarmup後200tickを計測した。各tickにforce+motion処理と`advance_tick`を含む。参照経路はp50/p95/p99が30.800/33.699/34.331 ms、sleep経路は0.829/0.864/0.896 msだった。割当は参照4,109,400回/25,868,883,200 byte、sleep8,600回/25,724,612,800 byteで、Rust allocatorのpeak live増分はそれぞれ64,475,112/64,311,352 byteだった。別々の初回stepを一回ずつ測ると、参照33.596 ms、sleep証明作成44.596 msで、proofは参照1回より約11.0 ms多くかかった。以降は静止条件で200tick合計が約6.03秒短くなった。この初回値は単一計測であり統計値ではない。fixtureは静止・単一chunk・作者callbackが入力速度を返す合成条件で、動的材料、複数chunk、Runtime worker、描画やゲームFPSの性能を示さない。

判定は各tickのMaterialForceMotionReportとWorld::checkpointの完全一致。skip対象外で例外が出た場合は参照World::editと同じrollbackとなり、失敗した計算をキャッシュ作成済みとして偽装しない。E05/Q04工程全体の合格を意味しない。
