//! 管理台“文件”页的文件管理：新建文件夹、重命名、删除、移动 / 复制、回收站、项目里按名字找、最近改动、打 zip
//! （浏览、预览、下载、上传见 gateway/api.rs 与 transfer.rs；设计见 docs/REMOTE_UX_2026-11-07.md §9）。
//!
//! 能打开管理台的人本来就有这台电脑上的完整 shell，这里不另加路径限制；只防误操作：
//! 新名字只能是一段（不含分隔符、不是 `.` / `..`），不覆盖已有的文件，不删根目录、盘符和用户主目录。

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::LocalError;

type R<T> = Result<T, LocalError>;

fn absolute(path: &str) -> R<PathBuf> {
    let p = PathBuf::from(path.trim());
    if path.trim().is_empty() || !p.is_absolute() {
        return Err(LocalError::invalid("path", "absolute"));
    }
    Ok(p)
}

/// 新名字：一段、不含控制字符、Windows 也能用。
fn valid_name(name: &str) -> R<&str> {
    let n = name.trim();
    let bad = n.is_empty()
        || n == "."
        || n == ".."
        || n.len() > 255
        || n.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
        || (cfg!(windows) && (n.chars().any(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) || n.ends_with(['.', ' '])));
    if bad {
        return Err(LocalError::invalid("name", "file_name"));
    }
    Ok(n)
}

fn io_err(e: std::io::Error, path: &Path) -> LocalError {
    let p = path.display().to_string();
    match e.kind() {
        ErrorKind::NotFound => LocalError::not_found().with("path", p),
        ErrorKind::PermissionDenied => LocalError::new("FORBIDDEN").with("path", p),
        ErrorKind::AlreadyExists => LocalError::new("FILE_EXISTS").with("path", p),
        _ => LocalError::from(e),
    }
}

/// 在 `dir` 里新建文件夹 `name`，返回完整路径。
pub fn mkdir(dir: &str, name: &str) -> R<String> {
    let target = absolute(dir)?.join(valid_name(name)?);
    std::fs::create_dir(&target).map_err(|e| io_err(e, &target))?;
    Ok(target.display().to_string())
}

/// 把 `path` 改名为同一文件夹里的 `name`（不覆盖已有的），返回新路径。
pub fn rename(path: &str, name: &str) -> R<String> {
    let from = absolute(path)?;
    let parent = from.parent().ok_or_else(|| LocalError::invalid("path", "not_root"))?;
    let to = parent.join(valid_name(name)?);
    if to == from {
        return Ok(to.display().to_string());
    }
    std::fs::symlink_metadata(&from).map_err(|e| io_err(e, &from))?;
    // 只差大小写的改名（不区分大小写的文件系统上 `to` “已存在”的就是它自己）照常进行
    let same_file = to.to_string_lossy().eq_ignore_ascii_case(&from.to_string_lossy());
    if !same_file && std::fs::symlink_metadata(&to).is_ok() {
        return Err(LocalError::new("FILE_EXISTS").with("path", to.display().to_string()));
    }
    std::fs::rename(&from, &to).map_err(|e| io_err(e, &from))?;
    Ok(to.display().to_string())
}

/// 不允许删除的位置：根目录、盘符、用户主目录。
fn protected(p: &Path) -> bool {
    p.parent().is_none() || dirs::home_dir().is_some_and(|h| h == p)
}

/// 删除文件，或连同内容删除文件夹（符号链接只删链接本身）。
pub fn remove(path: &str) -> R<()> {
    let p = absolute(path)?;
    if protected(&p) {
        return Err(LocalError::new("FORBIDDEN").with("path", p.display().to_string()));
    }
    let m = std::fs::symlink_metadata(&p).map_err(|e| io_err(e, &p))?;
    let r = if m.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    r.map_err(|e| io_err(e, &p))
}

// ---------------- 移动 / 复制 ----------------

