# E05：全表現の物質収支と保存監査

## 状態と責務

`MaterialInventoryProfile`は、resident/inactive cell、正規物質粒子、archiveと作者の保存済み表現を、純物質別のtoken/massへ換算する。`MaterialBalanceLog`は実集計した基準と理由付きの純増減を保存し、現在の全実量へ照合する。cell journalと混合witnessも実recordから検査する。この単位はE05全体、物性・provider統合、広域移動、dirty/sleep、Q04の完成判定ではない。検証と実レビューは[evidence/E05](evidence/E05/README.md)へ記録する。

Worldのblock個体は占有する実cellを一度ずつ数え、slot metadataを追加の物質へ数えない。生成要求やJobの途中pageは未公開の複製であり、現在の地形に重ねて所有量へ足さない。共有組成や過去のreceiptも物質所有者ではない。

## profileと作者の表現

profileのキー・版、layerごとの標準MixtureRef channel、providerのキー・版を正規化してhashを作る。MixtureRef channelは対象layerへ物質追従するものを明示する。どの参照が物理的な組成かという作者の宣言を、無関係な参照channelから推測しない。標準MixtureStoreの組成から正確な有理数量を求め、carrierと成分を二重計上しない。範囲外の参照、空所有物の混合参照、未知素材、未来epochは拒否する。

`MaterialInventoryProvider`はbody local grid・排除reservoir・独自格子を接続する。`collect_world`はWorld作者状態、`collect_block`はresident/archiveの完全なblock作者状態を読む。body/reservoirは実格子の非空cell等を`MaterialInventorySink::add`へ出力する。純物質はMixtureRef::NONE、cell一つはMaterialQuantity::ONEを使う。独自の分数量も指定でき、massは登録済み素材から計算する。providerキーをremapのownerとして、実データの既適用epochから規則を解釈する。

callbackは副作用を持たず、固定の成功値や隠した可変状態で総量を捏造しない。native cell・粒子の重複出力は禁止する。所有状態はWorld/blockのStateStoreに保存し、同じキー・版の実装を再接続する。block callbackへ周囲の可変World状態を渡さず、archiveから得た実状態の解釈が別tickの周辺条件で変わらないようにする。作者側の標準以外の数量形式・検証器は、既存のSystem・StateStore・保存と独自profileで統合できる。

公開API試験のbody local cellとreservoirは、座標・素材・組成・epoch・energyを持つ作者の実StateCodecを保存して数えている。固定の期待総量を返すmockではない。ただし、標準の剛体backend・回転・排除physicsの完成を意味しない。それらはE07の責務である。

## archiveとIO境界

`World::load_material_archive_inventory`を準備・保存・再開のIO境界で呼ぶ。全参照groupを一度ずつ読み、checksum、World/Content、範囲、rootの休止個体metadata、cellの素材と境界、block maskとsidecarの両方向対応、全粒子payloadの型とキー列、channel codecを検査する。1groupずつ復号し、全archiveのbyte列を同時にRAMへ集めない。

派生入力にはgroup hashと、owner・layer・carrier・組成参照・既適用epoch・量を保持する。同じ条件の所有recordは量を合算する。成分合計だけに潰さないので、後からremapしても新しいcarrierの意味へ再換算できる。group索引はhash順へ一度正規化し、chunk数に対する二乗走査を避ける。

`World::material_inventory`と`verify_material_accounting`はこの入力を使い、tick中にIOを行わない。World・Content・profile・全参照hashを照合し、未準備や不一致はエラーにする。未読込を0量と扱わない。復帰済みgroupの余分なcacheは数えない。退避で新しいhashができた場合は、IO境界で入力を更新する。再開後も保存した正規archiveから再構築する。cache自体を正規状態として保存する必要はない。

## 基準・変更・照合

1. ContentへMaterialBalanceLogのschemaを登録する。
2. 正しい初期状態とarchive入力から`initialize_material_accounting`で基準を保存する。既存キーと標準MixtureStoreキーは初期化先にしない。
3. 所有権だけの移動は`edit_material_accounted`のreason=Noneで包み、変更前後の純物質別量の完全一致を要求する。
4. 発行・消費・変換はSome(reason)で包む。変更前後の実集計の差と理由を同じWorld候補に保存する。
5. 保存・再開、標準経路と独自providerの比較等で`verify_material_accounting`を呼ぶ。

