# 広域圧力と気体の圧力源

E05の圧力計算を分割して進める公開API。工程全体の完成判定は[E05の記録](evidence/E05/README.md)を参照する。

## 作者の使い方

`PressureJob::new(&world, layer, config)`が入力を採取し、`step(&world, budget)`が隣接構築、連結探索、連結量の反映、Jacobi更新と二重bufferの切替をcell単位で進める。各cellの四近傍は上・左・右・下の固定順で、列挙順やstepの分割で結果は変わらない。`completed_work`、`completed_iterations`、`sample_count`、`report`で進捗を診断する。0予算では進めない。各stageの切替も一件として数える。

完成した場合だけ`Ready`を返す。`WorldEdit::publish_pressure_job`が全出力の最終channel容量・編集容量を検査し、一括公開する。既定値に戻す出力を先に反映して空chunkを解放する。途中の圧力・地形は公開しない。失敗をcallbackが無視しても外側のWorld取引を破棄する。

```rust,ignore
let mut job = PressureJob::new(&world, layer, config)?;
job.step(&world, 64)?;
world.edit(|edit| edit.set_state(job_key, job_schema, &job))?;
if job.status() == PressureJobStatus::Ready {
    world.edit(|edit| edit.publish_pressure_job(&job))?;
}
```

schemaは起動時に`Schema::of::<PressureJob>`で登録する。型の保存形式は`CHPRJOB3`。素材別の係数・lambda・上限を入力へ保持する。本版はPR初期の`CHPRJOB1`・`CHPRJOB2`を拒否し、自動migrationは行わない。独自solverの採用を制限せず、作者は同じchannel・StateStore・取引APIで別の数値方式や分割規則を使える。

## 従来profileとの一致

`PressureJobConfig.gas = None`、`pump = None`では、既存の`relax_pressure`と同じ液体・気体の連結、自由表面0、閉領域の前回平均anchor、massと重力項、Jacobi反復を使う。浮動小数点は使わず、同じ反復数なら分割予算にかかわらず同じ出力になる。標準反復数4は製品の固定制限ではなく、呼出側が`PressureRelaxation`に指定する。Noneでは広域の気体profileを無効にし、素材の気体source係数・lambda・気体profileの上限を適用しない。

## 気体profile

`GasPressureSource`を渡すと、液体と気体の連結を分離する。気体は同じ活動域の気体・空cellを四近傍で連結し、全素材の気体massと絶対温度Q16を積算する。空cell一つを含む各sampleが容積1である。

`target_q16 = floor(Σ(mass_per_cell(i) × temperature_q16(i) × coefficient_q16(i)) / (volume × 65536))`

温度は`GasPressureSource.temperature`のi64・1対1channelから読み、位置固定または同layerの物質追従を選べる。温度と出力圧力は別channelを要求する。圧力の係数・緩和の重み・上限は`MaterialBuilder::gas_pressure(GasPressureProperties)`で素材ごとに登録する。

| 物性 | 標準値と意味 |
| --- | --- |
| `coefficient_q16` | 65536。非負Q16で、その素材のmass×温度の積へ掛ける。0も有効 |
| `relaxation_weight` | 1。各cellのtarget緩和の重みlambda。0では直接のtarget項を加えない |
| `maximum_q16` | i64::MAX。非負Q16圧の上限。超過は丸めずエラー |

気体以外への明示設定と負の上限は素材登録で拒否する。`MaterialDef::gas_pressure`から解決した物性をjobの各cellへ採取するので、同じ連結領域でも異なる素材に異なるlambdaが適用される。明示物性はContentの物理manifestへ含め、変更を保存互換と誤認しない。物性の明示設定がない既存Contentのmanifestは維持する。負温度・負の入力圧・負のポンプ値、計算のu128積算・i64出力の超過、素材の圧力上限超過はエラーにする。

標準係数でmass2・温度3・容積3ならtarget圧2、容積2ならtarget圧3となる。実際に公開する圧力は前回圧・近傍圧・重力・素材別lambdaからの緩和結果である。

