//! 从同一账户的另一台电脑导入宠物（设置 → 宠物 → 从其他电脑导入，docs/PET_PACK.md §6.3.1）。
//!
//! - 导出方（A）：`GET /api/pets/local/{id}/export` 把 `pets/<id>/` 打成 zip（不压缩，片段本来就是压过的视频）发出来；
//! - 导入方（B）：用户点了“导入”才开始（[`start`]），B 的后端经 `rtc::outbound` 直接向 A 取包，在后台下载、校验、装好，
//!   界面关掉也照样进行；界面轮询 [`list`] 看进度。同一只同时只下载一份。
//!
//! 宠物包不是秘密，A 那边是 `Access::Rtc`（同一账户、平台签过的连接），和浏览器远程打开 A 时能看的一样。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::LocalError;
use crate::gateway::App;
use crate::pets::{self, perr, PackError};

/// 做完的记录留多久（界面看到结果就够了）
const KEEP: Duration = Duration::from_secs(600);

#[derive(Clone, Debug, Serialize)]
pub struct Job {
    /// `{device}:{pet}`
    pub key: String,
    pub device: String,
    pub pet: String,
    /// running | done | failed
    pub state: &'static str,
    /// 收到的字节数、总字节数（对方给了长度时）
    pub got: u64,
    pub total: Option<u64>,
    /// 失败时的错误（`{code, params}`，界面按错误码显示）
    pub error: Option<Value>,
    /// 装好后的宠物（同设置页列表里的一项）
    pub pack: Option<Value>,
    #[serde(skip)]
    at: Option<Instant>,
}

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());

fn jobs() -> std::sync::MutexGuard<'static, Vec<Job>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

fn update(key: &str, f: impl FnOnce(&mut Job)) {
    if let Some(j) = jobs().iter_mut().find(|j| j.key == key) {
        f(j);
    }
}

/// 进行中的和最近做完的
pub fn list() -> Value {
    let mut all = jobs();
    all.retain(|j| j.state == "running" || j.at.is_none_or(|t| t.elapsed() < KEEP));
    json!({ "items": *all })
}

/// 开始从 `device` 导入它的宠物 `pet`（后台进行）；同一只已经在下载就返回那一份
pub fn start(app: &App, device: &str, pet: &str) -> Result<Job, LocalError> {
    if !pets::valid_id(pet) {
        return Err(LocalError::invalid("id", "format"));
    }
    let own = app.account.session().ok_or_else(|| LocalError::new("NOT_LOGGED_IN"))?.device_id;
    if device == own || device.is_empty() || device.len() > 64 {
        return Err(LocalError::invalid("device", "invalid"));
    }
    let key = format!("{device}:{pet}");
    let job = {
        let mut all = jobs();
        if let Some(j) = all.iter().find(|j| j.key == key && j.state == "running") {
            return Ok(j.clone());
        }
        all.retain(|j| j.key != key);
        let j = Job { key: key.clone(), device: device.into(), pet: pet.into(), state: "running", got: 0, total: None, error: None, pack: None, at: None };
        all.push(j.clone());
        j
    };
    let app = app.clone();
    let (device, pet) = (device.to_string(), pet.to_string());
    tokio::spawn(async move {
        let r = fetch(&app, &key, &device, &pet).await;
        update(&key, |j| {
            j.at = Some(Instant::now());
            match r {
                Ok(p) => {
                    j.state = "done";
                    j.pack = Some(p.summary());
                }
                Err(e) => {
                    tracing::warn!(device = %device, pet = %pet, code = %e.code, "从其他电脑导入宠物失败");
                    j.state = "failed";
                    j.error = Some(json!({ "code": e.code, "params": e.params }));
                }
            }
        });
    });
    Ok(job)
}

async fn fetch(app: &App, key: &str, device: &str, pet: &str) -> Result<pets::Pack, LocalError> {
    let link = app.outbound.link(&app.account, device).await.map_err(|e| e.local())?;
    download(link, &app.paths.home, key, device, pet).await
}

/// 另一台电脑装了哪些宠物（它的 `GET /api/pets`）：经这台电脑到它的连接去问
pub async fn remote_list(app: &App, device: &str) -> Result<Value, LocalError> {
    let own = app.account.session().ok_or_else(|| LocalError::new("NOT_LOGGED_IN"))?.device_id;
    if device == own || device.is_empty() || device.len() > 64 {
        return Err(LocalError::invalid("device", "invalid"));
    }
    let link = app.outbound.link(&app.account, device).await.map_err(|e| e.local())?;
    let path = "/api/pets".to_string();
    let req = Request::builder().method("GET").uri(&path).header(crate::gateway::CLIENT_HEADER, "1").body(Body::empty()).map_err(anyhow::Error::from)?;
    let resp = crate::rtc::outbound::http(link, req, path).await;
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 4 << 20).await.map_err(|_| LocalError::new("RTC_FAILED"))?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| LocalError::new("RTC_FAILED"))?;
    if !status.is_success() {
        return Err(match v["error"]["code"].as_str() {
            Some(c) => LocalError { status, code: c.to_string(), params: Default::default() },
            None => LocalError::new("RTC_FAILED"),
        });
    }
    Ok(v)
}

