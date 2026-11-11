//! 本设备的技能管理（AI 助手页 → 技能）：列出各 AI 工具在用户级目录里装的技能，看内容，删掉不要的。
//!
//! 只看用户级目录（项目里的 `.claude/skills` 不在这里管）。每个技能是一个带 `SKILL.md` 的子目录：
//!
//! | 根目录 | 谁会读 | 能删 |
//! | --- | --- | --- |
//! | `<Claude 配置目录>/skills` | Claude Code（OpenCode 也读） | 能；`synced/` 是 claude.ai 同步的，不列 |
//! | `$CODEX_HOME/skills` | Codex | 能；`.system/` 单独列成“内置”，只能看 |
//! | `~/.agents/skills` | Codex、OpenCode | 能 |
//! | `<OpenCode 配置目录>/skill(s)` | OpenCode | 能 |
//! | `$DSH_HOME/skills` | DSH | 能 |
//! | Claude Code 用户级插件的 `<安装目录>/skills` | Claude Code | 只能看（要删得卸载插件） |
//!
//! 同一个技能目录可能出现在几个根下面（软链、公共目录），按真实路径去重，`tools` 里列出所有会用到它的工具。
//! 删除不是真删：挪到 `<数据目录>/skill-trash/<秒>-<序号>/`（`meta.json` 记原位置），7 天后清掉，期间能撤销；
//! 技能本身是软链时只删链接（回收站只记它指向哪）。
//! 接口只认 `根/目录名`，不接受路径，在这里重新拼出来并检查。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::agents::Agent;
use crate::chat::commands::{frontmatter, read_head, valid_name};
use crate::error::LocalError;

const TRASH: &str = "skill-trash";
const TRASH_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
/// 详情里 SKILL.md 最多给这么多
const BODY_MAX: u64 = 256 * 1024;
/// 数文件、列文件时最多看这么多个（技能目录里偶尔会有 node_modules 之类）
const WALK_MAX: usize = 2000;
const LIST_FILES_MAX: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    User,
    System,
    Plugin,
}

#[derive(Debug, Clone)]
struct Root {
    /// 接口里的根标识：`claude`、`codex`、`agents`、`opencode`、`opencode-skill`、`dsh`、`codex-system`、`plugin.<名字>`
    key: String,
    dir: PathBuf,
    tools: Vec<&'static str>,
    source: Source,
    plugin: Option<String>,
    /// 这些子目录不算技能（claude 的 `synced/`）
    skip: &'static [&'static str],
}

/// 各工具的配置目录（测试里换成临时目录）
#[derive(Debug, Clone)]
struct Dirs {
    claude: PathBuf,
    codex: PathBuf,
    agents: PathBuf,
    opencode: PathBuf,
    dsh: PathBuf,
}

impl Dirs {
    fn real() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Dirs { claude: Agent::Claude.dir(), codex: Agent::Codex.dir(), agents: home.join(".agents"), opencode: Agent::OpenCode.dir(), dsh: Agent::Dsh.dir() }
    }

    fn roots(&self) -> Vec<Root> {
        let user = |key: &str, dir: PathBuf, tools: Vec<&'static str>| Root { key: key.into(), dir, tools, source: Source::User, plugin: None, skip: &[] };
        // OpenCode 也读 Claude 和 ~/.agents 的技能：装了 OpenCode 才标上
        let oc = self.opencode.is_dir();
        let with_oc = |mut v: Vec<&'static str>| {
            if oc {
                v.push("opencode");
            }
            v
        };
        let mut out = vec![
            Root { skip: &["synced"], ..user("claude", self.claude.join("skills"), with_oc(vec!["claude"])) },
            user("codex", self.codex.join("skills"), vec!["codex"]),
            user("agents", self.agents.join("skills"), with_oc(vec!["codex"])),
            user("opencode", self.opencode.join("skills"), vec!["opencode"]),
            user("opencode-skill", self.opencode.join("skill"), vec!["opencode"]),
            user("dsh", self.dsh.join("skills"), vec!["dsh"]),
            Root { source: Source::System, ..user("codex-system", self.codex.join("skills").join(".system"), vec!["codex"]) },
        ];
        for (name, path) in claude_plugins(&self.claude) {
            out.push(Root {
                key: format!("plugin.{name}"),
                dir: path.join("skills"),
                tools: vec!["claude"],
                source: Source::Plugin,
                plugin: Some(name),
                skip: &[],
            });
        }
        out
    }
}