/// 目标文件夹里不重名的名字：`a.txt` → `a (2).txt`，文件夹 `dir` → `dir (2)`。
fn unique(dir: &Path, name: &OsStr, is_dir: bool) -> PathBuf {
    let first = dir.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let p = Path::new(name);
    let (stem, ext) = match (is_dir, p.file_stem(), p.extension()) {
        (false, Some(s), Some(e)) => (s.to_string_lossy().into_owned(), format!(".{}", e.to_string_lossy())),
        _ => (name.to_string_lossy().into_owned(), String::new()),
    };
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| std::fs::symlink_metadata(c).is_err())
        .unwrap_or_else(|| dir.join(format!("{stem} ({}){ext}", now_ms())))
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// 连同内容复制（符号链接复制成链接本身，不跟进去）。
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(from)?;
    if m.file_type().is_symlink() {
        let target = std::fs::read_link(from)?;
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, to);
        #[cfg(windows)]
        return if std::fs::metadata(from).is_ok_and(|m| m.is_dir()) {
            std::os::windows::fs::symlink_dir(target, to)
        } else {
            std::os::windows::fs::symlink_file(target, to)
        };
    }
    if m.is_dir() {
        std::fs::create_dir(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ())
}

/// 要放进的文件夹：必须是已有的文件夹，不能是要移动的东西自己或它里面。
fn dest_dir(dest: &str, from: &Path) -> R<PathBuf> {
    let d = absolute(dest)?;
    if !std::fs::metadata(&d).map_err(|e| io_err(e, &d))?.is_dir() {
        return Err(LocalError::new("NOT_A_DIR").with("path", d.display().to_string()));
    }
    if d.starts_with(from) {
        return Err(LocalError::new("DEST_INSIDE").with("path", from.display().to_string()));
    }
    Ok(d)
}

/// 把 `path` 移动到文件夹 `dest` 里（重名时自动改名，不覆盖），返回新路径。跨磁盘时复制后删掉原来的。
pub fn move_into(path: &str, dest: &str) -> R<String> {
    let from = absolute(path)?;
    if protected(&from) {
        return Err(LocalError::new("FORBIDDEN").with("path", from.display().to_string()));
    }
    let m = std::fs::symlink_metadata(&from).map_err(|e| io_err(e, &from))?;
    let d = dest_dir(dest, &from)?;
    if from.parent() == Some(d.as_path()) {
        return Ok(from.display().to_string());
    }
    let name = from.file_name().ok_or_else(|| LocalError::invalid("path", "not_root"))?;
    let to = unique(&d, name, m.is_dir());
    if let Err(e) = std::fs::rename(&from, &to) {
        if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::PermissionDenied) {
            return Err(io_err(e, &from));
        }
        // 多半是跨磁盘（EXDEV / ERROR_NOT_SAME_DEVICE）：复制过去再删原来的；复制失败把复制了一半的清掉
        if let Err(e) = copy_tree(&from, &to) {
            let _ = if to.is_dir() { std::fs::remove_dir_all(&to) } else { std::fs::remove_file(&to) };
            return Err(io_err(e, &from));
        }
        let r = if m.is_dir() { std::fs::remove_dir_all(&from) } else { std::fs::remove_file(&from) };
        r.map_err(|e| io_err(e, &from))?;
    }
    Ok(to.display().to_string())
}

/// 把 `path` 复制到文件夹 `dest` 里（放回同一个文件夹时得到 `a (2).txt`），返回新路径。
pub fn copy_into(path: &str, dest: &str) -> R<String> {
    let from = absolute(path)?;
    let m = std::fs::symlink_metadata(&from).map_err(|e| io_err(e, &from))?;
    let d = dest_dir(dest, &from)?;
    let name = from.file_name().ok_or_else(|| LocalError::invalid("path", "not_root"))?;
    let to = unique(&d, name, m.is_dir());
    if let Err(e) = copy_tree(&from, &to) {
        let _ = if to.is_dir() { std::fs::remove_dir_all(&to) } else { std::fs::remove_file(&to) };
        return Err(io_err(e, &from));
    }
    Ok(to.display().to_string())
}

// ---------------- 回收站 ----------------
//
// 自己的回收站（数据目录下的 trash/），不用系统回收站：三个系统的系统回收站接口不一样，还原也拿不到可靠的位置，
// 而“删除后撤销”只需要能放回原处。每次放进去是一个 `<毫秒>-<随机>` 文件夹，里面是原来的东西和记着原路径的 `.origin`。
// 和数据目录不在同一个磁盘上的（移动要整个复制）不放回收站，界面改问“永久删除”。保留 7 天。

const TRASH_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
const ORIGIN: &str = ".origin";

