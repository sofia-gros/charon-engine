//! 上限付きlittle-endian codec。Rustのメモリ配置は保存形式に含めません。
//!
//! 読取側は入力全体と各recordの長さを確保前に検証します。
//! 書込側も追加ごとに上限と確保を確認するため、巨大な長さを信頼しません。

use super::error::{EngineError, EngineResult, ErrorCode, allocation};

/// 一つのbufferへ書き込む最大byte数を固定したwriterです。
pub struct Encoder {
    bytes: Vec<u8>,
    limit: usize,
    measured: Option<usize>,
}

impl Encoder {
    /// 上限を設定します。ゼロ上限は空recordの表現に使用できます。
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            measured: None,
        }
    }

    /// 保存bufferを確保せず、同じcodec経路で書込量だけを数えます。
    pub(crate) fn measuring() -> Self {
        Self {
            bytes: Vec::new(),
            limit: usize::MAX,
            measured: Some(0),
        }
    }

    pub(crate) fn is_measuring(&self) -> bool {
        self.measured.is_some()
    }

    pub(crate) fn written_len(&self) -> usize {
        self.measured.unwrap_or(self.bytes.len())
    }

    /// 別codecが数えたpayloadへ、通常のbytesと同じ長さheaderを加えます。
    pub(crate) fn measure_bytes(&mut self, len: usize) -> EngineResult<()> {
        if !self.is_measuring() {
            return Err(EngineError::new(
                ErrorCode::InvalidValue,
                "計数専用writerが必要",
            ));
        }
        u32::try_from(len)
            .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "recordの長さ"))?;
        let additional = len
            .checked_add(4)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "recordの長さ"))?;
        let end = self
            .written_len()
            .checked_add(additional)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "保存byte数"))?;
        self.measured = Some(end);
        Ok(())
    }

    /// 生byte列を追加します。失敗した追加はbufferを変更しません。
    pub fn raw(&mut self, bytes: &[u8]) -> EngineResult<()> {
        if let Some(count) = self.measured {
            self.measured = Some(count.checked_add(bytes.len()).ok_or_else(|| {
                EngineError::new(ErrorCode::InvalidValue, "保存byte数のoverflow")
            })?);
            return Ok(());
        }
        let end = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "保存byte数のoverflow"))?;
        if end > self.limit {
            return Err(EngineError::capacity(
                "保存byte数",
                end as u64,
                self.limit as u64,
            ));
        }
        self.bytes.try_reserve(bytes.len()).map_err(allocation)?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    /// 固定幅の符号なし整数を書きます。
    pub fn u8(&mut self, value: u8) -> EngineResult<()> {
        self.raw(&[value])
    }
    /// 16bit整数をlittle-endianで書きます。
    pub fn u16(&mut self, value: u16) -> EngineResult<()> {
        self.raw(&value.to_le_bytes())
    }
    /// 32bit整数をlittle-endianで書きます。
    pub fn u32(&mut self, value: u32) -> EngineResult<()> {
        self.raw(&value.to_le_bytes())
    }
    /// 64bit整数をlittle-endianで書きます。
    pub fn u64(&mut self, value: u64) -> EngineResult<()> {
        self.raw(&value.to_le_bytes())
    }
    /// 符号付き座標をlittle-endianで書きます。
    pub fn i32(&mut self, value: i32) -> EngineResult<()> {
        self.raw(&value.to_le_bytes())
    }

    /// 熱energy等の符号付き64bit整数を書きます。
    pub fn i64(&mut self, value: i64) -> EngineResult<()> {
        self.raw(&value.to_le_bytes())
    }

    /// u32のbyte長に続けてpayloadを書きます。
    ///
    /// 長さだけを書いてから容量不足になることを避けるため、
    /// headerとpayloadをまとめて予約してから変更します。
    pub fn bytes(&mut self, value: &[u8]) -> EngineResult<()> {
        if self.is_measuring() {
            return self.measure_bytes(value.len());
        }
        let len = u32::try_from(value.len())
            .map_err(|_| EngineError::new(ErrorCode::InvalidValue, "recordの長さ"))?;
        let additional = value
            .len()
            .checked_add(4)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "recordの長さ"))?;
        let end = self
            .bytes
            .len()
            .checked_add(additional)
            .ok_or_else(|| EngineError::new(ErrorCode::InvalidValue, "保存byte数"))?;
        if end > self.limit {
            return Err(EngineError::capacity(
                "保存byte数",
                end as u64,
                self.limit as u64,
            ));
        }
        self.bytes.try_reserve(additional).map_err(allocation)?;
        self.bytes.extend_from_slice(&len.to_le_bytes());
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    /// 完成bufferの所有権を呼出側へ渡します。
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// 借用した入力を範囲確認しながら読むreaderです。
pub struct Decoder<'a> {
    input: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    /// 入力全体の上限を検証します。この段階では確保しません。
    pub fn new(input: &'a [u8], limit: usize) -> EngineResult<Self> {
        if input.len() > limit {
            return Err(EngineError::capacity(
                "読込byte数",
                input.len() as u64,
                limit as u64,
            ));
        }
        Ok(Self { input, cursor: 0 })
    }

    /// 残っているbyte数を返します。
    pub fn remaining(&self) -> usize {
        self.input.len() - self.cursor
    }

    /// 指定byteを借用します。切れた入力ではcursorを進めません。
    pub fn raw(&mut self, count: usize) -> EngineResult<&'a [u8]> {
        let end = self.cursor.checked_add(count).ok_or_else(invalid)?;
        let value = self.input.get(self.cursor..end).ok_or_else(invalid)?;
        self.cursor = end;
        Ok(value)
    }

    /// 8bit整数を読みます。
    pub fn u8(&mut self) -> EngineResult<u8> {
        Ok(self.raw(1)?[0])
    }
    /// 16bit整数を読みます。
    pub fn u16(&mut self) -> EngineResult<u16> {
        Ok(u16::from_le_bytes(
            self.raw(2)?.try_into().map_err(|_| invalid())?,
        ))
    }
    /// 32bit整数を読みます。
    pub fn u32(&mut self) -> EngineResult<u32> {
        Ok(u32::from_le_bytes(
            self.raw(4)?.try_into().map_err(|_| invalid())?,
        ))
    }
    /// 64bit整数を読みます。
    pub fn u64(&mut self) -> EngineResult<u64> {
        Ok(u64::from_le_bytes(
            self.raw(8)?.try_into().map_err(|_| invalid())?,
        ))
    }
    /// 符号付き32bit整数を読みます。
    pub fn i32(&mut self) -> EngineResult<i32> {
        Ok(i32::from_le_bytes(
            self.raw(4)?.try_into().map_err(|_| invalid())?,
        ))
    }

    /// 符号付き64bit整数を読みます。
    pub fn i64(&mut self) -> EngineResult<i64> {
        Ok(i64::from_le_bytes(
            self.raw(8)?.try_into().map_err(|_| invalid())?,
        ))
    }

    /// 長さを検証してpayloadを借用します。確保は呼出側が行います。
    pub fn bytes(&mut self, limit: usize) -> EngineResult<&'a [u8]> {
        let count = self.u32()? as usize;
        if count > limit {
            return Err(EngineError::capacity(
                "record byte数",
                count as u64,
                limit as u64,
            ));
        }
        self.raw(count)
    }

    /// 余分な末尾を拒否します。切れた入力と連結された別recordを区別します。
    pub fn finish(self) -> EngineResult<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