`Pnext(i) = max(0, trunc((Σ(Pneighbor + rho_avg*g*dy) + lambda(i)*target)/(neighbor_count + lambda(i))))`

近傍がなくlambda=0なら前回圧を維持する。活動中の空cellは近傍の相対圧0として扱い、気体cellの前回圧もtargetへ直接上書きしない。targetはlambda項だけから加え、空cell境界から迂回して注入しない。公開するのは占有する流体cellのみで、空cellには気体圧を保存しない。block、固体、粉体、液体、World境界、活動外、未読込は気体空間のno-flow境界。有限Worldの部分chunkの外を容積へ含めない。空だけの空間を流体領域の診断件数へ含めず、温度0の気体領域は数える。

これはcell占有のゲーム用pressure proxyであり、実在流体の精度、異なる容積のtile流体、圧縮性の連続体計算を保証する方式ではない。圧力だけを理由に気体を発行・消去しない。物質移動は既存の輸送・押出しphaseに任せる。温度は作者が与える状態であり、enthalpyからの温度解決はE06で接続する。

## ポンプ境界

`pump = Some(channel_key)`は、別の位置固定・1対1のi64 channelの非負Q16値を各流体cellの最低圧にする。自由表面や気体targetより高い値も明示的に指定でき、隣接する液体への伝播は同じJacobi反復で行う。0なら通常の境界を変えない。圧力設定は仕事量・energy・物質sourceの発行記録ではない。作者が機械の消費・発行を持つ場合は別の理由付き取引で記録する。

## 入力失効と保存再開

World ID・Content fingerprint・material epoch・活動chunk集合とそのrevisionを保持する。入力の占有、channel、活動集合が変われば古いjobを失効させる。無関係な位置の編集でも同じ活動chunkなら再計算が必要な保守的方針である。完成後の公開によるrevision変更で同じ非空jobの二重公開を拒否する。出力がないjobの公開は状態を変えない。

保存には入力と計算中の配列・queue・cursor・反復数を含め、復元後もその続きから進める。完成済みrecordはReadyを維持するが、公開前にWorldの実入力全体を再採取して照合する。切断・不正件数・負値・数値上限・不正index・隣接関係・不正段階もdecodeで拒否する。連結成分を一度構築し、stage・cursor・rootから訪問集合、BFS順のqueueと発見prefix、反映済みsource・anchor、部分集計と診断件数を導いて保存値と照合する。未処理の別成分を訪問済みとして飛ばすrecord、別rootのqueueや順序の入替えは受理しない。Jacobiでは処理済みprefixのnextを現在のoldから検算し、未処理suffixの保持値とcursorを照合する。buffer切替ではコピー済みprefixのoldとnextの一致、初回の未コピーsuffixの初期値を検査する。完成時もoldとnextの一致を要求する。

`WorldEdit::publish_pressure_job_then_search_push`は圧力出力を同じ候補へ適用した後、その新しいpressure channelからPressurePushSearchを作る。探索budget内でReadyとなれば押出し経路も一取引で公開する。budget不足なら圧力fieldと進捗中searchを成功値として返すため、呼出側は同じ外側WorldEdit内でsearchをStateStoreへ保存できる。新pressure公開後のsearch構築・step・経路輸送が失敗した場合、候補全体をrollbackする。探索scratchはjob_bytes、活動cell採取とsearch budgetはjob_cellsに収める。別入口`publish_material_motion_pressure_jobs_and_pushes`は、完成済みPressurePushSearchを保存時のsnapshotのままPressureJob/DDAと共通候補へ載せる。

