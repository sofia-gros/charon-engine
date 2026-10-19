# E05：物質の輸送と圧力の公開API

## 状態

E05は実装中。このページは追加した輸送・局所移動・圧力緩和・押出し探索の契約を説明する。E05全体の完了やQ04の通過を意味しない。検証と残作業は[evidence/E05](evidence/E05/README.md)へ記録する。

## 位置と物質の所有権

`ContentRegistry::channel`は位置へ属する場を登録する。圧力や配線の値は、物質が動いてもその位置に残る。

`ContentRegistry::material_channel(layer_key, definition)`は、指定layerの物質に所属する値を登録する。energy、timer、混合状態への型付きIDなどを格納できる。同じ座標の別layerの輸送には追従しない。対象layerは先に登録する。Contentの確定後に所有権を変更しない。

この追従経路は一つのWorld cellに一つのchannel要素が対応する格子を扱う。原点は任意で、World座標から原点を引いてchannel座標へ変換する。変換のi32範囲を超える操作は失敗する。別解像度の分配・合流は通常channelと作者の明示的な変換取引を使う。別解像度を登録できる既存APIを変更・制限するものではない。

`World::channel_layout::<T>`と`channel_material_layer::<T>`で定義を確認できる。値型が違えばエラーになる。

## 原子的な輸送

- `WorldEdit::exchange_cells(layer, first, second)`は隣接二点を交換する。
- `WorldEdit::transport_path(layer, path)`は経路の各旧値を一つ先へ送り、末尾の旧値を先頭へ戻す。押出しには末尾の空cellを含める。

CellCoreのflags・variationと、そのlayerの全物質channelを同じ取引で輸送する。呼出側が移すchannelを列挙する必要はなく、選択漏れを防ぐ。経路の全点について読込、境界、重複、隣接、block個体、通常編集予算を検査する。編集予約数は`経路cell数 × (所属する物質channel数 + 1)`で、地形だけを数えてchannelの費用を隠さない。block個体は暗黙に分解しない。斜め隣接を許すが、形状による通過可否はsolverが判定する。

channelの旧値を退避してから対象点を既定値へ戻し、再配置する。これにより、一つしかchannel chunkを確保できない場合も、空になったchunkを解放して移動先を確保できる。全channelについて輸送後に残る非既定値数と必要chunk数・byte数を配列確保前に計算し、World全体とchannel個別の上限を検査する。容量を解放するchannelから処理し、一時的な確保量もWorld上限を超えない。他の値が残っていて容量が足りなければ失敗する。失敗をcallbackが無視しても、外側のWorld取引は地形・channel・先行編集をまとめて破棄する。

保存形式の構造は変更していない。所有権はContent manifestに含まれ、位置固定と物質追従を互換と誤認しない。通常channelだけの既存manifestは維持する。channel値は既存のcheckpoint・区域退避・復帰に従う。

## 局所移動の参照phase

`WorldEdit::transport_materials(layer, candidate_limit)`はSimulate leaseで活動しているlayerの非block cellを走査する。Runtimeへ隠れた重力を自動追加する処理ではない。作者Systemがphaseを明示して呼び、World時計の更新はRuntimeへ任せる。

| 分類 | 候補の順 |
| --- | --- |
| Solid | 移動しない |
| Powder | 下、左右の斜め下 |
| Liquid | 下、左右 |
| Gas | 上、左右の斜め上、左右 |

下から上へ、行内はWorld tickで左右を交互に走査する。異物質の交換は、下降する重い物質または上昇する軽い物質と、液体・気体の移動先との間で行う。固体と粉体を密度交換で崩さない。両側の直交点が共に塞がる角を斜めに通過しない。

両端の予約で、押し退けられた物質も同じphase中に再移動しない。予約scratchは活動chunk当たり4096bit。活動外・未読込・World境界を空きとして使わない。候補数や編集容量が足りない場合は部分更新を成功扱いにしない。このphaseは1cellの局所移動であり、速度残差・複数cell sweepの実装済み表示には使わない。

## 圧力緩和

`WorldEdit::relax_pressure(layer, PressureRelaxation)`は、位置固定の1対1格子のi64 channelへ非負Q16の圧力を保存する。反復数・重力係数・流体cell予算を呼出側が渡す。標準4反復を固定の製品制約にしない。

前反復の配列から次配列へJacobi更新し、自由表面を0、固体・block・活動外・未読込をno-flowにする。閉じた連結領域は最小座標のanchorに前回の平均を保持する。計算中間値はi128で、i64へ戻せなければ失敗する。全反復が完成してからchannelを一括公開する。全出力の非既定値数から最終的な必要chunk数・World全体のbyte数を事前検査し、channelの既定値へ戻る出力を先に反映して空chunkを解放する。既定値が0以外でも、座標の走査順で不要な同時確保を要求しない。圧力更新で物質やenergyを発行しない。

気体の温度・連結容積によるsource、ポンプ境界、保存可能な広域緩和は別APIの[33](33_WIDE_PRESSURE.md)で扱う。この同期APIがそれらを自動実行する意味ではない。

## 満杯経路の押出し

