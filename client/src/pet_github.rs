//! 从 GitHub 仓库装宠物（docs/PET_PACK.md §6.2）：下载仓库的 zip → 找出所有带 `pet.json` 的文件夹 →
//! 每个解到 `pets/.gh-<令牌>/<序号>/` 里校验 → 用户挑一部分（只有一只时直接装）→ 换进 `pets/<id>/`。
//!
//! 一个仓库可以只放一只（`pet.json` 在根目录），也可以放好几只（每只一个文件夹，嵌套多深都行）。
//! 只解宠物文件夹，仓库里别的文件（代码、大图、README）不碰。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::{json, Value};

use crate::pets::{self, perr, Pack, PackError};

/// 仓库 zip 最大多少（多只宠物 + 别的文件）
pub const MAX_REPO: u64 = 600 << 20;
/// 一个仓库里最多认多少只
const MAX_PETS: usize = 60;
/// 挑选用的临时目录留多久
const STAGE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    /// 分支 / 标签 / 提交；None = 默认分支
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// 只看仓库里这个文件夹下面（`/tree/<ref>/<path>` 链接带的）
    pub path: String,
}

impl Repo {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    fn zip_url(&self) -> String {
        format!("https://github.com/{}/{}/archive/{}.zip", self.owner, self.name, self.reference.as_deref().unwrap_or("HEAD"))
    }
}

