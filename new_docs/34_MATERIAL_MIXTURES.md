# 混合の組成・正確な成分量・原子的な変換

E05内の混合・分離API。物質粒子や剛体を含む全体収支、標準providerとdirty/sleepの認定は別の残作業で、E05全体の完成とは扱わない。

## 所有権と組成

`MixtureState`は純物質の安定キーと正の整数token比率を持つ。`new`は同素材を合算し、安定キー順へ整列して全比率を最大公約数で約分する。異なる2素材以上が必要で、空気の予約IDや未登録素材を成分として登録しない。同じ正規化組成の再登録は同じ参照を返す。

一つのWorldで`MixtureStore::key()`（`engine:material-mixtures`）に共有storeを作り、`MixtureRef`だけを1対1の物質channelへ置く。参照にはWorld IDとappend順のindexを持ち、別Worldと範囲外indexを拒否する。`MixtureRef::NONE`は純物質・空cellの一意な値。組成storeには質量所有権がなく、同じ組成を何cellが共有しても、定義の登録だけで物質は増えない。

定義を削除・再利用しないため、区域archive内の古い参照も有効なまま残る。標準APIは使用中storeの再初期化を拒否する。Worldの汎用`set_state`と内部のencoded書込みでも固定キーの型を照合し、現在の全定義・witnessと完全一致することを同じ取引内で要求する。先に作った空storeや古いsnapshotで既存定義・履歴を巻き戻すことはできない。汎用書戻しは容量変更だけを許し、定義・witness追記は`register_mixture`・`apply_mixture_changes`の専用取引で行う。参照先のMaterialReceiptLogも汎用書戻しで履歴を変えられず、cell変更と同じ取引でのみ追記する。別Worldの分岐から同じIDで履歴だけを持ち込む操作も拒否する。完全checkpointの復元は世界全体の復元であり、この差分書戻しとは別の操作になる。`entries`・一組成の`components`・変換履歴の`receipts`上限を明示し、枠不足では定義や物質を自動破棄しない。容量は既存データを失わない範囲で変更できるが、schema・Worldのbyte上限は独立して残る。

## carrierと成分量

実際の占有・密度・移動分類は作者が登録したcarrier素材の`MaterialDef`を使う。CellCoreやmass_per_cellの形式を変更せず、carrierの1cell質量Mを、その組成の純物質へ正確に展開する。

成分iの純物質質量をmi、整数比率をriとすると、純物質cell相当のtoken数は`M × ri / Σ(mi × ri)`、その成分の換算massは`token数 × mi`。分母を含めて約分するので、端数を切り捨てない。成分massの合計はcarrier massのMと一致する。carrier自体をさらに純物質として重ねて計上しない。

| 占有状態 | 水mass5・油mass3、比率1:1の成分量 |
| --- | --- |
| mass4のcarrier一cell | 水token1/2・mass5/2、油token1/2・mass3/2 |
| 同carrier二cell | 水token1・mass5、油token1・mass3 |
| mass8のcarrier一cell | 水token1・mass5、油token1・mass3 |

`MaterialQuantity`は非負の約分済みu128分子・正のu128分母を持つ。0は0/1のみ。加算・整数倍は先に公約数で積を小さくし、必要な中間積・和・分母がu128を超える場合はエラーにする。数学的に最終結果が収まる場合でも中間値の上限で拒否することがあり、飽和や丸めで成功扱いにはしない。これは標準profileの数量型であり、独自providerへ強制しない。

## 作者の登録

起動時に`MixtureRef`の値schema、`MixtureStore`のschema、既存の`MaterialReceiptLog`のschemaを登録する。組成channelは対象layerの物質に所属させ、既定値をNONEとする。位置固定channel・別layerへの書込みを拒否する。混合APIのschema引数は、現在のstoreに登録されたschemaと一致させる。

```rust,ignore
let store = MixtureStore::new(&world, MixtureLimits {
    entries: 256, components: 16, receipts: 4096,
})?;
world.edit(|edit| edit.set_state(MixtureStore::key(), mixture_schema, &store))?;
let state = MixtureState::new(&[
    MixturePart { material: water_key, ratio: 2 },
    MixturePart { material: oil_key, ratio: 2 },
])?;
let reference = world.edit(|edit| edit.register_mixture(mixture_schema, &state))?;
```

異なる液体を接触だけで自動混合しない。作者のmix/separate ruleが条件と全入出力を選び、以下の同じ取引へ渡す。体積・cell数・出力位置も作者のrecipeで決め、空間不足なら入力を保持する。

