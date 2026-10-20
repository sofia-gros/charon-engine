# 世界と保存基盤の公開API（E04）

このページは`charon_engine::world`の実装契約です。製品要件は[09](09_WORLD_AND_STREAMING.md)、[14](14_PERSISTENCE_AND_REPLAY.md)、[25](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)を参照してください。検証結果は[E04記録](evidence/E04/README.md)へ分けます。

## 作者の組み立て方

コンパイル対象の全体例は[examples/sandbox_world.rs](../examples/sandbox_world.rs)です。ゲーム固有の素材名、生成規則、家具の状態を作例側へ置いています。

1. `ContentRegistry`へ`LayerDef`、`MaterialBuilder`、作者の`Schema`、`ChannelDefinition<T>`を登録します。
2. `freeze()`で安定キー順の辞書とBLAKE3 fingerprintを確定します。以後のContentは不変です。
3. `World::new`またはRuntimeの`WorldBuilder`でseed、論理境界、各容量を指定します。世界の面積分の配列は確保しません。
4. `GenerationPipeline`のpassを登録し、region要求・worker計算・結果提出・指定tickでの公開を分けます。
5. `World::edit`の中でcell、block、状態、channel、leaseを編集します。途中のエラーを無視しても候補全体は失敗します。
6. `World::save`でresidentと退避済み区域を一つの保存rootへ収めます。復元は独立候補を検証してから呼出側が採用します。

```rust
use charon_engine::world::{MaterialBase, MaterialBuilder, BreakMode};

// 大きさは物理格子のcell単位です。画像のピクセル寸法とは独立しています。
let stone = MaterialBuilder::new("game:stone", MaterialBase::Solid)?
    .block_size(16, 16)
    .break_mode(BreakMode::Cell);
registry.register(stone)?;
```

上の断片で`registry`は作者が用意した`ContentRegistry`です。背景、配線、前景を同一layerへ押し込める必要はありません。Solid/Powder/Liquid/Gasは分類であり、E05以降の移動solverを実装済みとする表示ではありません。

## cell、block、形状

- 64×64cellのchunkを疎な辞書で保持します。`CellCore`は8byteで、block参照は必要なlayer pageにだけsidecarとして確保します。
- `BlockRef`はWorldと世代付き個体IDを持ちます。回転は90度単位で、跨chunk配置は全占有先の読込・空き・容量を確認して一括公開します。
- Full/Half/Triangle/Platform/OccupancyMaskと登録済みCustom形状を扱います。textureの透過率を占有判定へ暗黙に使いません。任意角度の回転bodyとexact shape連成はE07です。
- `AnchorRule`は自由配置、指定layerの下支えAny/All、独自validatorを選べます。下支えが壊れた後の自動崩壊を全ゲームへ強制しません。
- 個体の独自componentは`set_block_state`、World共通状態は`set_state`で登録schemaへ接続します。家具のinventory等のゲーム規則は作者が定義します。
- queryは`Loaded`、`Unavailable`、`WorldBoundary`を区別します。未読込を空気として返しません。隣接8方向にも同じ区別を保ちます。

## 生成と編集履歴

`GenerationPass`はkey、version、依存pass、configuration、`generate`を定義します。Pipelineは依存を解決し、同順位を安定キーで固定します。入力はseed、絶対chunk座標、境界、不変Contentです。`anchor_seed`で絶対位置の候補を決め、対象chunkへclipします。共有乱数の消費順に構造物を依存させません。

`GenerationTask`はWorldの可変参照を持たず、threadへ移せます。要求はRequested→Preparing→Ready→Published、またはFailed/Cancelledへ進みます。完了の順番でWorldを書き換えず、due tickで明示公開します。現在tick期限の要求は`publish_generation`で明示公開します。未来のdue tickへ到達する際は`advance_tick`が遷移先を検査し、そのtick期限の要求が全てReadyなら時計と区域を一括公開します。Requested/Preparing/Failedが一つでもあれば時計を進めず、部分公開しません。さらに未来の候補は先取りせず、Cancelledは待機条件から除きます。

worker再開には`restart_generation`を使います。要求IDの世代に加えて試行番号を照合するため、再開前のworker結果は混入しません。Preparingの保存を復元したら作者のworker管理が再開を指示します。OS thread自体は保存しません。

