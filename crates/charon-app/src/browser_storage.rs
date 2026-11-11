//! Web 専用アダプター。ファイルを使わず、同一 origin の永続ストレージへ保存する。

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use charon_world::web_save::WebSnapshot;
use web_sys::Storage;

const KEY: &str = "charon.world.v1";

fn storage() -> Result<Storage, String> {
    web_sys::window()
        .ok_or("ブラウザーの window がありません".to_owned())?
        .local_storage()
        .map_err(|err| format!("永続ストレージを開けません: {err:?}"))?
        .ok_or("永続ストレージが利用できません".to_owned())
}

pub(crate) fn load() -> Result<Option<WebSnapshot>, String> {
    let Some(data) =
        storage()?.get_item(KEY).map_err(|err| format!("保存データを読めません: {err:?}"))?
    else {
        return Ok(None);
    };
    let bytes = STANDARD.decode(data).map_err(|err| format!("保存データの復号に失敗: {err}"))?;
    WebSnapshot::decode(&bytes).map(Some).map_err(|err| err.to_string())
}

pub(crate) fn save(snapshot: &WebSnapshot) -> Result<(), String> {
    let bytes = snapshot.encode().map_err(|err| err.to_string())?;
    let encoded = STANDARD.encode(bytes);
    storage()?
        .set_item(KEY, &encoded)
        .map_err(|err| format!("未保存: 永続ストレージに書けません: {err:?}"))
}