/// Claude Code 用户级装的插件：`plugins/installed_plugins.json` 里 scope 是 user 的（项目级的归项目管）
fn claude_plugins(claude: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(claude.join("plugins").join("installed_plugins.json")) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    let Some(map) = v.get("plugins").and_then(|p| p.as_object()) else { return vec![] };
    let mut out = vec![];
    for (id, entries) in map {
        let name = id.split('@').next().unwrap_or(id).to_string();
        if name.is_empty() || name.contains(['/', '\\']) {
            continue;
        }
        let user = entries.as_array().into_iter().flatten().find(|e| e.get("scope").and_then(|s| s.as_str()) == Some("user"));
        if let Some(p) = user.and_then(|e| e.get("installPath")).and_then(|p| p.as_str()) {
            if !out.iter().any(|(n, _)| *n == name) {
                out.push((name, PathBuf::from(p)));
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// `根/目录名`
    pub id: String,
    pub name: String,
    pub description: String,
    pub tools: Vec<&'static str>,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub path: String,
    /// 技能目录本身是软链
    pub link: bool,
    pub files: usize,
    pub bytes: u64,
    /// 修改时间（毫秒）
    pub mtime: Option<i64>,
    pub deletable: bool,
    /// 看看自己装的（MCP 那张卡片管的“自动 / 先问”）
    pub ours: bool,
}

fn ok_dir_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0']) && name.len() <= 200
}

fn list_in(dirs: &Dirs) -> Vec<Item> {
    let mut out: Vec<(PathBuf, Item)> = vec![];
    for root in dirs.roots() {
        let Ok(rd) = std::fs::read_dir(&root.dir) else { continue };
        let mut names: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(String::from)).collect();
        names.sort();
        for dir_name in names {
            if !ok_dir_name(&dir_name) || root.skip.contains(&dir_name.as_str()) {
                continue;
            }
            let path = root.dir.join(&dir_name);
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if let Some((_, it)) = out.iter_mut().find(|(r, _)| *r == real) {
                for t in &root.tools {
                    if !it.tools.contains(t) {
                        it.tools.push(t);
                    }
                }
                continue;
            }
            let text = read_head(&skill_md);
            let (fm, body) = frontmatter(&text);
            let name = fm.get("name").filter(|n| valid_name(n)).cloned().unwrap_or_else(|| dir_name.clone());
            let description = describe(&text, fm.get("description").map(String::as_str), body);
            let (files, bytes) = measure(&path);
            let mtime = std::fs::metadata(&skill_md).and_then(|m| m.modified()).ok().and_then(ms);
            let link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
            let ours = root.source == Source::User && crate::agents::Skill::ours(&dir_name);
            out.push((
                real,
                Item {
                    id: format!("{}/{}", root.key, dir_name),
                    name,
                    description,
                    tools: root.tools.clone(),
                    source: root.source,
                    plugin: root.plugin.clone(),
                    path: path.display().to_string(),
                    link,
                    files,
                    bytes,
                    mtime,
                    deletable: root.source == Source::User,
                    ours,
                },
            ));
        }
    }
    out.into_iter().map(|(_, it)| it).collect()
}

/// 说明：frontmatter 的 `description`（支持 `>` / `|` 折行写法），没有就用正文第一行
fn describe(text: &str, desc: Option<&str>, body: &str) -> String {
    let s = match desc {
        Some(d) if d.starts_with('>') || d.starts_with('|') => {
            let mut lines = text.lines().skip_while(|l| !l.starts_with("description:")).skip(1);
            let mut parts = vec![];
            for l in lines.by_ref() {
                if !l.starts_with([' ', '\t']) {
                    break;
                }
                parts.push(l.trim());
            }
            parts.join(" ")
        }
        Some(d) => d.to_string(),
        None => body.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("").to_string(),
    };
    s.chars().take(300).collect()
}

fn ms(t: SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// 文件数和总大小（不跟着目录里面的软链走，最多看 WALK_MAX 个）
fn measure(dir: &Path) -> (usize, u64) {
    let mut files = vec![];
    walk(dir, Path::new(""), 0, &mut files, WALK_MAX);
    (files.len(), files.iter().map(|(_, n)| n).sum())
}

fn walk(dir: &Path, rel: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>, max: usize) {
    if depth > 8 || out.len() >= max {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= max {
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let r = rel.join(e.file_name());
        if ft.is_dir() {
            walk(&e.path(), &r, depth + 1, out, max);
        } else if ft.is_file() {
            out.push((r, e.metadata().map(|m| m.len()).unwrap_or(0)));
        }
    }
}

/// 按 `根/目录名` 找回技能目录；目录名不合法、根不存在、不是技能都算没有
fn resolve(dirs: &Dirs, root: &str, name: &str) -> Result<(Root, PathBuf), LocalError> {
    let root = dirs.roots().into_iter().find(|r| r.key == root).ok_or_else(LocalError::not_found)?;
    if !ok_dir_name(name) || root.skip.contains(&name) {
        return Err(LocalError::not_found());
    }
    let path = root.dir.join(name);
    if !path.join("SKILL.md").is_file() {
        return Err(LocalError::not_found());
    }
    Ok((root, path))
}

#[derive(Debug, Serialize)]
pub struct Detail {
    pub item: Item,
    /// SKILL.md 正文（太长时截断）
    pub body: String,
    pub truncated: bool,
    /// 目录里的文件（相对路径、大小）
    pub files: Vec<(String, u64)>,
    pub more_files: bool,
}

fn detail_in(dirs: &Dirs, root: &str, name: &str) -> Result<Detail, LocalError> {
    let (_, path) = resolve(dirs, root, name)?;
    let id = format!("{root}/{name}");
    let item = list_in(dirs).into_iter().find(|it| it.id == id || Path::new(&it.path) == path).ok_or_else(LocalError::not_found)?;
    let raw = {
        use std::io::Read;
        let mut buf = vec![];
        std::fs::File::open(path.join("SKILL.md"))?.take(BODY_MAX + 1).read_to_end(&mut buf)?;
        buf
    };
    let truncated = raw.len() as u64 > BODY_MAX;
    let mut body = String::from_utf8_lossy(&raw[..raw.len().min(BODY_MAX as usize)]).into_owned();
    if truncated {
        // 截断处可能切开了一个字：去掉替换字符
        body = body.trim_end_matches('\u{FFFD}').to_string();
    }
    let mut files = vec![];
    walk(&path, Path::new(""), 0, &mut files, LIST_FILES_MAX + 1);
    let more_files = files.len() > LIST_FILES_MAX;
    files.truncate(LIST_FILES_MAX);
    let files = files.into_iter().map(|(p, n)| (p.to_string_lossy().replace('\\', "/"), n)).collect();
    Ok(Detail { item, body, truncated, files, more_files })
}

/// 回收站里一条：`<秒>-<序号>/meta.json` 记着原来在哪，`skill/` 是挪过来的目录；软链只记 `link`（指向哪），没有 `skill/`
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TrashMeta {
    root: String,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<PathBuf>,
}

/// 删除：挪进回收站，返回回收站条目名（撤销用）
fn delete_in(dirs: &Dirs, trash_root: &Path, root: &str, name: &str, now: SystemTime) -> Result<String, LocalError> {
    let (root, path) = resolve(dirs, root, name)?;
    if root.source != Source::User {
        return Err(LocalError::new("FORBIDDEN"));
    }
    purge(trash_root, now);
    std::fs::create_dir_all(trash_root)?;
    let secs = now.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (entry, dir) = (1..)
        .map(|n| format!("{secs}-{n}"))
        .map(|e| (e.clone(), trash_root.join(e)))
        .find(|(_, d)| !d.exists())
        .expect("unbounded");
    std::fs::create_dir(&dir)?;
    let is_link = std::fs::symlink_metadata(&path)?.file_type().is_symlink();
    let meta = TrashMeta { root: root.key.clone(), name: name.to_string(), link: if is_link { Some(std::fs::read_link(&path)?) } else { None } };
    std::fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).map_err(|_| LocalError::new("INTERNAL"))?)?;
    let moved = if is_link {
        // 只删链接，指向的目录不动
        remove_link(&path)
    } else {
        move_dir(&path, &dir.join("skill"))
    };
    if let Err(e) = moved {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e.into());
    }
    tracing::info!(skill = %path.display(), trash = %dir.display(), "技能已移到回收站");
    Ok(entry)
}