fn gh_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn gh_ref(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.contains("..") && !s.starts_with('/') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

/// 认这些写法：`owner/repo`、`owner/repo@ref`、`github.com/owner/repo`、`https://github.com/owner/repo(.git)`、
/// `https://github.com/owner/repo/tree/<ref>/<文件夹>`（ref 里带 `/` 的分支请用 `@ref` 写法）
pub fn parse(input: &str) -> Option<Repo> {
    let s = input.trim().trim_end_matches('/');
    let s = s.strip_prefix("git+").unwrap_or(s);
    let s = ["https://", "http://"].iter().find_map(|p| s.strip_prefix(p)).unwrap_or(s);
    let s = s.strip_prefix("www.").unwrap_or(s);
    let s = s.strip_prefix("github.com/").unwrap_or(s);
    if s.contains("://") || s.contains('?') || s.contains('#') {
        return None;
    }
    let (s, at_ref) = match s.split_once('@') {
        Some((a, r)) => (a, Some(r.to_string())),
        None => (s, None),
    };
    let mut seg = s.split('/');
    let owner = seg.next()?.to_string();
    let name = seg.next()?.trim_end_matches(".git").to_string();
    let rest: Vec<&str> = seg.collect();
    let (mut reference, path) = match rest.as_slice() {
        [] => (None, String::new()),
        ["tree" | "blob", r, p @ ..] => (Some(r.to_string()), p.join("/")),
        _ => return None,
    };
    if at_ref.is_some() {
        reference = at_ref;
    }
    // 指向 pet.json 本身的链接 = 它所在的文件夹
    let path = path.strip_suffix("pet.json").unwrap_or(&path).trim_end_matches('/').to_string();
    if !gh_name(&owner) || !gh_name(&name) || reference.as_deref().is_some_and(|r| !gh_ref(r)) || path.split('/').any(|p| p == "..") {
        return None;
    }
    Some(Repo { owner, name, reference, path })
}

/// 输入看起来是 GitHub 仓库（而不是本地文件）
pub fn looks_like(input: &str) -> bool {
    let s = input.trim();
    s.contains("github.com/") || (!s.contains('\\') && !s.ends_with(".llpet") && !s.ends_with(".zip") && !Path::new(s).exists() && parse(s).is_some())
}

/// 下载仓库 zip 到 `out`（超过 [`MAX_REPO`] 就停）
pub async fn download(repo: &Repo, out: &Path) -> Result<(), PackError> {
    use tokio::io::AsyncWriteExt;
    let c = reqwest::Client::builder()
        .user_agent(concat!("looklook-client/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        // 大仓库在慢网络上要下好久：不限总时长，60 秒收不到数据才算断
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| perr("download", e.to_string()))?;
    let mut res = c.get(repo.zip_url()).send().await.map_err(|e| perr("download", e.to_string()))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(perr("repo_not_found", repo.slug()));
    }
    if !res.status().is_success() {
        return Err(perr("download", format!("HTTP {}", res.status().as_u16())));
    }
    if res.content_length().is_some_and(|n| n > MAX_REPO) {
        return Err(perr("repo_too_big", ""));
    }
    let mut f = tokio::fs::File::create(out).await.map_err(|e| perr("io", e.to_string()))?;
    let mut size = 0u64;
    while let Some(chunk) = res.chunk().await.map_err(|e| perr("download", e.to_string()))? {
        size += chunk.len() as u64;
        if size > MAX_REPO {
            return Err(perr("repo_too_big", ""));
        }
        f.write_all(&chunk).await.map_err(|e| perr("io", e.to_string()))?;
    }
    f.flush().await.map_err(|e| perr("io", e.to_string()))?;
    Ok(())
}

/// 仓库里认出来的一只宠物
#[derive(Debug, Clone, PartialEq)]
enum Cand {
    /// 带 `pet.json` 的文件夹（本格式或 Codex 宠物）
    Ours(String),
    /// dsh-pet 插件本体：`<dir>assets/config.jsonc` + `<dir>assets/webm/`
    DshPackage(String),
    /// dsh-pet 自己做的种类：`<dir><kind>-config.json` + `<dir><kind>-animation/`
    DshKind(String, String),
}

impl Cand {
    fn dir(&self) -> &str {
        match self {
            Cand::Ours(d) | Cand::DshPackage(d) | Cand::DshKind(d, _) => d,
        }
    }
}

fn hidden(d: &str) -> bool {
    d.contains("/__MACOSX/") || d.split('/').any(|s| s.starts_with('.') && !s.is_empty()) || d.split('/').any(|s| s == "node_modules")
}

/// zip 里的每只宠物（目录是完整前缀，以 `/` 结尾或为空）；只要 `under` 下面的，套在别的宠物文件夹里的不算
fn candidates(names: &[String], top: &str, under: &str) -> Vec<Cand> {
    let want = if under.is_empty() { top.to_string() } else { format!("{top}{under}/") };
    let has_prefix = |p: &str| names.iter().any(|n| n.starts_with(p) && n.len() > p.len());
    let parent = |n: &str, suffix: &str| -> Option<String> {
        if n == suffix {
            Some(String::new())
        } else {
            n.strip_suffix(&format!("/{suffix}")).map(|d| format!("{d}/"))
        }
    };
    let mut all: Vec<Cand> = Vec::new();
    for n in names {
        if let Some(d) = parent(n, "pet.json") {
            all.push(Cand::Ours(d));
        } else if let Some(d) = parent(n, "assets/config.jsonc") {
            if has_prefix(&format!("{d}assets/webm/")) {
                all.push(Cand::DshPackage(d));
            }
        } else if let Some((d, kind)) = n.rsplit_once('/').map(|(d, f)| (format!("{d}/"), f)).or(Some((String::new(), n.as_str()))).and_then(|(d, f)| f.strip_suffix("-config.json").map(|k| (d, k.to_string()))) {
            if !kind.is_empty() && has_prefix(&format!("{d}{kind}-animation/")) {
                all.push(Cand::DshKind(d, kind));
            }
        }
    }
    all.retain(|c| c.dir().starts_with(&want) && !hidden(c.dir()));
    all.sort_by(|a, b| a.dir().cmp(b.dir()).then_with(|| format!("{a:?}").cmp(&format!("{b:?}"))));
    all.dedup();
    let mut out: Vec<Cand> = Vec::new();
    for c in all {
        // 套在别的宠物文件夹里面的（比如包里的示例）不算；同一个文件夹里的几个 dsh 种类都算
        if !out.iter().any(|o| c.dir().starts_with(o.dir()) && (c.dir() != o.dir() || !matches!((o, &c), (Cand::DshKind(..), Cand::DshKind(..))))) {
            out.push(c);
        }
    }
    out
}

/// 只由小写英文、数字、`-` 组成的 id；中文名之类的换成 `dsh-<哈希>`（同一个名字每次都一样，重装会覆盖）
fn ascii_id(name: &str, prefix: &str) -> String {
    let s: String = name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if pets::valid_id(&s) && s.len() >= 2 {
        return s;
    }
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    format!("{prefix}{h:08x}")
}

/// GitHub 的 zip 外面包了一层 `<repo>-<ref>/`
fn top_dir(names: &[String]) -> String {
    let first = names.first().and_then(|n| n.split_once('/')).map(|(a, _)| format!("{a}/")).unwrap_or_default();
    if !first.is_empty() && names.iter().all(|n| n.starts_with(&first)) {
        first
    } else {
        String::new()
    }
}

#[derive(Debug)]
pub struct Found {
    /// 仓库里的位置（空 = 根目录）
    pub path: String,
    pub pack: Result<Pack, PackError>,
    /// 解到哪了（安装时搬走它）
    root: PathBuf,
}

/// 解出仓库 zip 里的每只宠物到 `stage/<序号>/<文件夹名>/` 并校验；dsh-pet 格式的只解配置和动画，再补一份 `pet.json`
pub fn stage(zip_path: &Path, under: &str, stage: &Path) -> Result<Vec<Found>, PackError> {
    let f = std::fs::File::open(zip_path).map_err(|_| perr("bad_zip", ""))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| perr("bad_zip", e.to_string()))?;
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    let top = top_dir(&names);
    let cands = candidates(&names, &top, under);
    if cands.is_empty() {
        return Err(perr("no_pets", under));
    }
    if cands.len() > MAX_PETS {
        return Err(perr("too_many_pets", cands.len().to_string()));
    }
    let mut out = Vec::new();
    for (i, c) in cands.iter().enumerate() {
        let d = c.dir();
        let dir_path = d.strip_prefix(&top).unwrap_or(d).trim_end_matches('/').to_string();
        let path = match c {
            Cand::DshKind(_, kind) if dir_path.is_empty() => kind.clone(),
            Cand::DshKind(_, kind) => format!("{dir_path}/{kind}"),
            _ => dir_path,
        };
        // Codex 宠物没写 id 时拿文件夹名当 id，所以解到同名文件夹里
        let leaf = path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("pet");
        let leaf: String = leaf.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let root = stage.join(i.to_string()).join(if leaf.is_empty() || leaf.starts_with('.') { "pet".into() } else { leaf });
        std::fs::create_dir_all(&root).map_err(|e| perr("io", e.to_string()))?;
        let pack = stage_one(&mut z, c, &path, &root).and_then(|_| pets::load_dir("local", &root));
        std::fs::write(stage.join(format!("{i}.path")), &path).map_err(|e| perr("io", e.to_string()))?;
        out.push(Found { path, pack, root });
    }
    Ok(out)
}

