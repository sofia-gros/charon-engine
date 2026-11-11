//! 仕様 04 §3 / ロードマップ T6.9: 剛体をスタンプから分離して保存する。

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::Path;

use charon_core::cell::{Cell, FLAG_BODY, FLAG_PARITY, MaterialId};
use charon_core::material::MaterialTable;
use rapier2d::prelude::{Rotation, Vector};

use crate::{BodyError, CELLS_PER_METER, PhysicsEngine};

const MAGIC: &[u8; 4] = b"CHRB";
const VERSION: u16 = 1;
const CELL_BYTES: usize = 8;
const BODY_BYTES: usize = 28;

#[derive(Debug)]
pub enum BodySaveError {
    Io(io::Error),
    InvalidData,
    NewerVersion(u16),
    MissingMaterial(MaterialId),
    Body(BodyError),
}

impl fmt::Display for BodySaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "剛体ファイルの入出力に失敗: {error}"),
            Self::InvalidData => write!(f, "剛体ファイルが壊れています"),
            Self::NewerVersion(version) => write!(f, "未対応の新しい剛体形式: {version}"),
            Self::MissingMaterial(id) => write!(f, "剛体の材質 ID {} を保存できません", id.0),
            Self::Body(error) => write!(f, "保存された剛体を生成できません: {error}"),
        }
    }
}

impl std::error::Error for BodySaveError {}

impl From<io::Error> for BodySaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn read<const N: usize>(cursor: &mut Cursor<&[u8]>) -> Result<[u8; N], BodySaveError> {
    let mut bytes = [0; N];
    cursor.read_exact(&mut bytes).map_err(|_| BodySaveError::InvalidData)?;
    Ok(bytes)
}

fn read_u16(cursor: &mut Cursor<&[u8]>) -> Result<u16, BodySaveError> {
    Ok(u16::from_le_bytes(read(cursor)?))
}

fn read_u32(cursor: &mut Cursor<&[u8]>) -> Result<u32, BodySaveError> {
    Ok(u32::from_le_bytes(read(cursor)?))
}

fn read_f32(cursor: &mut Cursor<&[u8]>) -> Result<f32, BodySaveError> {
    let value = f32::from_le_bytes(read(cursor)?);
    if value.is_finite() { Ok(value) } else { Err(BodySaveError::InvalidData) }
}

impl PhysicsEngine {
    /// `world.ron` の材質名順を使い、スタンプでない剛体本体と速度を保存する。
    pub fn save_bodies(
        &self,
        root: &Path,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<(), BodySaveError> {
        let count = u32::try_from(self.bodies.len()).map_err(|_| BodySaveError::InvalidData)?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        for body in &self.bodies {
            let Some(rigid) = self.world.bodies.get(body.handle) else {
                return Err(BodySaveError::InvalidData);
            };
            bytes.extend_from_slice(&body.width.to_le_bytes());
            bytes.extend_from_slice(&body.height.to_le_bytes());
            for value in [
                rigid.translation().x * CELLS_PER_METER,
                rigid.translation().y * CELLS_PER_METER,
                rigid.rotation().angle(),
                rigid.linvel().x,
                rigid.linvel().y,
                rigid.angvel(),
            ] {
                if !value.is_finite() {
                    return Err(BodySaveError::InvalidData);
                }
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            for cell in &body.cells {
                let name = &materials.get(cell.material).name;
                let index = saved_names
                    .iter()
                    .position(|saved| saved == name)
                    .and_then(|index| u16::try_from(index).ok())
                    .ok_or(BodySaveError::MissingMaterial(cell.material))?;
                bytes.extend_from_slice(&index.to_le_bytes());
                bytes.push(cell.flags & !(FLAG_BODY | FLAG_PARITY));
                bytes.push(cell.shade);
                bytes.extend_from_slice(&cell.temp.to_le_bytes());
                bytes.push(cell.vy as u8);
                bytes.push(cell.life);
            }
        }
        fs::create_dir_all(root)?;
        let path = root.join("bodies.bin");
        for suffix in 0..32 {
            let temporary = root.join(format!("bodies.bin.tmp.{suffix}"));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&temporary) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
                drop(file);
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }
            drop(file);
            if let Err(error) = fs::rename(&temporary, &path) {
                let _ = fs::remove_file(&temporary);
                return Err(error.into());
            }
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "空いている剛体の一時ファイル名がありません",
        )
        .into())
    }

    /// 保存ファイルがなければ None。全件の検証後だけ復元結果を返す。
    pub fn load_bodies(
        root: &Path,
        materials: &MaterialTable,
        saved_names: &[String],
    ) -> Result<Option<Self>, BodySaveError> {
        let bytes = match fs::read(root.join("bodies.bin")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut cursor = Cursor::new(bytes.as_slice());
        if &read::<4>(&mut cursor)? != MAGIC {
            return Err(BodySaveError::InvalidData);
        }
        let version = read_u16(&mut cursor)?;
        if version > VERSION {
            return Err(BodySaveError::NewerVersion(version));
        }
        if version != VERSION {
            return Err(BodySaveError::InvalidData);
        }
        let count = read_u32(&mut cursor)? as usize;
        if count > (bytes.len() - cursor.position() as usize) / BODY_BYTES {
            return Err(BodySaveError::InvalidData);
        }
        let mut engine = Self::new();
        for _ in 0..count {
            let width = read_u16(&mut cursor)?;
            let height = read_u16(&mut cursor)?;
            let values = [
                read_f32(&mut cursor)?,
                read_f32(&mut cursor)?,
                read_f32(&mut cursor)?,
                read_f32(&mut cursor)?,
                read_f32(&mut cursor)?,
                read_f32(&mut cursor)?,
            ];
            let len = usize::from(width) * usize::from(height);
            if len == 0 || len > (bytes.len() - cursor.position() as usize) / CELL_BYTES {
                return Err(BodySaveError::InvalidData);
            }
            let mut cells = Vec::with_capacity(len);
            for _ in 0..len {
                let index = read_u16(&mut cursor)?;
                let name = saved_names.get(usize::from(index)).ok_or(BodySaveError::InvalidData)?;
                let material = materials.id(name).unwrap_or(MaterialId::EMPTY);
                let flags = read::<1>(&mut cursor)?[0] & !(FLAG_BODY | FLAG_PARITY);
                let shade = read::<1>(&mut cursor)?[0];
                let temp = i16::from_le_bytes(read(&mut cursor)?);
                let vy = read::<1>(&mut cursor)?[0] as i8;
                let life = read::<1>(&mut cursor)?[0];
                cells.push(Cell { material, flags, shade, temp, vy, life });
            }
            let index = engine
                .create_body(width, height, cells, materials, [values[0], values[1]])
                .map_err(BodySaveError::Body)?;
            let rigid = &mut engine.world.bodies[engine.bodies[index].handle];
            rigid.set_rotation(Rotation::new(values[2]), true);
            rigid.set_linvel(Vector::new(values[3], values[4]), true);
            rigid.set_angvel(values[5], true);
        }
        if cursor.position() as usize != bytes.len() {
            return Err(BodySaveError::InvalidData);
        }
        Ok(Some(engine))
    }
}