/// 撤销删除：从回收站挪回原处（原处已经又有同名的就不动）
fn restore_in(dirs: &Dirs, trash_root: &Path, entry: &str) -> Result<(), LocalError> {
    if !entry.chars().all(|c| c.is_ascii_digit() || c == '-') || entry.is_empty() {
        return Err(LocalError::not_found());
    }
    let dir = trash_root.join(entry);
    let meta: TrashMeta = std::fs::read(dir.join("meta.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(LocalError::not_found)?;
    let root = dirs.roots().into_iter().find(|r| r.key == meta.root && r.source == Source::User).ok_or_else(LocalError::not_found)?;
    if !ok_dir_name(&meta.name) || root.skip.contains(&meta.name.as_str()) {
        return Err(LocalError::not_found());
    }
    let dest = root.dir.join(&meta.name);
    if std::fs::symlink_metadata(&dest).is_ok() {
        return Err(LocalError::new("SKILL_EXISTS").with("name", &meta.name));
    }
    std::fs::create_dir_all(&root.dir)?;
    match &meta.link {
        Some(target) => make_link(target, &dest)?,
        None => move_dir(&dir.join("skill"), &dest)?,
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

fn move_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    // 跨盘（数据目录和家目录不在一个分区）：复制过去再删
    copy_dir(from, to)?;
    std::fs::remove_dir_all(from)
}

fn make_link(target: &Path, at: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, at);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_dir(target, at);
}

fn remove_link(p: &Path) -> std::io::Result<()> {
    // Windows 上指向目录的链接（含 junction）要用 remove_dir
    std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p))
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let ft = e.file_type()?;
        let dest = to.join(e.file_name());
        if ft.is_dir() {
            copy_dir(&e.path(), &dest)?;
        } else if ft.is_file() {
            std::fs::copy(e.path(), &dest)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(e.path())?, &dest)?;
        }
    }
    Ok(())
}