fn stage_one<R: std::io::Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, c: &Cand, path: &str, root: &Path) -> Result<(), PackError> {
    let manifest = match c {
        Cand::Ours(d) => return pets::extract(z, d, root),
        Cand::DshPackage(d) => {
            pets::extract_some(z, &format!("{d}assets/"), &root.join("assets"), &|n| n == "config.jsonc" || n.starts_with("webm/") || n.starts_with("mov/"))?;
            let leaf = path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("dsh-pet");
            json!({
                "format": "dsh-pet",
                "id": ascii_id(leaf, "dsh-"),
                "config": "assets/config.jsonc",
                "webm": "assets/webm",
                "mov": "assets/mov",
                "license": pets::dsh_license(),
                "credits": pets::dsh_credits(),
            })
        }
        Cand::DshKind(d, kind) => {
            let (cfg, anim) = (format!("{kind}-config.json"), format!("{kind}-animation/"));
            pets::extract_some(z, d, root, &|n| n == cfg || n.starts_with(&anim))?;
            json!({
                "format": "dsh-pet",
                "id": ascii_id(kind, "dsh-"),
                "config": cfg,
                "webm": format!("{kind}-animation"),
                "mov": format!("{kind}-animation/mov"),
            })
        }
    };
    std::fs::write(root.join("pet.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| perr("io", e.to_string()))
}

/// 装挑中的几只（`picks` 是仓库里的位置或 id；空 = 全部能装的）
pub fn install(home: &Path, repo: &Repo, found: Vec<Found>, picks: &[String]) -> Result<Vec<Pack>, PackError> {
    let chosen: Vec<&Found> = found
        .iter()
        .filter(|f| f.pack.is_ok())
        .filter(|f| picks.is_empty() || picks.iter().any(|p| *p == f.path || f.pack.as_ref().is_ok_and(|k| k.id == *p)))
        .collect();
    if chosen.is_empty() {
        return Err(match found.iter().find_map(|f| f.pack.as_ref().err()) {
            Some(e) if picks.is_empty() => e.clone(),
            _ => perr("no_pets", picks.join(",")),
        });
    }
    let mut ids = std::collections::HashSet::new();
    for f in &chosen {
        let id = &f.pack.as_ref().expect("ok").id;
        if !ids.insert(id.clone()) {
            return Err(perr("dup_id", id));
        }
    }
    let mut done = Vec::new();
    for f in chosen {
        let origin = json!({ "github": repo.slug(), "ref": repo.reference, "path": f.path });
        done.push(pets::install_root(home, &f.root, Some(&origin))?);
    }
    pets::forget();
    Ok(done)
}

/// 设置页列表的一项：包的摘要 + 仓库里的位置；坏的包也列出来（灰掉、写原因）
pub fn found_json(f: &Found, i: usize) -> Value {
    let mut v = match &f.pack {
        Ok(p) => p.summary(),
        Err(e) => json!({ "id": f.root.file_name().map(|s| s.to_string_lossy().into_owned()), "error": e }),
    };
    v["path"] = json!(f.path);
    v["index"] = json!(i);
    v
}

// ---------------- 设置页：先看再装 ----------------

pub fn stage_dir(home: &Path, token: &str) -> Option<PathBuf> {
    (token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit())).then(|| pets::local_dir(home).join(format!(".gh-{token}")))
}

/// 清掉过期的挑选目录
pub fn sweep(home: &Path) {
    let Ok(rd) = std::fs::read_dir(pets::local_dir(home)) else { return };
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|d| d > STAGE_TTL);
        if n.starts_with(".gh-") && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// 下载 + 解开 + 校验，结果留在 `.gh-<令牌>/`，`repo.json` 记下仓库，等用户挑
pub async fn scan(home: &Path, repo: &Repo) -> Result<Value, PackError> {
    let base = pets::local_dir(home);
    tokio::fs::create_dir_all(&base).await.map_err(|e| perr("io", e.to_string()))?;
    {
        let h = home.to_path_buf();
        let _ = tokio::task::spawn_blocking(move || sweep(&h)).await;
    }
    let token = format!("{:016x}", rand::random::<u64>());
    let dir = stage_dir(home, &token).expect("token");
    std::fs::create_dir_all(&dir).map_err(|e| perr("io", e.to_string()))?;
    let zip = dir.join("repo.zip");
    let r = async {
        download(repo, &zip).await?;
        let (z, under, d) = (zip.clone(), repo.path.clone(), dir.join("p"));
        let found = tokio::task::spawn_blocking(move || stage(&z, &under, &d)).await.map_err(|e| perr("io", e.to_string()))??;
        let _ = std::fs::remove_file(&zip);
        std::fs::write(dir.join("repo.json"), serde_json::to_vec(repo).unwrap_or_default()).map_err(|e| perr("io", e.to_string()))?;
        Ok::<_, PackError>(found)
    }
    .await;
    let found = match r {
        Ok(f) => f,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
    };
    Ok(json!({
        "token": token,
        "repo": repo,
        "items": found.iter().enumerate().map(|(i, f)| found_json(f, i)).collect::<Vec<_>>(),
    }))
}

/// 重新读一遍挑选目录（scan 时解好的），带 scan 结果里的序号
fn restage(dir: &Path) -> Result<(Repo, Vec<(usize, Found)>), PackError> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join("repo.json")).map_err(|_| perr("expired", ""))?).map_err(|_| perr("expired", ""))?;
    let repo = Repo {
        owner: v["owner"].as_str().unwrap_or_default().into(),
        name: v["name"].as_str().unwrap_or_default().into(),
        reference: v["ref"].as_str().map(str::to_string),
        path: v["path"].as_str().unwrap_or_default().into(),
    };
    let p = dir.join("p");
    let mut idx: Vec<(usize, PathBuf)> = std::fs::read_dir(&p)
        .map_err(|_| perr("expired", ""))?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().to_str()?.parse::<usize>().ok()?, e.path())))
        .collect();
    idx.sort();
    let mut found = Vec::new();
    for (i, d) in idx {
        let Some(root) = std::fs::read_dir(&d).ok().and_then(|r| r.flatten().find(|e| e.file_type().is_ok_and(|t| t.is_dir()))).map(|e| e.path()) else { continue };
        let path = std::fs::read_to_string(p.join(format!("{i}.path"))).unwrap_or_default();
        found.push((i, Found { path, pack: pets::load_dir("local", &root), root }));
    }
    Ok((repo, found))
}

