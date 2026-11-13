//! 画像と音のファイルの読み込み (仕様 15)。PNG・WAV・OGG を読み、ゲームが `SpriteAsset` や `Sound` として使えるようにする。

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use charon_audio::Sound;
use charon_render::SpriteFilter;

use crate::animation::{Clips, SpriteAsset};

/// 1 ファイルの大きさの上限 (バイト)。
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// 画像の一辺の上限 (画素)。
pub const MAX_IMAGE_SIDE: u32 = 4096;
/// 読み込む音の長さの上限 (サンプル数、モノラル換算)。約 4 分 (44.1 kHz)。
pub const MAX_SOUND_SAMPLES: usize = 44_100 * 240;

/// 読み込みの失敗。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// ファイルを読めない、大きすぎる、名前が不正。
    File(String),
    /// 画像として読めない・大きすぎる・対応しない形式。
    Image(String),
    /// 音として読めない・対応しない形式。
    Sound(String),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(m) => write!(f, "ファイル: {m}"),
            Self::Image(m) => write!(f, "画像: {m}"),
            Self::Sound(m) => write!(f, "音: {m}"),
        }
    }
}

impl std::error::Error for AssetError {}

/// 読み込んだ画像 (RGBA8、ストレートアルファ)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

/// PNG を RGBA8 にする。グレー・RGB・パレット・16 ビットも読む。大きすぎる画像は読まずに断る。
pub fn decode_png(bytes: &[u8]) -> Result<Image, AssetError> {
    let bad = |e: png::DecodingError| AssetError::Image(e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(bad)?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(AssetError::Image(format!("大きさが範囲外です: {width}x{height}")));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| AssetError::Image("バッファの大きさを決められません".into()))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(bad)?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => {
            buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Indexed => {
            return Err(AssetError::Image("パレットが展開されませんでした".into()));
        }
    };
    if rgba.len() != width as usize * height as usize * 4 {
        return Err(AssetError::Image("画素の数が合いません".into()));
    }
    // 上の検査で `MAX_IMAGE_SIDE` (4096) 以下なので u16 に収まる。
    Ok(Image { width: width as u16, height: height as u16, rgba })
}

/// 画像をスプライトとして使うときの指定。
#[derive(Clone, Debug)]
pub struct SpriteSpec {
    /// 1 コマの大きさ (画素)。`None` なら画像全体 (アニメーションなし)。
    pub frame_size: Option<[u16; 2]>,
    /// 基準点 (コマの中の画素)。`None` なら足元の中央。
    pub pivot: Option<[f32; 2]>,
    /// 画像の 1 画素あたりのセル数。
    pub scale: f32,
    pub filter: SpriteFilter,
    pub clips: Clips,
}

impl Default for SpriteSpec {
    fn default() -> Self {
        Self {
            frame_size: None,
            pivot: None,
            scale: 1.0,
            filter: SpriteFilter::Nearest,
            clips: Clips::STILL,
        }
    }
}

/// PNG のバイト列からスプライトを作る。コマの大きさが画像に収まらない・0 のときは断る。
pub fn sprite_from_png(bytes: &[u8], spec: &SpriteSpec) -> Result<SpriteAsset, AssetError> {
    let image = decode_png(bytes)?;
    let frame = spec.frame_size.unwrap_or([image.width, image.height]);
    if frame[0] == 0 || frame[1] == 0 || frame[0] > image.width || frame[1] > image.height {
        return Err(AssetError::Image(format!(
            "コマの大きさ {frame:?} が画像 {}x{} に収まりません",
            image.width, image.height
        )));
    }
    if !spec.scale.is_finite() || spec.scale <= 0.0 {
        return Err(AssetError::Image("scale は正の有限な値にしてください".into()));
    }
    let pivot = spec.pivot.unwrap_or([f32::from(frame[0]) / 2.0, f32::from(frame[1])]);
    Ok(SpriteAsset {
        width: image.width,
        height: image.height,
        rgba: Arc::from(image.rgba),
        frame_size: frame,
        pivot,
        scale: spec.scale,
        filter: spec.filter,
        clips: spec.clips,
    })
}