/// 清掉回收站里放了超过 7 天的
fn purge(trash_root: &Path, now: SystemTime) {
    let Ok(rd) = std::fs::read_dir(trash_root) else { return };
    let cutoff = now.checked_sub(TRASH_KEEP).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(secs) = name.to_str().and_then(|n| n.split('-').next()).and_then(|s| s.parse::<u64>().ok()) else { continue };
        if secs < cutoff {
            let p = e.path();
            let r = if e.file_type().is_ok_and(|t| t.is_dir()) { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            if let Err(err) = r {
                tracing::warn!(path = %p.display(), %err, "清理技能回收站失败");
            }
        }
    }
}

// ---------------- 对外 ----------------

pub async fn list(home: PathBuf) -> Vec<Item> {
    tokio::task::spawn_blocking(move || {
        purge(&home.join(TRASH), SystemTime::now());
        list_in(&Dirs::real())
    })
    .await
    .unwrap_or_default()
}

pub async fn detail(root: String, name: String) -> Result<Detail, LocalError> {
    tokio::task::spawn_blocking(move || detail_in(&Dirs::real(), &root, &name)).await.map_err(|_| LocalError::new("INTERNAL"))?
}

pub async fn delete(home: PathBuf, root: String, name: String) -> Result<String, LocalError> {
    tokio::task::spawn_blocking(move || delete_in(&Dirs::real(), &home.join(TRASH), &root, &name, SystemTime::now()))
        .await
        .map_err(|_| LocalError::new("INTERNAL"))?
}

