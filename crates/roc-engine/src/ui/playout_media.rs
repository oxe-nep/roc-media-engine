//! Uploaded playout media library (`/api/playout/media`).

use std::fs::{self, File};
use std::io::{copy, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_MEDIA_UPLOAD_BYTES: u64 = 16 << 30; // 16 GiB

fn allowed_ext(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        ".mp4" | ".mov" | ".mkv" | ".mxf" | ".ts"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing, default)]
    pub path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistFile {
    items: Vec<PersistedMedia>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedMedia {
    id: String,
    name: String,
    filename: String,
    size: u64,
    created_at: DateTime<Utc>,
}

pub struct MediaStore {
    dir: PathBuf,
    meta_path: PathBuf,
    items: Mutex<std::collections::HashMap<String, MediaItem>>,
}

impl MediaStore {
    pub fn open(data_dir: &Path) -> Self {
        let dir = data_dir.join("playout-media");
        let meta_path = data_dir.join("playout-media.json");
        let _ = fs::create_dir_all(&dir);
        let s = Self {
            dir,
            meta_path,
            items: Mutex::new(std::collections::HashMap::new()),
        };
        s.load();
        s
    }

    fn load(&self) {
        let Ok(raw) = fs::read_to_string(&self.meta_path) else {
            return;
        };
        let Ok(f) = serde_json::from_str::<PersistFile>(&raw) else {
            return;
        };
        let mut map = self.items.lock().unwrap();
        for it in f.items {
            let path = self.dir.join(&it.filename);
            if !path.is_file() {
                continue;
            }
            map.insert(
                it.id.clone(),
                MediaItem {
                    id: it.id,
                    name: it.name,
                    size: it.size,
                    created_at: it.created_at,
                    path,
                },
            );
        }
    }

    fn save_locked(&self, map: &std::collections::HashMap<String, MediaItem>) -> Result<()> {
        let mut items: Vec<_> = map
            .values()
            .map(|it| PersistedMedia {
                id: it.id.clone(),
                name: it.name.clone(),
                filename: it
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                size: it.size,
                created_at: it.created_at,
            })
            .collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let raw = serde_json::to_vec_pretty(&PersistFile { items })?;
        fs::write(&self.meta_path, raw).context("write playout-media.json")?;
        Ok(())
    }

    pub fn list(&self) -> Vec<MediaItem> {
        let map = self.items.lock().unwrap();
        let mut out: Vec<_> = map.values().cloned().collect();
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        out
    }

    pub fn add_from_reader(&self, orig_name: &str, mut reader: impl Read) -> Result<MediaItem> {
        let ext = Path::new(orig_name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        if !allowed_ext(&ext) {
            bail!("unsupported media type {ext:?} (allowed: mp4, mov, mkv, mxf, ts)");
        }
        fs::create_dir_all(&self.dir)?;
        let id = Uuid::new_v4().simple().to_string();
        let safe = sanitize_filename(
            Path::new(orig_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("media"),
        );
        let filename = format!("{id}_{safe}{ext}");
        let dest = self.dir.join(&filename);
        let mut file = File::create(&dest).context("create media file")?;
        let mut limited = reader.by_ref().take(MAX_MEDIA_UPLOAD_BYTES + 1);
        let n = copy(&mut limited, &mut file)? as u64;
        if n > MAX_MEDIA_UPLOAD_BYTES {
            let _ = fs::remove_file(&dest);
            bail!("file too large (max {MAX_MEDIA_UPLOAD_BYTES} bytes)");
        }
        let item = MediaItem {
            id: id.clone(),
            name: Path::new(orig_name)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| orig_name.to_string()),
            size: n,
            created_at: Utc::now(),
            path: dest.clone(),
        };
        let mut map = self.items.lock().unwrap();
        map.insert(id, item.clone());
        if let Err(e) = self.save_locked(&map) {
            map.remove(&item.id);
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
        Ok(item)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut map = self.items.lock().unwrap();
        let Some(it) = map.remove(id) else {
            bail!("media `{id}` not found");
        };
        let _ = fs::remove_file(&it.path);
        self.save_locked(&map)?;
        Ok(())
    }
}

fn sanitize_filename(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => out.push(c),
            ' ' => out.push('_'),
            _ => {}
        }
    }
    if out.is_empty() {
        out.push_str("media");
    }
    if out.len() > 80 {
        out.truncate(80);
    }
    out
}