/// OGG Vorbis を読む。ステレオは平均してモノラルにする (`Sound` はモノラル)。
pub fn decode_ogg(bytes: &[u8]) -> Result<Sound, AssetError> {
    let bad = |e: lewton::VorbisError| AssetError::Sound(e.to_string());
    let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(bytes)).map_err(bad)?;
    let rate = reader.ident_hdr.audio_sample_rate;
    let channels = usize::from(reader.ident_hdr.audio_channels).max(1);
    let mut samples: Vec<f32> = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(bad)? {
        for frame in packet.chunks_exact(channels) {
            let sum: f32 = frame.iter().map(|&s| f32::from(s) / 32768.0).sum();
            samples.push(sum / channels as f32);
        }
        if samples.len() > MAX_SOUND_SAMPLES {
            return Err(AssetError::Sound("長すぎます".into()));
        }
    }
    if samples.is_empty() {
        return Err(AssetError::Sound("音が空です".into()));
    }
    Ok(Sound::new(rate, samples))
}

/// 先頭の印で WAV か OGG かを決めて読む。
pub fn decode_sound(bytes: &[u8]) -> Result<Sound, AssetError> {
    if bytes.starts_with(b"OggS") {
        decode_ogg(bytes)
    } else {
        Sound::from_wav(bytes).map_err(|e| AssetError::Sound(format!("{e:?}")))
    }
}

/// 名前 (`dir/file.png`) が、置き場の外へ出ない相対パスか。
fn safe_name(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    let ok = !name.is_empty()
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && !name.contains('\\');
    ok.then(|| path.to_path_buf())
}

/// 画像と音の置き場。後から足した置き場が先に探される (ゲームの MOD がエンジンの既定を上書きできる)。
/// 読み込んだものは名前で覚える。
#[derive(Clone, Debug, Default)]
pub struct Assets {
    roots: Vec<PathBuf>,
    sprites: BTreeMap<String, SpriteAsset>,
    sounds: BTreeMap<String, Sound>,
}

impl Assets {
    pub fn new() -> Self {
        Self::default()
    }

    /// 置き場を足す。最後に足したものが最優先。
    pub fn add_root(&mut self, root: impl Into<PathBuf>) {
        self.roots.push(root.into());
    }

    /// 名前のファイルのバイト列。置き場の外へ出る名前、大きすぎるファイルは断る。
    pub fn read(&self, name: &str) -> Result<Vec<u8>, AssetError> {
        let rel =
            safe_name(name).ok_or_else(|| AssetError::File(format!("名前が不正です: {name:?}")))?;
        for root in self.roots.iter().rev() {
            let path = root.join(&rel);
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_FILE_BYTES {
                return Err(AssetError::File(format!(
                    "{name} が大きすぎます ({} バイト)",
                    meta.len()
                )));
            }
            return std::fs::read(&path).map_err(|e| AssetError::File(format!("{name}: {e}")));
        }
        Err(AssetError::File(format!("{name} が見つかりません")))
    }

    /// 画像を読んでスプライトにする (同じ名前は覚えたものを返す)。
    pub fn sprite(&mut self, name: &str, spec: &SpriteSpec) -> Result<SpriteAsset, AssetError> {
        if let Some(found) = self.sprites.get(name) {
            return Ok(found.clone());
        }
        let asset = sprite_from_png(&self.read(name)?, spec)?;
        self.sprites.insert(name.to_string(), asset.clone());
        Ok(asset)
    }

    /// 音を読む (同じ名前は覚えたものを返す)。
    pub fn sound(&mut self, name: &str) -> Result<Sound, AssetError> {
        if let Some(found) = self.sounds.get(name) {
            return Ok(found.clone());
        }
        let sound = decode_sound(&self.read(name)?)?;
        self.sounds.insert(name.to_string(), sound.clone());
        Ok(sound)
    }