pub async fn restore(home: PathBuf, entry: String) -> Result<(), LocalError> {
    tokio::task::spawn_blocking(move || restore_in(&Dirs::real(), &home.join(TRASH), &entry)).await.map_err(|_| LocalError::new("INTERNAL"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(dir: &Path, name: &str, md: &str) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        std::fs::write(dir.join(name).join("SKILL.md"), md).unwrap();
    }

    fn setup() -> (tempfile::TempDir, Dirs) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        let dirs = Dirs { claude: p.join(".claude"), codex: p.join(".codex"), agents: p.join(".agents"), opencode: p.join("oc"), dsh: p.join(".dsh") };
        (d, dirs)
    }

    #[test]
    fn lists_sources_and_skips() {
        let (d, dirs) = setup();
        let cs = dirs.claude.join("skills");
        skill(&cs, "pdf", "---\nname: pdf-tools\ndescription: >\n  处理 PDF\n  和表格\n---\n正文");
        skill(&cs, "looklook-pages-ask", "---\nname: looklook-pages-ask\ndescription: x\n---\n");
        skill(&cs.join("synced"), "s1", "# synced");
        skill(&cs, ".trash", "# hidden");
        std::fs::create_dir_all(cs.join("empty")).unwrap();
        skill(&dirs.codex.join("skills").join(".system"), "imagegen", "# 画图\n");
        let plug = d.path().join("cache/foo/1.0.0");
        skill(&plug.join("skills"), "faq", "---\ndescription: 常见问题\n---\n");
        std::fs::create_dir_all(dirs.claude.join("plugins")).unwrap();
        let ip = serde_json::json!({ "version": 2, "plugins": {
            "foo@market": [{ "scope": "user", "installPath": plug }],
            "bar@market": [{ "scope": "project", "installPath": d.path().join("nope") }],
        }});
        std::fs::write(dirs.claude.join("plugins/installed_plugins.json"), ip.to_string()).unwrap();

        let items = list_in(&dirs);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["claude/looklook-pages-ask", "claude/pdf", "codex-system/imagegen", "plugin.foo/faq"]);
        let pdf = &items[1];
        assert_eq!((pdf.name.as_str(), pdf.description.as_str()), ("pdf-tools", "处理 PDF 和表格"));
        assert!(pdf.deletable && !pdf.ours && items[0].ours);
        assert_eq!(pdf.tools, ["claude"]);
        assert!(!items[2].deletable && items[2].source == Source::System && items[2].description == "画图");
        assert_eq!((items[3].source, items[3].plugin.as_deref(), items[3].deletable), (Source::Plugin, Some("foo"), false));
    }

    #[cfg(unix)]
    #[test]
    fn dedupes_links_and_deletes_only_link() {
        let (d, dirs) = setup();
        std::fs::create_dir_all(&dirs.opencode).unwrap();
        skill(&dirs.agents.join("skills"), "shared", "# 共用");
        std::fs::create_dir_all(dirs.claude.join("skills")).unwrap();
        std::os::unix::fs::symlink(dirs.agents.join("skills/shared"), dirs.claude.join("skills/shared")).unwrap();
        let items = list_in(&dirs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "claude/shared");
        assert!(items[0].link);
        assert_eq!(items[0].tools, ["claude", "opencode", "codex"]);
        let trash = d.path().join("trash");
        let entry = delete_in(&dirs, &trash, "claude", "shared", SystemTime::now()).unwrap();
        assert!(std::fs::symlink_metadata(dirs.claude.join("skills/shared")).is_err());
        assert!(dirs.agents.join("skills/shared/SKILL.md").is_file());
        restore_in(&dirs, &trash, &entry).unwrap();
        assert!(std::fs::symlink_metadata(dirs.claude.join("skills/shared")).unwrap().file_type().is_symlink());
        assert!(!trash.join(&entry).exists());
    }

    #[test]
    fn delete_moves_to_trash_and_purges() {
        let (d, dirs) = setup();
        skill(&dirs.dsh.join("skills"), "a", "# a");
        std::fs::write(dirs.dsh.join("skills/a/x.txt"), "1234").unwrap();
        let trash = d.path().join("trash");
        let old = SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
        delete_in(&dirs, &trash, "dsh", "a", old).unwrap();
        assert!(!dirs.dsh.join("skills/a").exists());
        assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 1);
        // 再删一个（现在）：8 天前那个被清掉
        skill(&dirs.dsh.join("skills"), "b", "# b");
        delete_in(&dirs, &trash, "dsh", "b", SystemTime::now()).unwrap();
        let left: Vec<String> = std::fs::read_dir(&trash).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        assert_eq!(left.len(), 1);
        let meta: TrashMeta = serde_json::from_slice(&std::fs::read(trash.join(&left[0]).join("meta.json")).unwrap()).unwrap();
        assert_eq!((meta.root.as_str(), meta.name.as_str()), ("dsh", "b"));
        // 撤销：挪回原处，文件都在；原处又有同名的就拒绝
        skill(&dirs.dsh.join("skills"), "c", "# c");
        let e = delete_in(&dirs, &trash, "dsh", "c", SystemTime::now()).unwrap();
        skill(&dirs.dsh.join("skills"), "c", "# 新的");
        assert_eq!(restore_in(&dirs, &trash, &e).unwrap_err().code, "SKILL_EXISTS");
        std::fs::remove_dir_all(dirs.dsh.join("skills/c")).unwrap();
        restore_in(&dirs, &trash, &e).unwrap();
        assert_eq!(std::fs::read_to_string(dirs.dsh.join("skills/c/SKILL.md")).unwrap(), "# c");
        for bad in ["../x", "", "a/b"] {
            assert!(restore_in(&dirs, &trash, bad).is_err());
        }
    }

    #[test]
    fn rejects_bad_targets() {
        let (d, dirs) = setup();
        let trash = d.path().join("trash");
        skill(&dirs.codex.join("skills").join(".system"), "imagegen", "# x");
        skill(&dirs.claude.join("skills").join("synced"), "s", "# x");
        skill(&dirs.claude.join("skills"), "ok", "# x");
        let now = SystemTime::now();
        for (root, name) in [("codex-system", "imagegen"), ("codex", ".system"), ("claude", "synced"), ("claude", ".."), ("claude", "../skills"), ("nope", "ok"), ("claude", "missing")] {
            assert!(delete_in(&dirs, &trash, root, name, now).is_err(), "{root}/{name}");
        }
        assert!(dirs.codex.join("skills/.system/imagegen").is_dir());
        let det = detail_in(&dirs, "claude", "ok").unwrap();
        assert_eq!(det.body, "# x");
        assert_eq!(det.files, [("SKILL.md".to_string(), 3)]);
    }
}