#[derive(Debug, Serialize)]
pub struct Trashed {
    pub id: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct TrashFail {
    pub path: String,
    pub code: String,
}

#[derive(Debug, Default, Serialize)]
pub struct TrashOut {
    pub items: Vec<Trashed>,
    pub failed: Vec<TrashFail>,
}

/// 清掉放进去超过 7 天的。
pub fn purge_trash(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let now = now_ms();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let at = name.split('-').next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
        if now - at > TRASH_KEEP.as_millis() as i64 {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn same_volume(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let drive = |p: &Path| p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
        drive(a) == drive(b)
    }
}

/// 放进回收站。每一项单独成败：不能放的（受保护的位置、不在同一个磁盘、没有权限）记在 `failed` 里。
pub fn trash(root: &Path, paths: &[String]) -> TrashOut {
    purge_trash(root);
    let mut out = TrashOut::default();
    let fail = |out: &mut TrashOut, path: &str, code: &str| out.failed.push(TrashFail { path: path.into(), code: code.into() });
    if std::fs::create_dir_all(root).is_err() {
        paths.iter().for_each(|p| fail(&mut out, p, "TRASH_UNAVAILABLE"));
        return out;
    }
    for path in paths {
        let from = match absolute(path) {
            Ok(p) => p,
            Err(e) => {
                fail(&mut out, path, &e.code);
                continue;
            }
        };
        if protected(&from) || root.starts_with(&from) {
            fail(&mut out, path, "FORBIDDEN");
            continue;
        }
        if let Err(e) = std::fs::symlink_metadata(&from) {
            fail(&mut out, path, &io_err(e, &from).code);
            continue;
        }
        let parent = from.parent().unwrap_or(&from);
        if !same_volume(parent, root) {
            fail(&mut out, path, "TRASH_UNAVAILABLE");
            continue;
        }
        let id = format!("{}-{:06x}", now_ms(), rand::random::<u32>() & 0xff_ffff);
        let slot = root.join(&id);
        let name = from.file_name().unwrap_or(OsStr::new("item"));
        let r = std::fs::create_dir(&slot)
            .and_then(|_| std::fs::write(slot.join(ORIGIN), from.to_string_lossy().as_bytes()))
            .and_then(|_| std::fs::rename(&from, slot.join(name)));
        match r {
            Ok(()) => out.items.push(Trashed { id, path: path.clone() }),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&slot);
                let code = match e.kind() {
                    ErrorKind::NotFound | ErrorKind::PermissionDenied => io_err(e, &from).code,
                    _ => "TRASH_UNAVAILABLE".into(),
                };
                fail(&mut out, path, &code);
            }
        }
    }
    out
}

/// 从回收站放回原处（原处已经有同名的了就自动改名），返回放回后的路径。
pub fn restore(root: &Path, id: &str) -> R<String> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err(LocalError::invalid("id", "trash_id"));
    }
    let slot = root.join(id);
    let origin = std::fs::read_to_string(slot.join(ORIGIN)).map_err(|e| io_err(e, &slot))?;
    let origin = PathBuf::from(origin);
    let item = std::fs::read_dir(&slot)
        .map_err(|e| io_err(e, &slot))?
        .flatten()
        .find(|e| e.file_name() != ORIGIN)
        .ok_or_else(LocalError::not_found)?;
    let parent = origin.parent().ok_or_else(|| LocalError::invalid("path", "not_root"))?;
    std::fs::create_dir_all(parent).map_err(|e| io_err(e, parent))?;
    let is_dir = item.file_type().is_ok_and(|t| t.is_dir());
    let to = unique(parent, origin.file_name().unwrap_or(&item.file_name()), is_dir);
    std::fs::rename(item.path(), &to).map_err(|e| io_err(e, &origin))?;
    let _ = std::fs::remove_dir_all(&slot);
    Ok(to.display().to_string())
}

// ---------------- 在项目里找、最近改动 ----------------