    /// 覚えたものを忘れる (ファイルを差し替えて読み直すとき)。
    pub fn clear_cache(&mut self) {
        self.sprites.clear();
        self.sounds.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(width: u32, height: u32, color: png::ColorType, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn png_color_types_become_rgba() {
        let rgba = encode(2, 1, png::ColorType::Rgba, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(decode_png(&rgba).unwrap().rgba, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let rgb = encode(2, 1, png::ColorType::Rgb, &[1, 2, 3, 4, 5, 6]);
        assert_eq!(decode_png(&rgb).unwrap().rgba, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        let gray = encode(2, 1, png::ColorType::Grayscale, &[9, 200]);
        assert_eq!(decode_png(&gray).unwrap().rgba, vec![9, 9, 9, 255, 200, 200, 200, 255]);
        let ga = encode(1, 1, png::ColorType::GrayscaleAlpha, &[50, 60]);
        assert_eq!(decode_png(&ga).unwrap().rgba, vec![50, 50, 50, 60]);
    }

    #[test]
    fn bad_and_huge_images_are_rejected() {
        assert!(matches!(decode_png(b"not a png"), Err(AssetError::Image(_))));
        assert!(matches!(decode_png(&[]), Err(AssetError::Image(_))));
        // 大きすぎる画像は、画素を作る前に断る (ヘッダーだけの PNG)。
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, MAX_IMAGE_SIDE + 1, 1);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::One);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![0; (MAX_IMAGE_SIDE as usize + 1).div_ceil(8)]).unwrap();
        writer.finish().unwrap();
        assert!(matches!(decode_png(&out), Err(AssetError::Image(_))));
        // 切れた PNG。
        let cut = encode(4, 4, png::ColorType::Rgba, &[0; 64]);
        assert!(decode_png(&cut[..cut.len() / 2]).is_err());
    }

    #[test]
    fn sprites_take_frame_size_pivot_and_scale() {
        let bytes = encode(8, 4, png::ColorType::Rgba, &[255; 8 * 4 * 4]);
        let sheet = sprite_from_png(
            &bytes,
            &SpriteSpec { frame_size: Some([4, 4]), scale: 0.5, ..Default::default() },
        )
        .unwrap();
        assert_eq!((sheet.width, sheet.height, sheet.frame_size), (8, 4, [4, 4]));
        assert_eq!(sheet.pivot, [2.0, 4.0], "既定は足元の中央");
        assert_eq!(sheet.scale, 0.5);
        for spec in [
            SpriteSpec { frame_size: Some([0, 4]), ..Default::default() },
            SpriteSpec { frame_size: Some([9, 4]), ..Default::default() },
            SpriteSpec { scale: 0.0, ..Default::default() },
            SpriteSpec { scale: f32::NAN, ..Default::default() },
        ] {
            assert!(sprite_from_png(&bytes, &spec).is_err(), "{spec:?}");
        }
    }

    #[test]
    fn sounds_are_picked_by_their_header() {
        // 16 ビットのモノラル WAV を作る。
        let samples: Vec<i16> = (0..100).map(|i| (i * 100) as i16).collect();
        let mut wav = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let sound = decode_sound(&wav).unwrap();
        assert_eq!(sound.rate, 8000);
        assert_eq!(sound.samples.len(), 100);
        // OGG の印があって中身が壊れていれば、OGG として断る。
        assert!(matches!(decode_sound(b"OggS\0\0\0\0garbage"), Err(AssetError::Sound(_))));
        assert!(matches!(decode_sound(b"RIFFxxxx"), Err(AssetError::Sound(_))));
        assert!(matches!(decode_sound(&[]), Err(AssetError::Sound(_))));
    }

    #[test]
    fn names_cannot_escape_the_roots() {
        let assets = Assets::new();
        for name in ["", "../x.png", "/etc/passwd", "a/../../x", "C:/x.png", "a\\b.png"] {
            assert!(matches!(assets.read(name), Err(AssetError::File(_))), "{name:?}");
        }
        assert!(safe_name("sprites/hero.png").is_some());
    }

    #[test]
    fn later_roots_override_earlier_ones_and_results_are_cached() {
        let base = std::env::temp_dir().join(format!("charon-assets-{}", std::process::id()));
        let (engine, game) = (base.join("engine"), base.join("game"));
        std::fs::create_dir_all(&engine).unwrap();
        std::fs::create_dir_all(&game).unwrap();
        let red = encode(1, 1, png::ColorType::Rgba, &[255, 0, 0, 255]);
        let blue = encode(1, 1, png::ColorType::Rgba, &[0, 0, 255, 255]);
        std::fs::write(engine.join("a.png"), &red).unwrap();
        std::fs::write(engine.join("only.png"), &red).unwrap();
        std::fs::write(game.join("a.png"), &blue).unwrap();

        let mut assets = Assets::new();
        assets.add_root(&engine);
        assets.add_root(&game);
        let spec = SpriteSpec::default();
        assert_eq!(
            &assets.sprite("a.png", &spec).unwrap().rgba[..4],
            &[0, 0, 255, 255],
            "後の置き場が優先"
        );
        assert_eq!(&assets.sprite("only.png", &spec).unwrap().rgba[..4], &[255, 0, 0, 255]);
        // 消しても、覚えたものを返す。消したあと忘れると見つからない。
        std::fs::remove_file(game.join("a.png")).unwrap();
        assert!(assets.sprite("a.png", &spec).is_ok());
        assets.clear_cache();
        assert_eq!(
            &assets.sprite("a.png", &spec).unwrap().rgba[..4],
            &[255, 0, 0, 255],
            "下の置き場にもどる"
        );
        assert!(matches!(assets.sprite("missing.png", &spec), Err(AssetError::File(_))));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