## 混合・分離・発行・消費

`apply_mixture_changes`へlayer、`MixtureEditConfig`、理由、成分保存方針、全`MixtureCellChange`を渡す。各変更はCellCoreの期待値・出力、payload方針、組成参照の期待値・出力を持つ。全点を座標順にして、読込・block・値・重複・期待値・Worldと組成参照を検査する。

- `PreserveComponents`はConversionの理由だけを受け付け、純物質ごとの正確なtoken・massが前後で一致することを要求する。混合・分離の標準経路である。
- `RecordConversion`はSource・Sink・Conversionの意味に従い、作者が意図した発行・消費・成分変化やmass差を認める。実在質量保存をすべてのゲームrecipeへ強制しない。

cell変更と全payloadの保持・初期化は[32のreceipt](32_MATERIAL_RECEIPTS.md)へ委譲する。さらに同じ取引で、新しい組成参照と、journalキー・receipt番号・組成channel・変更前後の組成参照を持つwitnessを保存する。実際のcell receiptの変更列と不変組成を合わせれば、過去の成分量を再計算できる。

KeepChannelsは各点の物質payloadを保持し、ResetChannelsは各定義の既定値へ戻す。混合参照はその後に明示した出力へ設定する。位置固定場・他layerは初期化しない。複数入力から一出力へenergyやtimer、個体状態を移す場合は、作者が同じWorld取引で配分・移行を指定する。参照先storeの個体を暗黙に破棄せず、このAPI単独で熱や作者instanceの全移行を実装したとは扱わない。

出力参照の全最終容量を検査し、既定値へ戻る出力を先に反映する。journal・witness・channel・通常編集容量のいずれが不足しても、cell・全channel・両storeを含む外側のWorld取引全体を破棄する。callbackがエラーを無視しても部分消費しない。返すreceiptは外側のWorld::edit成功までは候補である。

## 保存・監査・費用

組成とwitnessの形式は`CHMIX001`。種類数・件数をbyte数と明示容量から確保前に検査し、正規形、重複した組成、参照scope・index、座標順、journal番号の増加順を検査する。MaterialQuantityの独立codecも分母0や未約分表現を拒否する。新規schemaとchannelはContent manifestに含まれ、既存保存の自動互換や移行を保証するものではない。

参照channelは通常の物質輸送・World保存・区域退避に従う。共有storeはWorld状態に残す。区域を退避して参照channel配列を回収しても組成は失わず、復帰後の参照が再び解決できる。

通常操作はStateStoreのrecord復元・候補複製と組成検査、全入出力の成分列挙と安定整列、journal追記と既存履歴の再保存を含む参照実装である。固定キーへの書戻しでは現recordの再decodeと定義・witnessの完全比較も行う。組成数・履歴数・変更cell数・成分種類数とcodec byte容量に依存し、定義を登録する処理をcell毎のhot kernelとして呼ばない。cell上の参照自体は固定長で、各cellへHashMapやBoxを置かない。

`MixtureStore::validate_world`はjournalを種類ごとに一度読み、過去のcell変更とwitness、成分保存方針を監査する全履歴処理である。現在の全cell・粒子・剛体・reservoir・archiveの総量を集計するAPIではない。raw編集・生成器・独自providerの自動監視も行わない。全表現の総量と履歴照合は[36](36_MATERIAL_ACCOUNTING.md)で接続する。物性providerの統合・認定とRuntimeのQ04はE05の残作業として続ける。

## 検証と作例

`tests/material_mixtures.rs`で正規化・共有参照、密度が異なる素材の端数、混合と分離、詰めたcarrier、意図した変換差、Source/Sink、scope・Content・schemaとchannel所有権、期待値・block・未読込・重複、各容量のrollback、輸送・保存・区域退避、codec切断と有理数演算を検査する。

`cargo run --locked --example material_mixtures`はmass5・3の二cellをmass4の二cellへ混合し、一cellずつに水・油token各1/2を持たせる。混合後の保存から分離を再開し、連続実行とcheckpointが一致すること、witness2件と参照配列の回収をassertする。

## 全表現の収支との接続

[36の全表現収支](36_MATERIAL_ACCOUNTING.md)は実所有量を集計し、基準と理由付き増減へ照合する。cell・混合・粒子の個別記録を総量へ二重計上しない。標準body physics、物性統合、広域移動、dirty/sleep、RuntimeのQ04完成を示すものではない。