#[derive(Debug, Serialize, PartialEq)]
pub struct Hit {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<i64>,
    /// 最近改动：在哪个项目里
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Hits {
    pub items: Vec<Hit>,
    /// 到了条数 / 耗时上限没找完
    pub truncated: bool,
}

/// 遵守 .gitignore 的遍历；`.git` 本身也跳过（隐藏文件打开时它会被列出来）。
fn walker(root: &Path, hidden: bool) -> ignore::Walk {
    ignore::WalkBuilder::new(root)
        .hidden(!hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .max_depth(Some(24))
        .filter_entry(|e| e.file_name() != ".git")
        .build()
}

fn mtime(m: &std::fs::Metadata) -> Option<i64> {
    m.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

const SEARCH_LIMIT: usize = 200;
const SEARCH_BUDGET: Duration = Duration::from_secs(3);
const SEARCH_VISIT: usize = 300_000;

/// 在 `root` 下按名字找：空格分开的每个词都要出现在相对路径里（不分大小写），最后一个词要在名字里。
/// 名字以它开头的排前面，再按路径短的在前。
pub fn search(root: &str, q: &str, hidden: bool) -> R<Hits> {
    let base = absolute(root)?;
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Ok(Hits { items: vec![], truncated: false });
    }
    let last = words.last().cloned().unwrap_or_default();
    let started = Instant::now();
    let mut hits = Vec::new();
    let mut truncated = false;
    for (n, e) in walker(&base, hidden).flatten().enumerate() {
        if n >= SEARCH_VISIT || started.elapsed() > SEARCH_BUDGET {
            truncated = true;
            break;
        }
        if e.depth() == 0 {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !name.contains(&last) {
            continue;
        }
        let rel = e.path().strip_prefix(&base).unwrap_or(e.path()).to_string_lossy().to_lowercase();
        if !words.iter().all(|w| rel.contains(w.as_str())) {
            continue;
        }
        let meta = e.metadata().ok();
        let is_dir = e.file_type().is_some_and(|t| t.is_dir());
        hits.push(Hit {
            path: e.path().display().to_string(),
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir,
            size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            mtime_ms: meta.as_ref().and_then(mtime),
            root: None,
        });
        if hits.len() >= SEARCH_LIMIT * 5 {
            truncated = true;
            break;
        }
    }
    hits.sort_by_key(|h| (!h.name.to_lowercase().starts_with(&last), h.path.len()));
    if hits.len() > SEARCH_LIMIT {
        hits.truncate(SEARCH_LIMIT);
        truncated = true;
    }
    Ok(Hits { items: hits, truncated })
}

const RECENT_LIMIT: usize = 60;
const RECENT_BUDGET: Duration = Duration::from_millis(2500);
const RECENT_VISIT: usize = 60_000;

/// 这些项目里 `since_ms` 之后改过的文件，新的在前（文件页“最近改动”，§9.3）。主目录、根目录这种太大的不扫。
pub fn recent(roots: &[String], since_ms: i64) -> Hits {
    let started = Instant::now();
    let home = dirs::home_dir();
    let mut hits = Vec::new();
    let mut truncated = false;
    let mut seen = std::collections::HashSet::new();
    for root in roots.iter().take(20) {
        let Ok(base) = absolute(root) else { continue };
        if protected(&base) || home.as_deref() == Some(base.as_path()) || !seen.insert(base.clone()) {
            continue;
        }
        for (n, e) in walker(&base, false).flatten().enumerate() {
            if n >= RECENT_VISIT || started.elapsed() > RECENT_BUDGET {
                truncated = true;
                break;
            }
            if !e.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let Some(t) = mtime(&m).filter(|t| *t >= since_ms) else { continue };
            hits.push(Hit {
                path: e.path().display().to_string(),
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: false,
                size: Some(m.len()),
                mtime_ms: Some(t),
                root: Some(base.display().to_string()),
            });
        }
    }
    hits.sort_by_key(|h| std::cmp::Reverse(h.mtime_ms));
    hits.truncate(RECENT_LIMIT);
    Hits { items: hits, truncated }
}

// ---------------- 打 zip（文件夹、多个文件一起下载） ----------------

pub const ZIP_MAX_BYTES: u64 = 4 << 30;
pub const ZIP_MAX_FILES: usize = 100_000;

/// 把这些文件 / 文件夹打进一个临时文件（不落名字，关掉就没了），返回文件和建议的下载名。
/// 文件夹里遵守 .gitignore、不带隐藏文件之外的东西照原样打包（`node_modules` 这类被忽略的不打进去，否则动辄几个 GB）。
pub fn zip(paths: &[String]) -> R<(std::fs::File, String)> {
    use std::io::{Seek, SeekFrom, Write};
    if paths.is_empty() {
        return Err(LocalError::invalid("paths", "required"));
    }
    let items: Vec<PathBuf> = paths.iter().map(|p| absolute(p)).collect::<R<_>>()?;
    let mut f = tempfile::tempfile().map_err(LocalError::from)?;
    let mut w = zip::ZipWriter::new(&mut f);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let (mut bytes, mut files) = (0u64, 0usize);
    let mut buf = vec![0u8; 256 * 1024];
    for item in &items {
        let m = std::fs::symlink_metadata(item).map_err(|e| io_err(e, item))?;
        let parent = item.parent().unwrap_or(item);
        let list: Vec<(PathBuf, bool)> = if m.is_dir() {
            walker(item, true).flatten().map(|e| (e.path().to_path_buf(), e.file_type().is_some_and(|t| t.is_dir()))).collect()
        } else {
            vec![(item.clone(), false)]
        };
        for (p, is_dir) in list {
            let rel = p.strip_prefix(parent).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            if is_dir {
                w.add_directory(format!("{rel}/"), opts).map_err(|e| LocalError::from(anyhow::anyhow!(e)))?;
                continue;
            }
            let Ok(mut src) = std::fs::File::open(&p) else { continue };
            files += 1;
            if files > ZIP_MAX_FILES {
                return Err(LocalError::new("ZIP_TOO_MANY").with("max", ZIP_MAX_FILES));
            }
            w.start_file(rel, opts).map_err(|e| LocalError::from(anyhow::anyhow!(e)))?;
            loop {
                let n = std::io::Read::read(&mut src, &mut buf).map_err(|e| io_err(e, &p))?;
                if n == 0 {
                    break;
                }
                bytes += n as u64;
                if bytes > ZIP_MAX_BYTES {
                    return Err(LocalError::new("ZIP_TOO_BIG").with("max", ZIP_MAX_BYTES));
                }
                w.write_all(&buf[..n]).map_err(LocalError::from)?;
            }
        }
    }
    w.finish().map_err(|e| LocalError::from(anyhow::anyhow!(e)))?;
    f.seek(SeekFrom::Start(0)).map_err(LocalError::from)?;
    let stem = match items.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "files".into()),
        [first, ..] => first.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "files".into()),
        [] => "files".into(),
    };
    Ok((f, format!("{stem}.zip")))
}