生成時のpageは不変baseとして現在地形と共有します。編集したlayerだけ分離し、その保持量をterrain予算へ数えます。保存はbaseと現在地形の差分を区別し、少数の変更はu16 indexと8byte cellで符号化します。差分が大きい場合やblock sidecarが変わった場合は完全pageへ切り替えます。生成器がなくても既に生成した地形と編集を復元できます。未生成区域の要求に必要な版がなければ明示失敗します。

`generation_stamp`と`generation_base_cell`で生成履歴と編集前の値を照会できます。生成器の新版で編集済みchunkを上書きするAPIにはしません。不変baseの保持・保存費用は明示し、再生成だけで保存できる最適化を実装済みとはしていません。

## activity、カメラ、退避

`LoadOnly`はresident、`Simulate`はactive、`Pin`は退避禁止を要求します。sourceと期限付きleaseの和を解決し、resident/active/byte予算を超えれば全操作を拒否します。描画されていなくても遠隔設備のleaseを維持できます。

`CameraObservation`はcameraの回転・zoomを含む領域をWorld境界へclipします。既定でresident marginは2chunk、simulation marginは1chunkです。必要なchunkをqueryして準備した後、入力tickで観測を適用します。同じsourceのcamera要求だけを交換し、別sourceの工場や別cameraの要求を消しません。描画threadからWorldを直接変更しません。

退避は`prepare_eviction`→`EvictionTask::store`→`finish_eviction`です。保存の再読込照合が成功したreceiptだけを受け付けます。保存中に対象区域が変われば拒否し、無関係な区域の編集は許容します。跨chunk個体は全所属chunkをまとめ、leaseと作業中Jobの予約を破りません。RAMのpage、個体mask、個体状態、channel部分を解放し、個体allocatorの休止slotと小さいarchive indexを残します。

復帰は`region_load_task`→`load`→`restore_region`です。個体ID、独自状態、channel、生成baseを戻し、現行material epochとの差を適用してから公開します。容量不足や破損で部分復元しません。E07のBodyPhysicsとjoint graphは跨region接続を保つためCHWORLD3 World rootへ保存し、region別CHREGN02 payloadにはterrain等を保存します。`World::save/load`はWorld rootと参照region archiveを一つの保存世代に結び、bodyのcollision sweepはresident chunkだけを通し、跨region更新前にcallerがleaseを確保します。E07のWorld保存・region復帰と事前lease経路を[E07検証記録](evidence/E07/README.md)で検証します。E08以降のEntity/AI等は、各工程の所有単位と同じ保存契約への接続を定義します。

## 独自channel、remap、Job

`ChannelDefinition<T>`は値schema、原点、各軸の有理数scale、既定値、容量を指定します。半解像度や負座標に対応し、Worldのresident coverageを満たす位置だけを操作します。`channel_batches`は格納chunk単位の不変sliceを公開します。GridChannelのarrayを地形cell本体へ固定的に追加しません。部分液量も整数・固定小数点など作者が選ぶ値型で保持できます。

`MaterialRemapTable`はowner、layer、安定素材キー、epochを保持します。地形cellとblockの標準ownerはresidentへ一括適用し、後から生成・復帰する区域にも同じ履歴を適用します。独自ownerは同じ台帳から変換を解決できます。blockの寸法・形状定義（Customのprovider/版/recordも含む）が変わる変換は、同じ占有cell数でも標準remapの登録時に拒否します。未配置・退避中の個体も同じ契約です。作者が再配置・component移行・参照更新を含む明示取引を実装します。未知キーを空気へ置換しません。

大きなcell編集は`submit_cell_job`、予算付き`step_cell_job`、`publish_cell_job`で処理します。cursor、入力revision、予約chunk、候補pageを保存し、途中をqueryへ公開しません。入力変更時はInvalidatedを返します。復元時はcursorまでを照合し、保存された候補だけに別の編集を混ぜられないよう検証します。blockを含む大規模prefabは作者が個体APIと自身の正規Jobを組み合わせる対象であり、cell Jobで個体を上書きしません。

`changes_after`は直前revisionからのchunk通知を返します。通知を取り逃した場合や復元後はFullRefreshRequiredとなり、各cacheは正規queryで再構築します。通知自体を保存の正にしません。

## 保存形式と障害

