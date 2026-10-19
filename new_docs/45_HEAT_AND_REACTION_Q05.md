# E06/Q05 熱と反応の実装

## 所有データ

- `ThermalCellState`はenthalpyだけを持ち、対象simulated layerのmaterial追従channelへ保存する。material輸送はenergyも同じ取引で運ぶ。
- `ThermalEdgeState`は`+x`・`+y`辺の伝導residualと1/8単位の平衡creditを持ち、位置固定channelへ保存する。materialが動いても辺の端数は格子位置から離れない。
- `ReactionSidecar`は燃料、濡れ、腐食、timerを持つmaterial追従channelへ保存する。
- `ReactionJob`は安定順候補、cursor、rule key、source snapshotをStateCodecで保存する。復元後のstepでは作者登録ruleとWorldのsource/contactを照合する。

`EnthalpyFamily`は構築時にenergyと温度が連続する顕熱・潜熱区間を検証する。顕熱は整数線形補間、潜熱は固定温度で相分率を計算する。隣接区間はenergy、温度、相keyの全てを連続させる。phase fractionが1/2以上になる相を、World cellの離散material keyとして選ぶ。温度・fractionの正規値はenthalpyから導出する。

`WorldEdit::step_thermal_channel`はcaller指定矩形のactive cellだけを処理する。各`+x`・`+y` edgeを一回ずつ評価し、Qeq/8の整数端数をtick間で保持する。必要な場合は8 credit単位で最小1 energyを移し、端数熱量を恒久的に切り捨てない。delta scratchで等量を加減算した後に、cell energy、edge residual/credit、相materialを同じWorld候補へ反映する。active/inactive・Unavailable境界は伝導しない。Blockの相変更に必要なinstance migration ruleはE07待ちであり、現在はその相変更を取引全体ごと拒否する。

`WorldEdit::step_reaction_channels`は作者の`ReactionRule`を安定順に候補化し、入力、接触、温度、燃料、濡れ、出力先を再確認してから確定する。反応ごとにsource energyを数量比で分割し、`energy_delta`を明示収支へ加える。ruleはsidecarをsourceに保持、outputへ移行、破棄、消費する方法を選ぶ。出力先cellとparticleの両方を選べ、粒子生成はE04の全material-following channelをpayloadとして引き継ぐ。cell空間やparticle slotが足りない候補は入力、fuel、job cursorを変えず延期する。payload byte容量超過を含むcommit失敗はWorldEdit全体をrollbackし、sourceを消費しない。job recordとcell/channel/particle変更は同じWorldEdit候補へ書く。

反応chainはrule単位で`Deferred`と`SameTick`を指定する。候補job内に未処理candidateがあればそのruleのpolicyを`WorkStatus`としてRuntime schedulerへ返す。`SameTick` ruleの出力が別ruleのinputになる場合、jobは保存可能なchain-pending印を立てる。候補batchの完了後は`ContinueSameTick`を返し、呼出側は次の同tick実行で`resume=None`としてcommit済みWorldから候補を再構築する。`Deferred`は次tickへ継続jobを保存する。再実行回数はRuntimeの同tick予算に従い、同じ壁時計callbackを無制限に回さない。

## 計算量と上限

熱solverはN cellのscratchを事前に予約し、最大4N edgeを評価する。各cellの温度とcurve区間は一度だけscratchへ求める。同じ顕熱区間で温度がenergyごとに厳密増加する場合は平衡fluxを直接計算し、段差・潜熱・区間横断ではenergy範囲の二分探索へfallbackする。最悪計算量は`O(N × log2(energy範囲) × log2(区間数))`、通常の同一区間経路は`O(N × log2(区間数))`。scratchは`WorldLimits.job_bytes`、cell数は`job_cells`、公開編集数は`edit_cells`で事前に制限する。追加heap割当はsolver呼出一回あたりO(N)本。Sleep中はsolver自体を呼ばないため走査費用はない。

反応候補はruleの`(priority,key)`順を構築時に固定し、その順で座標整列済みのcellを走査する。chain対象inputも構築時に一度だけsort/dedupする。出力の候補全件sortを行わず、scratchは`min(active物質cell数, candidate_limit)`件を先行予約する。生成計算量は`O(active cell数 × rule数 × contact検索)`で、候補順sortは不要。実行はcaller budgetを越えず、各候補は座標binary searchと出力挿入を行う。`tests/thermal_reactions_benchmark.rs`の100,000cell/199,300 edge熱solver単体はRyzen 7 2700・最適化test profileで50回計測し、P50 12.847ms、P95 12.993ms、P99 13.077ms、追加割当0だった。同じ環境で10,000反応候補job構築を50回計測し、P50 11.196ms、P95 12.080ms、P99 12.290ms、毎回1 allocation/9.2MB、追加peak約9.2MBを記録した。候補構築単体であり、WorldEdit・particle pool確保・他simulationは計測範囲外なので、実Worldの60 Hz達成は示さない。

## 既知の範囲

`ReactionRuleSet`は構築時にrule key、入出力material、数量、出力位置を検証し、対応するenthalpy familyを保持する。tickごとに静的rule全体を再検証せず、保存候補は該当ruleとsource/contactだけを再照合する。

`EnthalpyFamily`と`ReactionRuleSet`は作者が構築時に検証して各world systemへ渡す不変定義であり、標準`ContentRegistry`への一括格納APIは設けていない。相変化がblock個体へ移る場合のinstance migrationはE07の連成境界で、このQでは候補全体をrollbackしてblock stateを失わない。E06出口ではcell素材と正規粒子の相変化・反応、および記載したQ05検証を判定し、block instanceの相変換はE07で別途検証する。

工程出口条件は[18](18_VALIDATION_AND_RELEASE.md)と[19](19_IMPLEMENTATION_ROADMAP.md)、反応・熱の固定契約は[10](10_MATERIALS_AND_CELL_PHYSICS.md)・[22](22_REFERENCE_ALGORITHMS_AND_TRANSACTIONS.md)に従う。
