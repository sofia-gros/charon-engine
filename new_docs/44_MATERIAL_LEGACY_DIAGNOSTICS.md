# E05：PR #11〜#13 の公開診断・一様力学の互換継承

## 互換を残す理由

PR #11〜#13はPR #10から分岐していた。PR #14以降の全表現収支・物性Providerは保存則と原子性を大きく拡張したが、**以前の公開APIがすべて包含されたわけではない**。旧診断はセル/粒子、resident/archive別の量を返し、全表現台帳は純成分の合計と増減を照合する。どちらか一方では作者向けデバッグ用途を代替できない。

この実装では旧機能を派生読取専用APIとして復元し、既存のWorld save、material ledger、receipt、Content fingerprintを一切変更しない。既存の全表現収支は唯一の保存則監査として維持し、簡易診断を保存則の成功判定に流用しない。

## 実装する診断

- World::resident_material_inventory：全layerの読込済み（inactiveを含む）carrierのcell/正規粒子別tokenと名目mass。未配置素材もゼロ行を含む。archiveは未集計件数として明示。
- World::material_inventory_with_archives：外部ArchiveStorageから全groupを実際に読み、material remap epoch、保存metadata、容量を検査してresident/archivedのcell/粒子別carrier token・名目massを報告。外部archiveをゼロと仮定しない。
- World::resident_mixture_inventory：指定layerのMixtureRef追従channelから共有組成を正確な有理数の純成分へ展開し、resident cell/粒子それぞれのtoken・massを返す。退避分は未計上と明示。

これらは**読み取り専用の診断**で、Q04のmaterial_inventory(profile,archives)、verify_material_accountingや理由付き台帳の代替ではない。すべての可変所有権の境界を既存Worldと同じ型付きAPIで検査する。

## 旧一様力学の明示的な選択

UniformMaterialForcesは旧PR #13の一様重力、環境非依存gas浮力、力加算後の抗力、粉体水平固定量摩擦を保持する。新しいStandardMaterialForcesは隣接物質の密度や接触を使う異なる式のため、**標準Providerの意味を変更せず、互換Providerを明示選択する**。

旧MaterialForceProviderのevaluate契約と新MaterialForceProviderのintegrate契約は同名では公開できない。互換用にLegacyMaterialForceProvider、LegacyMaterialForceInput、LegacyMaterialForceReportを定義し、WorldEdit::apply_material_forcesを維持する。作者が旧traitを直接importしていた場合だけ名前の移行が必要である。旧式の速度・端数更新と失敗時のWorldEdit poisonは維持し、Providerの識別子/状態は作者のsim profileとStateStoreで保存する。

加えてUniformMaterialForcesは新MaterialForceProviderも実装し、明示的にこれを選択した場合のみ旧式の速度計算を統合Materials phaseで使用できる。新標準式を自動的に上書きすることはない。係数の変更は作者が正規profileに保存し、旧一様forceの実装キーとversionも固定する。

## 確認すべき証拠・旧PRの扱い

旧tests/material_inventory.rsを復元し、inactive/多layer、地形輸送、cellと粒子の移譲、退避復帰、epoch remap、receiptと保存復元の全6件を検査する。旧一様gasのQ16速度、粉体摩擦、係数拒否、失敗握り潰しrollbackと端数維持の公開回帰も追加する。さらに旧tests/material_mixtures.rsのlayer別成分試験と全体物質収支を照合する。

本互換単位の対象試験と独立コードレビューが終わるまで、**PR #11〜#13を完全置換済みとは呼ばない**。採用するPRだけを最後に確定し、E05最後のPR本文へ「E05はPR #4、#5、…、#（最終） の順でマージしてください」と漏れのない番号・依存・未採用PRと理由を掲載する。