E04の`CHWORLD1`はWorld、個体allocator、lease、未完了生成、Job、独自状態、channel、remap、退避indexを明示little-endian codecで保存します。E05の[正規物質粒子](35_MATERIAL_PARTICLES.md)を含む現形式は`CHWORLD2`で、末尾に粒子容量とallocator・recordを追加し、`CHWORLD1`も粒子ゼロとして読み込みます。区域は粒子payloadを含む`CHREGN02`を出力し、旧`CHREGN01`も読めます。`checkpoint`はメモリ状態のsnapshotです。退避済みpayloadも一緒に運ぶ完全な保存には`World::save`を使います。

`CHSAVE01`の世代rootにはengine版、profile、content hash、tick、plugin key/version、scope、kind、codec版、長さ、blob hashを記録します。`CHBLOB01`はkind、形式major/minor、codec版、展開後長、圧縮方式、payload checksumを持ちます。現在の圧縮方式はnoneで、未知方式を無視しません。BLAKE3はcontent/blob照合用です。既存の資産packのSHA-256は変更していません。

`SaveRepository`と`GenerationStorage`は保存媒体を分離します。ローカルadapterはwriter lock、temporary世代、file同期、不変世代の公開、root再検証、CURRENT更新、最低3有効世代の保持を行います。`SaveLimits::generations`は保持数keepより大きい値を要求し、新世代を検証してから整理するための最低1世代分を予約します。壊れた世代や一時世代も列挙上限へ数えるので、運用時は追加の余裕を設定します。新しい保存を始める前に、排他lockの下で中断した未公開世代を`GenerationStorage::discard_staged`により回収します。ローカルadapterはcanonical root直下のstagingと既知のobjectだけを削除し、公開済み世代・未知file・linkは削除しません。回収に失敗したら新世代を作らず、直前の公開rootを維持します。独自保存adapterもこの追加methodを実装します。CURRENT更新や旧世代整理だけが失敗して有効な公開世代が余っている場合は、完全性を再検証してkeep個を残してから次を作ります。破損した公開世代は保全し、空きを確保できなければ新規世代を作る前に容量エラーを返して既存rootの読込を維持します。CURRENTが壊れても世代一覧から最後の完全rootを選べます。保存streamが失敗・panicした場合もlockを解放し、不完全なrootを公開しません。

参照するregion archiveを一つずつ保存rootへ含めるので、未読込世界を全てRAMへ集めません。`World::load`はroot読込から参照blobの検証までread transactionでpruneとの競合を防ぎ、別の空のpaging保存先へも区域を復元できます。低水準の`latest_manifest`と`read_record`を組み合わせる呼出側も`read_transaction`を使ってください。

schema移行は`Schema`の移行codecと`schema_migration_from`で明示許可します。素材・layer・channelの意味や格子を変えたContentをschema移行として受け入れません。保留中生成には元のContentを保持し、移行後のworkerでも元の不変入力を使います。元saveへin-placeで書き換えません。

Windowsのfile同期とrenameを`FilesAndAtomicPublish`、Unixのdirectory同期を含む場合を別の`SaveDurability`で返します。特定hardwareの電源断耐性を単体試験だけで認定しません。形式はv1.0互換凍結前です。今回の未公開形式を旧simulationのsave互換と混同しません。

## Runtimeと費用

`AppBuilder::sandbox_world`/`EngineBuilder::sandbox_world`でnative Worldを登録します。Systemは`engine:terrain`のアクセスを宣言し、`WorldView::terrain`と`WorldAccess::edit_terrain`を使います。編集callbackはbarrierで実行され、失敗時はWorld状態とtickを共に公開しません。登録中の地形時計はRuntimeが所有し、callback適用前後と確定前に時計の一致を検査します。callbackによる`advance_tick`はpause中も含めてエラーとなり、先行する作者状態の変更も巻き戻します。独立したWorldの初期構築や作者の別ClockDomainはこの制約とは別の経路です。callbackの機械語やクロージャはserializeせず、確定tickの結果を保存します。

通常編集はchunk pageをCOW共有しますが、個体arena・管理辞書・検証にも費用があります。1cell編集が世界全体に対して定数時間とは説明しません。大規模操作にはJobとbatch、明示容量を使い、E13で参照実装との一致を保って最適化します。