/// 经连接取包、存成临时文件、装好
pub(crate) async fn download(link: Arc<crate::rtc::outbound::Link>, home: &Path, key: &str, device: &str, pet: &str) -> Result<pets::Pack, LocalError> {
    let path = format!("/api/pets/local/{pet}/export");
    let req = Request::builder().method("GET").uri(&path).header(crate::gateway::CLIENT_HEADER, "1").body(Body::empty()).map_err(anyhow::Error::from)?;
    let resp = crate::rtc::outbound::http(link, req, path).await;
    let status = resp.status();
    if !status.is_success() {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap_or_default();
        let code = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v["error"]["code"].as_str().map(String::from));
        return Err(match code {
            Some(c) => LocalError { status, code: c, params: Default::default() },
            // 那台电脑的看看太旧，还没有这个接口
            None if status == StatusCode::NOT_FOUND => LocalError::new("PET_PEER_OLD"),
            None => LocalError::new("RTC_FAILED"),
        });
    }
    let total = resp.headers().get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    update(key, |j| j.total = total);
    let base = pets::local_dir(home);
    tokio::fs::create_dir_all(&base).await?;
    let tmp = tempfile::Builder::new().prefix(".from-device-").suffix(".zip").tempfile_in(&base).map_err(anyhow::Error::from)?;
    let (file, tmp_path) = tmp.into_parts();
    let mut f = tokio::fs::File::from_std(file);
    let mut stream = resp.into_body().into_data_stream();
    let mut got = 0u64;
    let mut last = Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| LocalError::new("RTC_FAILED"))?;
        got += chunk.len() as u64;
        if got > pets::MAX_PACK + (1 << 20) {
            return Err(pets::pack_error(perr("too_big", "")));
        }
        tokio::io::AsyncWriteExt::write_all(&mut f, &chunk).await?;
        if last.elapsed() > Duration::from_millis(300) {
            last = Instant::now();
            update(key, |j| j.got = got);
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut f).await?;
    drop(f);
    update(key, |j| j.got = got);
    if total.is_some_and(|t| t != got) {
        return Err(LocalError::new("RTC_FAILED"));
    }
    let home = home.to_path_buf();
    let origin = json!({ "device": device });
    tokio::task::spawn_blocking(move || {
        let r = pets::import_with_origin(&home, &tmp_path, Some(&origin));
        drop(tmp_path);
        r
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(pets::pack_error)
}

/// 把 `pets/<id>/` 打成 zip 放进临时文件（`pets/` 下面，用完删）。不收隐藏文件（`.origin.json` 等），不跟符号链接。
pub fn export(home: &Path, id: &str) -> Result<tempfile::NamedTempFile, PackError> {
    if !pets::valid_id(id) {
        return Err(perr("missing_file", id));
    }
    let base = pets::local_dir(home);
    let dir = base.join(id);
    if !dir.join("pet.json").is_file() {
        return Err(perr("missing_file", "pet.json"));
    }
    let tmp = tempfile::Builder::new().prefix(".export-").suffix(".zip").tempfile_in(&base).map_err(|e| perr("io", e.to_string()))?;
    let mut z = zip::ZipWriter::new(tmp.reopen().map_err(|e| perr("io", e.to_string()))?);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(false);
    let mut files = vec![];
    walk(&dir, &mut files, 0)?;
    let mut size = 0u64;
    for p in files {
        let rel = p.strip_prefix(&dir).map_err(|_| perr("io", ""))?.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(&p).map_err(|e| perr("io", e.to_string()))?;
        size += bytes.len() as u64;
        if size > pets::MAX_PACK {
            return Err(perr("too_big", ""));
        }
        z.start_file(rel, opts).map_err(|e| perr("io", e.to_string()))?;
        z.write_all(&bytes).map_err(|e| perr("io", e.to_string()))?;
    }
    z.finish().map_err(|e| perr("io", e.to_string()))?;
    Ok(tmp)
}

fn walk(d: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), PackError> {
    if depth > 4 {
        return Ok(());
    }
    let mut es: Vec<_> = std::fs::read_dir(d).map_err(|e| perr("io", e.to_string()))?.flatten().collect();
    es.sort_by_key(|e| e.file_name());
    for e in es {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            walk(&e.path(), out, depth + 1)?;
        } else if t.is_file() {
            out.push(e.path());
            if out.len() > pets::MAX_FILES * 2 {
                return Err(perr("too_many", out.len().to_string()));
            }
        }
    }
    Ok(())
}

/// `GET /api/pets/local/{id}/export` 的响应：zip 文件流
pub async fn export_response(home: PathBuf, id: String) -> Result<axum::response::Response, LocalError> {
    let tmp = tokio::task::spawn_blocking(move || export(&home, &id)).await.map_err(anyhow::Error::from)?.map_err(|e| {
        if e.code == "missing_file" {
            LocalError::not_found()
        } else {
            pets::pack_error(e)
        }
    })?;
    // 打开后就可以删掉临时文件（Unix 上照样能读完；Windows 上删不掉就等 NamedTempFile 释放时删）
    let len = tmp.as_file().metadata().map(|m| m.len()).unwrap_or(0);
    let f = tokio::fs::File::open(tmp.path()).await?;
    let holder = Arc::new(tmp);
    let stream = tokio_util::io::ReaderStream::with_capacity(f, 256 * 1024).map(move |c| {
        let _keep = &holder;
        c
    });
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/zip")
        .header(header::CONTENT_LENGTH, len.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(anyhow::Error::from)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_then_import() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        pets::tests::min_pack(src.path());
        let p = pets::import_path(a.path(), src.path()).unwrap();
        std::fs::write(pets::local_dir(a.path()).join(&p.id).join(".origin.json"), "{}").unwrap();
        let zip = export(a.path(), &p.id).unwrap();
        let got = pets::import_with_origin(b.path(), zip.path(), Some(&json!({ "device": "d1" }))).unwrap();
        assert_eq!(got.id, p.id);
        let dir = pets::local_dir(b.path()).join(&p.id);
        let origin: Value = serde_json::from_slice(&std::fs::read(dir.join(".origin.json")).unwrap()).unwrap();
        assert_eq!(origin["device"], "d1");
        assert!(export(a.path(), "nope").is_err());
        assert!(export(a.path(), "../x").is_err());
    }
}