正規粒子と空き出口が重ならないよう、PressurePushSearchは開始時に同layer・活動域内のresident粒子cellを一度集め、座標順の索引を保持する。各候補・復元nodeの占有検査は二分探索で、同一layer粒子を経路・出口から除外する。索引構築はO(P log P)、各候補検査はO(log P)、record追加量は8P+4byteであり、Pは対象layerの活動域内resident粒子数。`CHPUSH03`が索引を保存するため、budget=1の毎phase復元でも粒子arena全体の再走査と再整列を繰り返さない。既存`CHPUSH01/02`を読み込んだ場合は最初のstepで索引を作り、node占有を予算付きで再検査する。旧形式を新形式へ再保存したrecordはStateStore schemaのbyte上限に従う。

old/nextの配列は保存する正規数値状態であり、過去の反復履歴を全再実行して検査しない。保存bufferのchecksumは既存の保存層が担う。jobを毎phaseでStateStoreから取得し、一件進めて再保存しても有限の仕事量で完了する。復元のたびに検査cursorを最初へ戻す方式は採用しない。

通常のstepはfrontier全体を複製しない。計算中の数値エラーは現在のcell処理直前で止まり、先に終えた仕事はjobに残る。Worldは変更されない。値域を超えた入力を直して同じjobを使うことはできず、変更後のWorldから新しいjobを作る。

## 費用と未統合の範囲

- newの入力採取は活動chunkの全4096cellを走査し、sampleを安定順に並べる。気体profileでは空cellも保持する。`cell_limit`にそれらを含め、容量不足なら開始しない。
- stepの予算はcell処理の件数で、隣接検索にはsample数に対する二分探索を使う。毎stepの活動集合・revision照合は別途chunk数に依存する。wall-clockの一定時間保証ではない。
- scratchはsample配列と最大sample数の探索queue。保存・復元は配列全体に比例する。StateStoreへ毎step保存する費用はsolverの件数予算に含めない。
- decodeの構造検査は配列全体、隣接の二分探索、一回の連結探索と現反復のprefix検算に依存し、訪問bit列と探索queueの一時scratchを使う。StateStoreを毎phaseで読み書きする費用は残るが、過去の反復の再計算や照合用の二重scratchを必要としない。
- 公開は入力を再採取するscratch、全入力比較、出力列、channel容量予約と一括書込の費用を持つ。全反復のやり直しは公開時に行わない。

このAPIを単独で全活動域を自動分割するscheduler、dirty/sleep、全solverのprovider登録、力の積分、物質粒子・混合・全表現収支の完成とは扱わない。巨大域で入力採取や最終公開まで厳密な時間分割が必要な作者は別profileを使える。これらのE05統合とQ04は続いて検証する。

## 検証

`tests/pressure_job.rs`で従来profileとの保存byte一致、予算1を含む分割、各計算段階のcodec復元、Worldとjobの同時保存、温度・容積・連結・chunk境界・非活動域・有限境界、ポンプ、lambdaの0・1・3と空cell隣接の参照式、入力失効、負値と数値上限、容量不足・無視されたエラーのrollback、素材別の係数・lambda・上限とContent fingerprint、毎phaseのStateStore復元、切断・不可能な診断件数・queue外の不正訪問bit・BFS順の不整合を検査する。さらに新pressureからの押出し探索・公開と、探索失敗時のpressure出力rollbackを確認する。`cargo run --locked --example pressure_job`はmass2・温度3・容積3でtarget Q16=131072、lambda1と空境界0からの緩和結果Q16=65536を検査し、途中保存と連続実行のcheckpoint一致をassertする。

同期型`WorldEdit::relax_pressure`には、Jacobi反復の出力fieldが入力と完全一致した後の反復を省略する`World::relax_pressure_sleeping`を追加した。証明はWorld枝・Content/remap・設定・活動chunkとchunk revisionに結びつける。値の変わらないpressure channelは参照solverでも書き直さず、不要なchunk dirty更新を避ける。詳細なwake・復元契約と固定点benchmarkは[39](39_MATERIAL_SLEEP.md)および[E05検証記録](evidence/E05/README.md)に記録する。分割実行する`PressureJob`へのsleep適用ではなく、粒子やprovider pressure sourceの自動sleepも含まない。
