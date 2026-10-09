//! Library metadata overrides. Originals are never rewritten; EXIF reindexing
//! preserves the user's explicit values, including fields cleared by the user.
use crate::ipc::{run_blocking, SharedState};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EditableMetadata {
    pub title: String,
    pub description: String,
    pub author: String,
    pub copyright: String,
    pub keywords: Vec<String>,
    pub captured_at: Option<String>,
    pub camera: String,
    pub lens: String,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

pub fn get(db: &crate::db::Db, id: i64) -> Result<EditableMetadata, String> {
    let asset = db
        .asset_by_id(id)
        .map_err(|e| e.to_string())?
        .ok_or("照片不存在")?;
    let value: Option<String> =
        db.0.query_row(
            "SELECT value FROM asset_metadata WHERE asset_id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(value) = value {
        return serde_json::from_str(&value).map_err(|e| e.to_string());
    }
    // Read descriptive EXIF fields not held in the gallery's fast index.
    let head = std::fs::File::open(&asset.path).ok().and_then(|file| {
        use std::io::Read;
        let mut head = Vec::new();
        file.take(2 * 1024 * 1024).read_to_end(&mut head).ok()?;
        Some(head)
    });
    let exif = head.as_ref().and_then(|head| {
        exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(head))
            .ok()
    });
    let ascii = |tag| {
        exif.as_ref()
            .and_then(|data| data.get_field(tag, exif::In::PRIMARY))
            .and_then(|field| {
                if let exif::Value::Ascii(values) = &field.value {
                    values.first().map(|value| {
                        String::from_utf8_lossy(value)
                            .trim_end_matches('\0')
                            .to_owned()
                    })
                } else {
                    None
                }
            })
    };
    let source = head
        .as_ref()
        .map(|head| crate::metadata::exif_lite::parse(head))
        .unwrap_or_default();
    let position = asset
        .gps_lat
        .or(source.deep.gps_lat)
        .zip(asset.gps_lon.or(source.deep.gps_lon));
    Ok(EditableMetadata {
        description: ascii(exif::Tag::ImageDescription).unwrap_or_default(),
        copyright: ascii(exif::Tag::Copyright).unwrap_or_default(),
        captured_at: asset
            .captured_at
            .or(source.captured_at.map(|date| date.to_rfc3339())),
        camera: asset.camera.or(source.camera).unwrap_or_default(),
        lens: asset.lens.or(source.lens).unwrap_or_default(),
        author: asset.artist.or(source.deep.artist).unwrap_or_default(),
        gps_lat: position.map(|value| value.0),
        gps_lon: position.map(|value| value.1),
        ..Default::default()
    })
}

pub fn save(
    db: &crate::db::Db,
    id: i64,
    mut value: EditableMetadata,
) -> Result<EditableMetadata, String> {
    if value.gps_lat.is_some() != value.gps_lon.is_some() {
        return Err("经纬度需要同时填写或同时清除".into());
    }
    if value
        .gps_lat
        .is_some_and(|v| !v.is_finite() || !(-90.0..=90.0).contains(&v))
        || value
            .gps_lon
            .is_some_and(|v| !v.is_finite() || !(-180.0..=180.0).contains(&v))
    {
        return Err("经纬度超出有效范围".into());
    }
    if let Some(date) = value.captured_at.as_ref() {
        if chrono::DateTime::parse_from_rfc3339(date).is_err()
            && chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%dT%H:%M:%S").is_err()
        {
            return Err("拍摄日期格式无效".into());
        }
    }
    for text in [
        &mut value.title,
        &mut value.description,
        &mut value.author,
        &mut value.copyright,
        &mut value.camera,
        &mut value.lens,
    ] {
        *text = text.trim().to_owned();
        if text.len() > 16_384 {
            return Err("元数据文字过长".into());
        }
    }
    value.keywords = value
        .keywords
        .into_iter()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .collect();
    value.keywords.sort();
    value.keywords.dedup();
    if value.keywords.len() > 200 || value.keywords.iter().any(|v| v.len() > 512) {
        return Err("关键词过多或过长".into());
    }
    let json = serde_json::to_string(&value).map_err(|e| e.to_string())?;
    let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
    let changed = tx.execute("UPDATE assets SET captured_at=?2, camera=NULLIF(?3,''), lens=NULLIF(?4,''), artist=NULLIF(?5,''), gps_lat=?6, gps_lon=?7 WHERE id=?1 AND kind IN ('photo','raw')",
        params![id, value.captured_at, value.camera, value.lens, value.author, value.gps_lat, value.gps_lon]).map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("照片不存在".into());
    }
    tx.execute("INSERT INTO asset_metadata(asset_id,value) VALUES(?1,?2) ON CONFLICT(asset_id) DO UPDATE SET value=excluded.value", params![id, json]).map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(value)
}

#[tauri::command]
pub async fn asset_metadata_get(
    state: State<'_, SharedState>,
    asset_id: i64,
) -> Result<EditableMetadata, String> {
    run_blocking(state.inner().clone(), move |state| {
        get(&crate::ipc::app_database_db(state)?, asset_id)
    })
    .await
}

#[tauri::command]
pub async fn asset_metadata_save(
    state: State<'_, SharedState>,
    asset_id: i64,
    metadata: EditableMetadata,
) -> Result<EditableMetadata, String> {
    run_blocking(state.inner().clone(), move |state| {
        let db = crate::ipc::app_database_db(state)?;
        let saved = save(&db, asset_id, metadata)?;
        // 编辑联动重索引（2026-09-29 地图定案）：GPS 变化（含清除）即时迁移
        // 地区挂接；索引未就绪时静默跳过（下次管线增量补齐）
        let (lat, lon) = (saved.gps_lat, saved.gps_lon);
        if crate::geo::backfill::reindex_asset(&db, asset_id, lat, lon) {
            state
                .bus
                .publish(crate::events::AppEvent::MapRegionsUpdated);
        }
        Ok(saved)
    })
    .await
}