// ---------------- 编辑保存、跟随日志（§9.4 J4 / J5） ----------------

/// 能在页面里编辑的最大文件（和预览一样 1 MiB）。
pub const EDIT_MAX: u64 = 1024 * 1024;

fn file_mtime(p: &Path) -> R<(std::fs::Metadata, i64)> {
    let m = std::fs::metadata(p).map_err(|e| io_err(e, p))?;
    if !m.is_file() {
        return Err(LocalError::new("NOT_A_FILE").with("path", p.display().to_string()));
    }
    let t = mtime(&m).unwrap_or(0);
    Ok((m, t))
}

/// 保存编辑过的文字文件，返回新的修改时间。`if_mtime`：打开时的修改时间，对不上（期间被改过）报 FILE_CHANGED，
/// 界面问覆盖还是重新载入；覆盖时不带。原地写（保留权限、属主、硬链接），不新建文件。
pub fn write(path: &str, content: &str, if_mtime: Option<i64>) -> R<i64> {
    let p = absolute(path)?;
    let (m, now) = file_mtime(&p)?;
    if m.len() > EDIT_MAX || content.len() as u64 > EDIT_MAX {
        return Err(LocalError::new("FILE_TOO_BIG").with("max", EDIT_MAX));
    }
    if let Some(want) = if_mtime {
        if want != now {
            return Err(LocalError::new("FILE_CHANGED").with("mtime_ms", now));
        }
    }
    std::fs::write(&p, content).map_err(|e| io_err(e, &p))?;
    Ok(file_mtime(&p)?.1)
}

#[derive(Debug, Serialize)]
pub struct Tail {
    /// 文件现在多大；下一次从这里接着取
    pub size: u64,
    /// 这段从哪开始（文件变短了——被截断、轮转——就从头来，`reset` 为真）
    pub from: u64,
    pub reset: bool,
    pub text: String,
}

const TAIL_MAX: u64 = 256 * 1024;

