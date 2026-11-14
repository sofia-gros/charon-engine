//! 仕様 27 §4: エンジンの固定 UI 文字列と、日本語フォントの登録。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use serde::Deserialize;

use crate::settings::Language;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UiStrings {
    en: BTreeMap<String, String>,
    ja: BTreeMap<String, String>,
}

impl UiStrings {
    pub fn from_ron(text: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(text)
    }

    /// 日本語が欠けたら英語、英語も欠けたらキーをそのまま出す。
    pub fn text<'a>(&'a self, language: Language, key: &'a str) -> &'a str {
        if language == Language::Japanese
            && let Some(value) = self.ja.get(key).filter(|v| !v.is_empty())
        {
            return value;
        }
        self.en.get(key).filter(|v| !v.is_empty()).map_or(key, String::as_str)
    }
}

pub fn ui_strings() -> &'static UiStrings {
    static STRINGS: OnceLock<UiStrings> = OnceLock::new();
    STRINGS.get_or_init(|| {
        UiStrings::from_ron(include_str!("../../../assets/strings.ron")).unwrap_or_default()
    })
}

/// 既存の英語字体を先に使い、日本語だけ同梱の OFL フォントで補う。
pub fn install_japanese_font(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let name = "noto-sans-jp".to_owned();
    fonts.font_data.insert(
        name.clone(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../../../assets/fonts/noto-sans-jp/NotoSansJP.ttf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push(name.clone());
    }
    context.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_language_falls_back_for_missing_strings() {
        let strings = UiStrings::from_ron(include_str!("../../../assets/strings.ron")).unwrap();
        assert_eq!(strings.text(Language::English, "menu.title"), "Main Menu");
        assert_eq!(strings.text(Language::Japanese, "menu.title"), "メインメニュー");
        let sparse = UiStrings::from_ron(
            r#"(en: {"known": "English", "empty": ""}, ja: {"known": "", "empty": "日本語"})"#,
        )
        .unwrap();
        assert_eq!(sparse.text(Language::Japanese, "known"), "English");
        assert_eq!(sparse.text(Language::Japanese, "empty"), "日本語");
        assert_eq!(sparse.text(Language::Japanese, "unknown"), "unknown");
        assert!(UiStrings::from_ron("broken").is_err());
        let old = crate::settings::Settings::default().to_ron().unwrap();
        let old = old.replace("language: English,", "");
        assert_eq!(crate::settings::Settings::from_ron(&old).unwrap().language, Language::English);
        let settings =
            crate::settings::Settings { language: Language::Japanese, ..Default::default() };
        assert_eq!(
            crate::settings::Settings::from_ron(&settings.to_ron().unwrap()).unwrap().language,
            Language::Japanese
        );
        let mod_name = "modded:item:鉄の道具";
        assert_eq!(strings.text(Language::Japanese, mod_name), mod_name);
    }

    #[test]
    fn japanese_glyphs_render_in_the_ui() {
        let context = egui::Context::default();
        install_japanese_font(&context);
        let mut output = context.run_pass(egui::RawInput::default(), |ctx| {
            let font = egui::FontId::proportional(18.0);
            let strings = ui_strings();
            for (key, text) in &strings.ja {
                assert!(ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, text)), "{key}: {text}");
                let galley = ctx.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
                });
                assert!(galley.size().x > 0.0 && galley.size().y > 0.0);
            }
            assert!(ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, "Main Menu 123")));
        });
        output.textures_delta.clear();
    }
}
