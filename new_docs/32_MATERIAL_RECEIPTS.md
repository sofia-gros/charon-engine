# E05：cell変更と理由付きreceipt

## 状態

`WorldEdit::apply_material_changes`は、cellの発行・消費・変換と、その理由・量の記録を同じWorld取引で確定する。E05全体・Q04の完了を示さない。

この単位はcell表現の変更に対するreceiptである。rawな`set_cell`、生成器、全providerの変更を自動監視しない。世界cell・粒子・剛体local cell・退避領域・reservoirを集計済みの全World台帳ではない。混合・粒子などの表現間bridgeと全体収支の検査は後続で接続する。

## journalの所属と保存

`MaterialReceiptLog`の型schemaをContentへ登録し、`new(&world, journal_key, capacity)`で作った空logをWorld StateStoreへ保存する。`MaterialReceiptJournal`はそのrecordキーとschemaを指定する。

journalはWorld ID・Content fingerprint・recordキーへ所属する。異なるWorld・Content・journalのlogを変換に流用しない。新規schema登録はContent manifestを変えるため、異なるContentの既存保存との互換や自動移行を仮定しない。World snapshot・区域archiveの構造は変更しない。

receiptはjournal内で0から増えるsequence、World tick、素材remap epoch、対象layer、理由、座標順の変更前後、payload方針、素材別の消費・発行量を持つ。World・journalキー・sequenceの組で識別する。過去の履歴を現在の素材へ書き換えない。

`StateCodec`は形式・件数・順序・値域を検査する。`validate_world(&world)`はさらに所属、参照するlayer・素材、過去の変更列から再計算した量と保存量の一致を照合する。変換APIは追加前にこの照合を実行する。外部recordを監査用途へ読み込む場合も明示的に照合する。現在の地形が過去の変更前後に一致することを要求する検査ではない。

## 変更の入力

各`MaterialCellChange`へWorld座標、flags・variationを含む期待CellCore、出力CellCore、payload方針を渡す。全点の読込・World境界・期待値・値域・重複を検査する。block個体は暗黙に分解・消費せず拒否する。

入力順は結果へ持ち込まず、座標順に正規化する。期待値が一つでも異なる場合は先行編集を含め取引を破棄する。

| payload方針 | 意味 |
| --- | --- |
| `KeepChannels` | そのlayerの物質channelの現在値を維持。作者の移行処理を同じeditで準備できる |
| `ResetChannels` | そのlayerの全物質channelを、各定義の既定値へ戻す |

位置固定場・別layerのchannelには触れない。ResetChannelsは参照先storeの個体まで自動破棄するものではない。個体の寿命・独自状態の移行を作者が明示し、必要な処理を同じWorld取引へ含める。

## 理由と量

`MaterialChangeReason`は作者ruleの安定キー・0以外の版・区分を持つ。キーは保存される識別子であり、このAPIがrecipeを自動実行・認定するものではない。

- Source：非空の入力がなく、非空の出力がある。
- Sink：非空の入力があり、非空の出力がない。
- Conversion：非空の入力・出力がある。

空cellをtokenに数えず、変更前後を素材別・安定キー順へ集計する。token数はu64、massはtoken数と登録済みの正の`mass_per_cell`の積をu128で保存する。codecではlittle-endianの固定16byteでmassを扱う。集計のoverflowは拒否する。

Conversionへ現実のmass保存を強制しない。作者が意図した増減を理由と入出力で検査可能にする。例えばmass5のcell二つをmass10のcell一つへ変換する場合も、mass5をmass10へ発行を伴って変換する場合も、実際の消費・発行量を記録する。熱energyや外部itemの収支を自動計算するものではない。

## 取引の境界と容量

cell書込数と、ResetChannelsで触る全物質channelの書込数を先に予約する。journalの件数上限とschema・StateStoreのbyte上限を検査してから地形を変更する。channel初期化は既定値への変更で、不要な新規配列を確保しない。

外側の`World::edit`が成功したときだけreceiptと地形がともに確定する。`apply_material_changes`の戻り値はcallback内部ではまだ候補であり、後続処理で取引が失敗すれば無効である。外部へ値を持ち出しても、失敗した取引の確定証明にはならない。

journalは上限付きで履歴を自動削除しない。満杯なら変換を止める。`set_capacity`で件数を増やして保存し直せるが、schema・StateStoreのbyte上限は独立して残る。同じキーへ空logを上書きすると履歴を失い番号も再使用するため、監査上の継続や外部永続化の代用にしない。外部へ安全に退避して整理するjournalの機構はこの単位に含まない。

現在の実装はStateStoreからlogを復元し、履歴を照合して全recordを再保存する。費用は履歴数・変更数に依存し、毎回一定時間のappendとは説明しない。大きい変更は一つのreceiptへまとめ、件数・byte数を作品の予算として設定する。StateStore・Worldの候補の複製費用も別に発生する。

## 実行と検証

[作例](../examples/material_receipts.rs)はSource・Conversion・Sinkを同じjournalへ保存し、空になった地形と3件のreceiptを一緒に復元する。

```sh
cargo run --locked --example material_receipts
cargo test --locked --test material_receipts
```

対象試験は発行・消費・意図したmass差、複数cell変換、payload移行、期待値失効、block拒否、編集・保存容量のrollback、所属、codec、履歴量の不一致、区域退避・復帰での履歴保持を扱う。全体の検証と未完了項目は[evidence/E05](evidence/E05/README.md)へ記録する。