`PressurePushSearch::new`で流体始点と圧力条件、node容量を指定する。`step(world, budget)`で最大budget nodeの四近傍を上・左・右・下の固定順に調べる。同じ素材の活動cellを通過し、空き先との圧力差と重力項が閾値を超えた経路をReadyとする。同layerの活動域にいる正規粒子のcellは探索時・保存node再検査時とも占有済みとして扱い、出口にも経路にも選ばない。活動中の空cellは相対圧0として扱い、位置固定channelに残った旧圧力値を使わない。保存から復元したReady経路の再検査・公開にも同じ境界条件を適用する。

Searching、Ready、NoOutletを区別する。容量不足はエラーであり、NoOutletへ丸めない。step失敗時はcursorとfrontierも変更しない。`set_node_capacity`で上限を増やして同じfrontierから再開できるが、StateStoreのrecord byte上限は独立して残る。World ID、Content、material epoch、入力した全活動chunkのrevisionと活動集合を照合する。失効時は古い経路を使わず再探索する。復元recordは親node・隣接・重複・所属領域だけでなく、再開・公開時に実際の物質・空き先・driveも再確認する。

訪問済みbitsetは探索ごとに活動chunk当たり4096bitを保持し、各stepでfrontier全体を複製・再構築しない。追記したnodeとその訪問bitだけを失敗時に巻き戻す。さらに同layer・活動域内のresident粒子cellを一度収集・座標順に索引化し、候補ごとに二分探索する。構築は粒子数Pに対してO(P log P)、一候補の照合はO(log P)、索引と保存byteは8P byte（加えて個数4byte）で、Pは活動域内の同layer粒子数である。chunk revisionが変化した場合はjob全体を失効させるため、占有索引だけ古く残らない。毎phaseにJobをStateStoreから保存し直す経路ではこの索引もrecordへ含め、全粒子の再走査と再整列を繰り返さない。

`CHPUSH03`は粒子占有索引と照合済みnode数を保存し、毎phaseのStateStore読込・保存で同じprefixのnode照合や粒子の再整列を繰り返さない。旧`CHPUSH02`・`CHPUSH01`も読める。旧形式は最初のstepで索引を作り、既存nodeの実地形・粒子占有照合をbudget一件ずつ最初からやり直す。照合中はSearchingを返し、探索cursorを進めず、全照合後に探索を再開する。新形式で照合済みのReady・NoOutletは復元直後も同じ状態を保つが、stepと公開は入力contextを照合する。保存record内の粒子索引は形式検査だけではWorldとの一致を証明しないため、公開前に現Worldから索引を再構築して保存値との一致を検査し、その実索引で全nodeを再確認する。

一stepの仕事は最大budget件のnode照合・展開と、活動chunk集合・revisionの検査である。旧recordからの索引再構築は一回O(P log P)を追加する。公開境界でも現在の索引を一度再構築するためO(P log P)を要し、保存索引との差があれば輸送候補を公開しない。後者の活動chunk検査は読込済みchunk走査と、記録した活動chunkのrevision照合に依存する。node Vecの拡張は償却費用で、常に一定時間を保証するものではない。全recordの保存・復元・明示的な候補複製と公開取引はnode数・粒子索引サイズに比例する費用を持ち、毎stepで全状態を保存する呼出側の費用までbudgetへ含むとは説明しない。旧形式と新形式のbyte上限はStateStore schemaの別上限に従い、新形式では粒子索引分として8P+4byteが増える。bitsetは保存nodeから再構築する。保存カーソルは構造的な範囲を検証する進捗値であり、実地形の監査証明ではない。

探索はStateCodecを実装し、作者がschemaを登録してWorld StateStoreへ保存できる。これは物質payloadをglobal stateへ移す処理ではなく、検索frontierと入力revisionの保存である。

```rust
// search_schemaは起動時にPressurePushSearchのschemaとして登録する。
// 探索中は物質を消費せず、Worldへ進捗だけを保存する。
search.step(&world, 64)?;
world.edit(|edit| edit.set_state(search_key, search_schema, &search))?;
// Readyのときだけ、完成した全経路を一括輸送する。
world.edit(|edit| edit.publish_pressure_push(&search))?;
```

公開でchunk revisionが変わるため同じ探索の二重公開は拒否する。入力集合を保守的に追跡するので、同じ活動集合の別地点の編集でも失効し得る。再探索費用を明示し、保存可能な探索だから並行編集と常に両立すると説明しない。

## E05の残作業

[31](31_MATERIAL_MOTION.md)の整数格子の速度・端数・経路検査を物性とproviderへ統合し、広域移動の継続Jobを実装する。[35](35_MATERIAL_PARTICLES.md)では正規粒子のpool・全payload所有権・DDA・着地と所属regionとの退避・復帰を追加したが、これだけでE05を完了とはしない。[34](34_MATERIAL_MIXTURES.md)の混合・分離と粒子を含む物性・provider統合、[36](36_MATERIAL_ACCOUNTING.md)で接続した全表現の収支・保存監査をRuntimeへ統合すること、[33](33_WIDE_PRESSURE.md)の広域圧力を含むdirty/sleepの参照比較、Runtime作例とQ04全体を完了してから工程を通過させる。

## 全表現の収支との接続

[36の全表現収支](36_MATERIAL_ACCOUNTING.md)は実所有量を集計し、基準と理由付き増減へ照合する。cell・混合・粒子の個別記録を総量へ二重計上しない。標準body physics、物性統合、広域移動、dirty/sleep、RuntimeのQ04完成を示すものではない。