receiptは連続番号、tick、material epoch、作者ruleのキー・0以外の版・区分と、純物質別の純増減を持つ。Sourceは純増だけ、Sinkは純減だけを許す。Conversionは同素材の共通量を相殺した差を保存する。grossな入力・出力やcell座標は[32のcell receipt](32_MATERIAL_RECEIPTS.md)と[34の混合witness](34_MATERIAL_MIXTURES.md)が別に保持する。成分を保存する混合・分離は全体の増減receiptを増やさず、個別の変換履歴を検証する。基準＋純増−純減と現在の全実量を照合し、履歴自体を物質へ数えない。

通常のWorld::editを全て自動監視する機構ではない。raw編集による未記録増減は照合で検出し、次の監査付き取引に紛れ込ませない。未記録の初期世界を正しい初期化時点で基準に含めることと、開始済みの基準を巻き戻すことを区別する。

汎用typed/encoded書戻しでは、基準・既存receipt・所属・キーの完全一致を要求し、容量変更だけを認める。新規baselineと追記は専用入口に限る。古いsnapshotや別分岐の追加履歴を持ち込まない。完全checkpointの復元は、物質と台帳を同時に復元する。

## 監査付きの世代保存と読込

`MaterialAccountingSaveProfile`へWorldSaveProfile、inventory profile、journalキー、archive読込上限を渡す。

- `save_material_accounted`は全参照archiveと全実量・履歴を照合してからSaveRepositoryの保存を始める。不一致なら既存世代を維持する。
- `load_material_accounted`は完全な保存候補を読み、全表現の実量を検証してからWorldを返す。不一致を無視したWorldを公開しない。

通常のcheckpoint/save/loadを監査の代用とは呼ばない。`WorldAccess::edit_material_accounted`で作者がMaterials barrierの単一原子取引を選べることは[40](40_MATERIAL_RUNTIME.md)へ統合し、RuntimeのQ04回帰でも確認した。監査は全Runtime保存へ暗黙に自動適用されず、対象台帳・profileと物質を変更するSystemが明示的に選ぶ。

## 数値・容量・費用

標準profileは約分済みu128有理数を使う。差は負を拒否し、大小比較は商と剰余を使って交差積のoverflowを避ける。加減乗の中間値が上限を超えたら、丸め・飽和で成功扱いにしない。

MaterialInventoryLimits.atomsは走査中の所有recordとcacheの作業件数、componentsは純物質種類数の上限である。archiveの圧縮後の件数だけでなく、一groupを走査する一時recordも予算へ含める。archive一groupのbyte上限、World/StateStore/schema、receipt件数も別々に残る。どれかが不足すれば取引は全て戻り、入力・先行編集・台帳を保持する。

全実量を読む参照経路であり、resident全cell・粒子・作者状態の走査と集計に費用がかかる。archive準備はgroupごとの復号と最大一groupの一時状態、合算したcache、hash索引のRAMを使う。監査付き変更は変更前後の走査と、基準・履歴の検証・再保存を行う。履歴数に無関係な定数時間appendとは説明しない。変更をphase等へまとめ、容量を設定する。履歴を自動削除して速度を保つ方式にはしない。

report形式はCHINV001、log形式はCHBAL001。WorldのCHWORLD2、regionのCHREGN02、CHMIX001を変更しない。追加schemaはContent manifestへ含まれるため、異なる既存Contentの自動互換は仮定しない。

## 実行

[作例](../examples/material_accounting.rs)は理由付きSource、cell→粒子→archive、監査付き世代保存・読込、復帰・着地を行い、token=1・mass=5とreceipt=1の一致を検査する。出力はtarget/material_accounting_exampleへ保存する。

```sh
cargo test --locked --test material_accounting
cargo run --locked --example material_accounting
```

対象試験は全表現、純物質と正確な混合成分、複数chunk groupの一度だけの集計、hash更新後も不正な素材を拒否するarchive検証、古いcache、remap、receipt/witness、未記録増減、容量とschema失敗、履歴巻戻し、世代の公開・読込拒否を扱う。詳細な検証件数と実レビューはevidenceへ記録する。

### 空素材への変換とarchive channel境界

`engine:terrain` と `engine:blocks` の素材remapで所有cellが `engine:air` へ変化したとき、cell本体だけでなく同じlayerの物質追従channelも既定値へ戻す。混合参照やenergyが空cellへ残って幽霊の所有物にならないよう、Worldの同一候補取引内で処理する。archiveの未適用epochを論理解決してcarrierが空になった場合、旧組成refを追加の物質として数えない。粒子の空素材remapは従来どおり登録時に拒否する。

区域に属するchannelのcodecは、チェックサム一致だけでは充分でない。格子の各非既定値が対象regionへ完全に収まることを `RegionLoadTask::load` で検証し、別regionの値や粗い格子が跨ぐ値は復帰前に拒否する。これにより、検証時に隣接値を読み飛ばして後のmergeで越境書込みすることを防ぐ。