/// 用户挑好了：`indexes` 是 scan 结果里的 `index`
pub fn install_staged(home: &Path, token: &str, indexes: &[usize]) -> Result<Vec<Pack>, PackError> {
    let dir = stage_dir(home, token).ok_or_else(|| perr("expired", ""))?;
    let (repo, found) = restage(&dir)?;
    let found: Vec<Found> = found.into_iter().filter(|(i, _)| indexes.contains(i)).map(|(_, f)| f).collect();
    let r = if found.is_empty() { Err(perr("no_pets", "")) } else { install(home, &repo, found, &[]) };
    let _ = std::fs::remove_dir_all(&dir);
    r
}

/// 挑选时看封面：`.gh-<令牌>/p/<序号>/…/preview.*`
pub fn staged_preview(home: &Path, token: &str, index: usize) -> Option<PathBuf> {
    let d = stage_dir(home, token)?.join("p").join(index.to_string());
    let root = std::fs::read_dir(&d).ok()?.flatten().find(|e| e.file_type().is_ok_and(|t| t.is_dir()))?.path();
    let pack = pets::load_dir("local", &root).ok()?;
    pack.file("preview").or_else(|| pack.file("sprite")).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_inputs() {
        let r = parse("PC2005-cloud/dsh-pet").unwrap();
        assert_eq!((r.owner.as_str(), r.name.as_str(), r.reference.as_deref(), r.path.as_str()), ("PC2005-cloud", "dsh-pet", None, ""));
        let r = parse("https://github.com/a/b.git").unwrap();
        assert_eq!(r.name, "b");
        let r = parse("https://github.com/a/b/tree/main/pets/cat/").unwrap();
        assert_eq!((r.reference.as_deref(), r.path.as_str()), (Some("main"), "pets/cat"));
        let r = parse("github.com/a/b/blob/v1/pets/cat/pet.json").unwrap();
        assert_eq!((r.reference.as_deref(), r.path.as_str()), (Some("v1"), "pets/cat"));
        assert_eq!(parse("a/b@feat/x").unwrap().reference.as_deref(), Some("feat/x"));
        for bad in ["a", "https://gitlab.com/a/b", "a/b/c", "a/b/tree/main/../x", "a/b@../x", "../b"] {
            assert!(parse(bad).is_none(), "{bad}");
        }
        assert!(looks_like("https://github.com/a/b"));
        assert!(!looks_like("cat.llpet"));
    }

    #[test]
    fn find_candidates() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let names = n(&["r-main/", "r-main/README.md", "r-main/pets/a/pet.json", "r-main/pets/a/anim/x/pet.json", "r-main/pets/b/pet.json", "r-main/.github/pet.json"]);
        let top = top_dir(&names);
        assert_eq!(top, "r-main/");
        let ours = |d: &str| Cand::Ours(d.into());
        assert_eq!(candidates(&names, &top, ""), [ours("r-main/pets/a/"), ours("r-main/pets/b/")]);
        assert_eq!(candidates(&names, &top, "pets/b"), [ours("r-main/pets/b/")]);
        let one = n(&["r-main/", "r-main/pet.json", "r-main/anim/idle.webm"]);
        assert_eq!(candidates(&one, &top_dir(&one), ""), [ours("r-main/")]);
        // dsh-pet 仓库本体 + 自己做的种类；node_modules 里的不算，没有动画文件夹的配置不算
        let dsh = n(&[
            "d-main/README.md",
            "d-main/dsh-pet/assets/config.jsonc",
            "d-main/dsh-pet/assets/webm/待机.webm",
            "d-main/dsh-pet/node_modules/x/assets/config.jsonc",
            "d-main/dsh-pet/node_modules/x/assets/webm/a.webm",
            "d-main/pet/猫-config.json",
            "d-main/pet/猫-animation/待机.webm",
            "d-main/pet/dog-config.json",
            "d-main/pet/dog-animation/idle.webm",
            "d-main/pet/lonely-config.json",
        ]);
        let t = top_dir(&dsh);
        assert_eq!(
            candidates(&dsh, &t, ""),
            [Cand::DshPackage("d-main/dsh-pet/".into()), Cand::DshKind("d-main/pet/".into(), "dog".into()), Cand::DshKind("d-main/pet/".into(), "猫".into())]
        );
        assert_eq!(candidates(&dsh, &t, "dsh-pet"), [Cand::DshPackage("d-main/dsh-pet/".into())]);
        assert_eq!(ascii_id("dog", "dsh-"), "dog");
        assert_eq!(ascii_id("dsh-pet", "dsh-"), "dsh-pet");
        assert!(ascii_id("猫", "dsh-").starts_with("dsh-"));
        assert_eq!(ascii_id("猫", "dsh-"), ascii_id("猫", "dsh-"));
    }

    #[test]
    fn multi_pet_repo() {
        use crate::pets::tests::{min_pack, write, zip_dir, MIN};
        let home = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        write(src.path(), "README.md", b"# pets");
        write(src.path(), "tools/BIG.bin", &vec![0u8; 9 << 20]);
        min_pack(&src.path().join("pets/a"));
        min_pack(&src.path().join("pets/b"));
        write(&src.path().join("pets/b"), "pet.json", MIN.replace(r#""id": "t""#, r#""id": "b""#).as_bytes());
        write(&src.path().join("pets/broken"), "pet.json", b"{}");
        let z = home.path().join("repo.zip");
        zip_dir(src.path(), &z, "pets-main/");
        let repo = parse("o/pets").unwrap();
        let stage_at = |n: &str| home.path().join(n);
        let found = stage(&z, "", &stage_at("s1")).unwrap();
        let paths: Vec<&str> = found.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["pets/a", "pets/b", "pets/broken"], "仓库里别的大文件不解、不报错");
        assert!(found[2].pack.is_err());
        // 只装 b
        let got = install(home.path(), &repo, found, &["b".into()]).unwrap();
        assert_eq!(got.len(), 1);
        assert!(home.path().join("pets/b/anim/idle.webm").is_file());
        assert!(!home.path().join("pets/t").exists());
        let summary = crate::pets::find(home.path(), "local", "b").unwrap().summary();
        assert_eq!(summary["origin"]["github"], "o/pets");
        assert_eq!(summary["origin"]["path"], "pets/b");
        // 全装：坏的跳过
        let found = stage(&z, "", &stage_at("s2")).unwrap();
        let got = install(home.path(), &repo, found, &[]).unwrap();
        assert_eq!(got.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["t", "b"]);
        // 只看一个文件夹
        assert_eq!(stage(&z, "pets/a", &stage_at("s3")).unwrap().len(), 1);
        assert_eq!(stage(&z, "nope", &stage_at("s4")).unwrap_err().code, "no_pets");
        // 设置页那条路：scan 留下的目录 → 按序号装
        let tok = "0123456789abcdef";
        let d = stage_dir(home.path(), tok).unwrap();
        stage(&z, "", &d.join("p")).unwrap();
        std::fs::write(d.join("repo.json"), serde_json::to_vec(&repo).unwrap()).unwrap();
        assert!(staged_preview(home.path(), tok, 0).is_some());
        let got = install_staged(home.path(), tok, &[0]).unwrap();
        assert_eq!(got[0].id, "t");
        assert!(!d.exists(), "装完清掉");
        assert_eq!(install_staged(home.path(), tok, &[0]).unwrap_err().code, "expired");
        // 单仓库单宠物（pet.json 在根目录）
        let one = tempfile::tempdir().unwrap();
        min_pack(one.path());
        zip_dir(one.path(), &z, "solo-v1/");
        assert_eq!(stage(&z, "", &stage_at("s5")).unwrap()[0].path, "");
    }

    #[test]
    fn dsh_repo() {
        use crate::pets::tests::{write, zip_dir};
        let home = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let pkg = src.path().join("dsh-pet");
        write(&pkg, "assets/config.jsonc", r#"{ "pets": [{ "name": "大肥鱼" }], "animations": { "idle": ["待机"], "clicks": ["点击"] }, }"#.as_bytes());
        write(&pkg, "assets/webm/待机.webm", b"x");
        write(&pkg, "assets/webm/点击.webm", b"x");
        write(&pkg, "assets/preview/big.gif", &vec![0u8; 9 << 20]);
        write(&pkg, "src/index.ts", b"//");
        write(src.path(), "pet/猫-config.json", r#"{ "animations": { "idle": ["趴"] } }"#.as_bytes());
        write(src.path(), "pet/猫-animation/趴.webm", b"x");
        let z = home.path().join("r.zip");
        zip_dir(src.path(), &z, "dsh-pet-main/");
        let found = stage(&z, "", &home.path().join("s")).unwrap();
        assert_eq!(found.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["dsh-pet", "pet/猫"]);
        let main = found[0].pack.as_ref().unwrap();
        assert_eq!(main.id, "dsh-pet");
        assert_eq!(main.manifest.clips.len(), 2);
        assert_eq!(main.manifest.license.as_ref().unwrap().commercial, Some(false));
        assert!(!found[0].root.join("assets/preview").exists() && !found[0].root.join("src").exists(), "只解配置和动画");
        let cat = found[1].pack.as_ref().unwrap();
        assert!(cat.id.starts_with("dsh-"));
        let got = install(home.path(), &parse("PC2005-cloud/dsh-pet").unwrap(), found, &[]).unwrap();
        assert_eq!(got.len(), 2);
        // 装好后照常扫得到、文件能取
        let p = crate::pets::find(home.path(), "local", "dsh-pet").unwrap();
        assert!(p.file("anim/待机.webm").unwrap().is_file());
        assert!(matches!(&p.manifest.name, Some(crate::pets::Text::One(n)) if n == "大肥鱼"));
    }
}