/// 跟随日志：从 `from` 起新增的内容（一次最多 256 KiB，落后太多时跳到只给最后 256 KiB）。
pub fn tail(path: &str, from: u64) -> R<Tail> {
    use std::io::{Read, Seek, SeekFrom};
    let p = absolute(path)?;
    let (m, _) = file_mtime(&p)?;
    let size = m.len();
    let reset = from > size;
    let mut start = if reset { 0 } else { from };
    if size - start > TAIL_MAX {
        start = size - TAIL_MAX;
    }
    let mut f = std::fs::File::open(&p).map_err(|e| io_err(e, &p))?;
    f.seek(SeekFrom::Start(start)).map_err(LocalError::from)?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    f.take(size - start).read_to_end(&mut buf).map_err(LocalError::from)?;
    // 截在多字节字符中间时，把不完整的尾巴留给下一次
    let cut = match std::str::from_utf8(&buf) {
        Ok(_) => buf.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => buf.len(),
    };
    let text = String::from_utf8_lossy(&buf[..cut]).into_owned();
    Ok(Tail { size: start + cut as u64, from: start, reset: reset || (start > from), text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mkdir_rename_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().display().to_string();
        let d = mkdir(&root, "新建文件夹").unwrap();
        assert!(Path::new(&d).is_dir());
        assert_eq!(mkdir(&root, "新建文件夹").unwrap_err().code, "FILE_EXISTS");
        assert_eq!(mkdir(&root, "../x").unwrap_err().code, "VALIDATION_FAILED");
        assert_eq!(mkdir(&root, "..").unwrap_err().code, "VALIDATION_FAILED");
        assert_eq!(mkdir("relative", "x").unwrap_err().code, "VALIDATION_FAILED");

        std::fs::write(Path::new(&d).join("a.md"), "# hi").unwrap();
        std::fs::write(Path::new(&d).join("b.md"), "").unwrap();
        let a = Path::new(&d).join("a.md").display().to_string();
        assert_eq!(rename(&a, "b.md").unwrap_err().code, "FILE_EXISTS", "不覆盖");
        assert_eq!(rename(&a, "sub/c.md").unwrap_err().code, "VALIDATION_FAILED");
        let c = rename(&a, "c.md").unwrap();
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "# hi");
        assert_eq!(rename(&a, "d.md").unwrap_err().code, "NOT_FOUND");

        remove(&c).unwrap();
        assert!(!Path::new(&c).exists());
        remove(&d).unwrap();
        assert!(!Path::new(&d).exists(), "文件夹连同内容删除");
        assert_eq!(remove(&d).unwrap_err().code, "NOT_FOUND");
        assert_eq!(remove("/").unwrap_err().code, "FORBIDDEN");
        if let Some(h) = dirs::home_dir() {
            assert_eq!(remove(&h.display().to_string()).unwrap_err().code, "FORBIDDEN");
        }
    }

    #[test]
    fn move_copy_trash_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("x.txt"), "x").unwrap();
        std::fs::write(a.join("sub/y.txt"), "y").unwrap();
        let s = |p: &Path| p.display().to_string();

        // 复制到同一个文件夹：自动改名
        let c = copy_into(&s(&a.join("x.txt")), &s(&a)).unwrap();
        assert!(c.ends_with("x (2).txt"), "{c}");
        // 文件夹复制连同内容；不能放进自己里面
        let a2 = copy_into(&s(&a), &s(&b)).unwrap();
        assert_eq!(std::fs::read_to_string(Path::new(&a2).join("sub/y.txt")).unwrap(), "y");
        assert_eq!(copy_into(&s(&a), &s(&a.join("sub"))).unwrap_err().code, "DEST_INSIDE");
        assert_eq!(move_into(&s(&a), &s(&a)).unwrap_err().code, "DEST_INSIDE");
        // 移动：重名时改名，原来的没了
        let m = move_into(&s(&a.join("x.txt")), &s(&b.join("a"))).unwrap();
        assert!(m.ends_with("x (3).txt"), "复制过去的 a 里已有 x.txt、x (2).txt：{m}");
        assert!(!a.join("x.txt").exists());
        assert_eq!(move_into(&s(&a.join("nope")), &s(&b)).unwrap_err().code, "NOT_FOUND");
        assert_eq!(move_into(&s(&a.join("x (2).txt")), &s(&a.join("x (2).txt"))).unwrap_err().code, "NOT_A_DIR");

        // 回收站：放进去、原处没了、放回来；原处又有同名的就改名
        let bin = root.join(".bin");
        let y = s(&a.join("sub/y.txt"));
        let out = trash(&bin, &[y.clone(), s(&a.join("missing")), "/".into()]);
        assert_eq!(out.items.len(), 1);
        assert_eq!(out.failed.iter().map(|f| f.code.as_str()).collect::<Vec<_>>(), ["NOT_FOUND", "FORBIDDEN"]);
        assert!(!Path::new(&y).exists());
        std::fs::write(&y, "new").unwrap();
        let back = restore(&bin, &out.items[0].id).unwrap();
        assert!(back.ends_with("y (2).txt"), "{back}");
        assert_eq!(std::fs::read_to_string(&back).unwrap(), "y");
        assert_eq!(restore(&bin, &out.items[0].id).unwrap_err().code, "NOT_FOUND");
        assert_eq!(restore(&bin, "../x").unwrap_err().code, "VALIDATION_FAILED");
        // 回收站所在的文件夹（和它的上级）不能放进回收站
        assert_eq!(trash(&bin, &[s(root)]).failed[0].code, "FORBIDDEN");
    }

    #[test]
    fn search_recent_zip_respect_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(root.join("src/app/App.jsx"), "x").unwrap();
        std::fs::write(root.join("src/app.css"), "x").unwrap();
        std::fs::write(root.join("node_modules/pkg/app.js"), "x").unwrap();
        let r = root.display().to_string();

        let names = |h: &Hits| h.items.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        let h = search(&r, "app", false).unwrap();
        assert_eq!(names(&h), ["app", "app.css", "App.jsx"], "忽略 node_modules；名字开头的在前、路径短的在前");
        assert_eq!(names(&search(&r, "src jsx", false).unwrap()), ["App.jsx"]);
        assert!(search(&r, "  ", false).unwrap().items.is_empty());

        let rec = recent(&[r.clone(), r.clone()], 0);
        assert_eq!(rec.items.len(), 2, "去重、只算文件、不算隐藏文件、忽略 node_modules：{:?}", names(&rec));
        assert!(recent(&[r.clone()], now_ms() + 60_000).items.is_empty());

        let (f, name) = zip(&[root.join("src").display().to_string()]).unwrap();
        assert_eq!(name, "src.zip");
        let mut z = ::zip::ZipArchive::new(f).unwrap();
        let mut entries: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        entries.sort();
        assert_eq!(entries, ["src/", "src/app.css", "src/app/", "src/app/App.jsx"]);
    }

    #[test]
    fn write_checks_mtime_and_tail_follows() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.log");
        std::fs::write(&f, "one\n").unwrap();
        let p = f.display().to_string();
        let (_, t0) = file_mtime(&f).unwrap();
        assert_eq!(write(&p, "x", Some(t0 - 5)).unwrap_err().code, "FILE_CHANGED");
        let t1 = write(&p, "one\ntwo\n", Some(t0)).unwrap();
        assert!(t1 >= t0);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "one\ntwo\n");
        assert_eq!(write(&tmp.path().display().to_string(), "x", None).unwrap_err().code, "NOT_A_FILE");
        assert_eq!(write(&tmp.path().join("new").display().to_string(), "x", None).unwrap_err().code, "NOT_FOUND", "不新建文件");

        let a = tail(&p, 0).unwrap();
        assert_eq!((a.text.as_str(), a.size, a.reset), ("one\ntwo\n", 8, false));
        std::fs::write(&f, "one\ntwo\nthree\n").unwrap();
        let b = tail(&p, a.size).unwrap();
        assert_eq!((b.text.as_str(), b.from), ("three\n", 8));
        std::fs::write(&f, "new\n").unwrap();
        let c = tail(&p, b.size).unwrap();
        assert!(c.reset, "文件变短了从头来");
        assert_eq!(c.text, "new\n");
        // 半个汉字留给下一次
        std::fs::write(&f, [b"new\n".as_slice(), &"好".as_bytes()[..2]].concat()).unwrap();
        let d = tail(&p, c.size).unwrap();
        assert_eq!((d.text.as_str(), d.size), ("", 4));
    }
}