/// 作者型の明示的な保存契約です。
///
/// decodeは外部参照を接続する前の値を作ります。ID等の参照の有効性は、
/// storeへの登録時またはWorld復元の候補検証で別に確認します。
pub trait StateCodec: Sized {
    /// 型固有の値を明示した順序で書き込みます。
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()>;
    /// 型固有の値を読み込み、値域を検証して返します。
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self>;
}

/// 任意recordは0/1のpresenceとpayloadで表します。未定義のpresenceをNoneへ丸めません。
/// payloadの長さ・値域はTのcodecが検査し、外側のDecoderが全体byte上限を守ります。
impl<T: StateCodec> StateCodec for Option<T> {
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> {
        match self {
            None => encoder.u8(0),
            Some(value) => {
                encoder.u8(1)?;
                value.encode(encoder)
            }
        }
    }
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> {
        match decoder.u8()? {
            0 => Ok(None),
            1 => Ok(Some(T::decode(decoder)?)),
            _ => Err(invalid()),
        }
    }
}

macro_rules! integer_codec {
    ($($ty:ty => $method:ident),* $(,)?) => {$ (
        impl StateCodec for $ty {
            fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> { encoder.$method(*self) }
            fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> { decoder.$method() }
        }
    )*};
}
integer_codec!(u8 => u8, u16 => u16, u32 => u32, u64 => u64, i32 => i32, i64 => i64);

impl StateCodec for bool {
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> {
        encoder.u8(u8::from(*self))
    }
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> {
        match decoder.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(invalid()),
        }
    }
}

impl StateCodec for f32 {
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> {
        if !self.is_finite() {
            return Err(invalid());
        }
        encoder.u32(self.to_bits())
    }
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = Self::from_bits(decoder.u32()?);
        if !value.is_finite() {
            return Err(invalid());
        }
        Ok(value)
    }
}

impl StateCodec for f64 {
    fn encode(&self, encoder: &mut Encoder) -> EngineResult<()> {
        if !self.is_finite() {
            return Err(invalid());
        }
        encoder.u64(self.to_bits())
    }
    fn decode(decoder: &mut Decoder<'_>) -> EngineResult<Self> {
        let value = Self::from_bits(decoder.u64()?);
        if !value.is_finite() {
            return Err(invalid());
        }
        Ok(value)
    }
}

pub(crate) fn invalid() -> EngineError {
    EngineError::new(ErrorCode::InvalidEncoding, "保存recordの形式").at(
        super::error::ErrorStage::Decode,
        None,
        None,
    )
}

#[cfg(test)]
mod measurement_tests {
    use super::*;

    #[test]
    fn measuring_matches_encoding_and_rejects_invalid_blob_lengths_atomically() {
        let write = |out: &mut Encoder| -> EngineResult<()> {
            out.raw(b"header")?;
            out.u64(17)?;
            out.bytes(b"payload")
        };
        let mut normal = Encoder::new(100);
        let mut measured = Encoder::measuring();
        write(&mut normal).unwrap();
        write(&mut measured).unwrap();
        assert_eq!(measured.written_len(), normal.finish().len());
        let before = measured.written_len();
        if let Some(oversize) = (u32::MAX as usize).checked_add(1) {
            assert!(measured.measure_bytes(oversize).is_err());
            assert_eq!(measured.written_len(), before);
        }
        measured.measure_bytes(7).unwrap();
        assert_eq!(measured.written_len(), before + 11);
        assert!(measured.finish().is_empty());
        let mut normal = Encoder::new(100);
        assert!(normal.measure_bytes(0).is_err());
        assert!(normal.finish().is_empty());
    }
}
