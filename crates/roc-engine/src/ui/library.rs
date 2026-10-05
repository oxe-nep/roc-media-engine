use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

fn sanitize_segment(raw: &str) -> Result<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains("..") || raw.contains('/') || raw.contains('\\') {
        bail!("invalid path segment");
    }
    Ok(raw.to_string())
}

/// Recording / library video extensions (proxy mp4 + mezz mxf/mov, etc.).
fn is_library_video(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("mp4" | "mxf" | "mov" | "mkv" | "ts")
    )
}

pub fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("mkv") => "video/x-matroska",
        Some("mxf") => "application/mxf",
        Some("ts") => "video/mp2t",
        _ => "application/octet-stream",
    }
}

pub fn list_categories(root: &Path) -> Result<Vec<Value>> {
    fs::create_dir_all(root)?;
    let mut out = Vec::new();
    for ent in fs::read_dir(root).context("read recordings dir")? {
        let ent = ent?;
        if !ent.file_type()?.is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let mut count = 0u64;
        if let Ok(rd) = fs::read_dir(ent.path()) {
            for f in rd.flatten() {
                if is_library_video(&f.path()) {
                    count += 1;
                }
            }
        }
        out.push(json!({ "name": name, "file_count": count }));
    }
    out.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(b.get("name").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Ok(out)
}

pub fn create_category(root: &Path, name: &str) -> Result<Value> {
    let name = sanitize_segment(name)?;
    let path = root.join(&name);
    if path.exists() {
        bail!("category already exists");
    }
    fs::create_dir_all(&path)?;
    Ok(json!({ "name": name, "file_count": 0 }))
}

pub fn rename_category(root: &Path, old: &str, new: &str) -> Result<Value> {
    let old = sanitize_segment(old)?;
    let new = sanitize_segment(new)?;
    let from = root.join(&old);
    let to = root.join(&new);
    if !from.is_dir() {
        bail!("category not found");
    }
    if to.exists() {
        bail!("target category already exists");
    }
    fs::rename(&from, &to)?;
    let cats = list_categories(root)?;
    cats.into_iter()
        .find(|c| c.get("name").and_then(|v| v.as_str()) == Some(new.as_str()))
        .context("renamed category missing")
}

pub fn delete_category(root: &Path, name: &str) -> Result<()> {
    let name = sanitize_segment(name)?;
    if name == "_unsorted" {
        bail!("cannot delete default category");
    }
    let path = root.join(&name);
    if !path.is_dir() {
        bail!("category not found");
    }
    fs::remove_dir_all(&path)?;
    Ok(())
}

pub fn list_files(root: &Path, category: &str) -> Result<Vec<Value>> {
    fs::create_dir_all(root)?;
    let mut out = Vec::new();
    let cats: Vec<String> = if category.is_empty() {
        list_categories(root)?
            .into_iter()
            .filter_map(|c| c.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect()
    } else {
        vec![sanitize_segment(category)?]
    };
    for cat in cats {
        let dir = root.join(&cat);
        if !dir.is_dir() {
            continue;
        }
        for ent in fs::read_dir(&dir)?.flatten() {
            let path = ent.path();
            if !is_library_video(&path) {
                continue;
            }
            let name = ent.file_name().to_string_lossy().into_owned();
            let meta = ent.metadata()?;
            let mod_time: DateTime<Utc> = meta.modified().ok().map(DateTime::<Utc>::from).unwrap_or_else(Utc::now);
            out.push(json!({
                "category": cat,
                "name": name,
                "size": meta.len(),
                "mod_time": mod_time.to_rfc3339(),
                "url": format!("/api/library/file/{cat}/{name}"),
            }));
        }
    }
    out.sort_by(|a, b| {
        let am = a.get("mod_time").and_then(|v| v.as_str()).unwrap_or("");
        let bm = b.get("mod_time").and_then(|v| v.as_str()).unwrap_or("");
        bm.cmp(am)
    });
    Ok(out)
}

pub fn file_path(root: &Path, category: &str, name: &str) -> Result<PathBuf> {
    let category = sanitize_segment(category)?;
    let name = sanitize_segment(name)?;
    let path = root.join(&category).join(&name);
    if !is_library_video(&path) {
        bail!("invalid file name");
    }
    if !path.is_file() {
        bail!("file not found");
    }
    Ok(path)
}

pub fn delete_file(root: &Path, category: &str, name: &str) -> Result<()> {
    let path = file_path(root, category, name)?;
    fs::remove_file(path)?;
    Ok(())
}

pub fn move_file(root: &Path, from_cat: &str, to_cat: &str, name: &str) -> Result<Value> {
    let from = file_path(root, from_cat, name)?;
    let to_cat = sanitize_segment(to_cat)?;
    let name = sanitize_segment(name)?;
    let to_dir = root.join(&to_cat);
    fs::create_dir_all(&to_dir)?;
    let to = to_dir.join(&name);
    if to.exists() {
        bail!("destination already exists");
    }
    fs::rename(&from, &to)?;
    let meta = fs::metadata(&to)?;
    let mod_time: DateTime<Utc> = meta.modified().ok().map(DateTime::<Utc>::from).unwrap_or_else(Utc::now);
    Ok(json!({
        "category": to_cat,
        "name": name,
        "size": meta.len(),
        "mod_time": mod_time.to_rfc3339(),
        "url": format!("/api/library/file/{to_cat}/{name}"),
    }))
}
