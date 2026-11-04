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
        .timeout(Duration::from_secs(600))
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

/// zip 里每个带 `pet.json` 的文件夹（完整前缀，以 `/` 结尾或为空）；只要 `under` 下面的，套在别的宠物文件夹里的不算
fn candidates(names: &[String], top: &str, under: &str) -> Vec<String> {
    let want = if under.is_empty() { top.to_string() } else { format!("{top}{under}/") };
    let mut dirs: Vec<String> = names
        .iter()
        .filter_map(|n| if n == "pet.json" { Some(String::new()) } else { n.strip_suffix("/pet.json").map(|d| format!("{d}/")) })
        .filter(|d| d.starts_with(&want) && !d.contains("/__MACOSX/") && !d.split('/').any(|s| s.starts_with('.') && !s.is_empty()))
        .collect();
    dirs.sort();
    dirs.dedup();
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        if !out.iter().any(|o| d.starts_with(o.as_str())) {
            out.push(d);
        }
    }
    out
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

/// 解出仓库 zip 里的每只宠物到 `stage/<序号>/<文件夹名>/` 并校验
pub fn stage(zip_path: &Path, under: &str, stage: &Path) -> Result<Vec<Found>, PackError> {
    let f = std::fs::File::open(zip_path).map_err(|_| perr("bad_zip", ""))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| perr("bad_zip", e.to_string()))?;
    let names: Vec<String> = z.file_names().map(str::to_string).collect();
    let top = top_dir(&names);
    let dirs = candidates(&names, &top, under);
    if dirs.is_empty() {
        return Err(perr("no_pets", under));
    }
    if dirs.len() > MAX_PETS {
        return Err(perr("too_many_pets", dirs.len().to_string()));
    }
    let mut out = Vec::new();
    for (i, d) in dirs.iter().enumerate() {
        let path = d.strip_prefix(&top).unwrap_or(d).trim_end_matches('/').to_string();
        // Codex 宠物没写 id 时拿文件夹名当 id，所以解到同名文件夹里
        let leaf = path.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("pet");
        let leaf: String = leaf.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let root = stage.join(i.to_string()).join(if leaf.is_empty() || leaf.starts_with('.') { "pet".into() } else { leaf });
        std::fs::create_dir_all(&root).map_err(|e| perr("io", e.to_string()))?;
        let pack = pets::extract(&mut z, d, &root).and_then(|_| pets::load_dir("local", &root));
        std::fs::write(stage.join(format!("{i}.path")), &path).map_err(|e| perr("io", e.to_string()))?;
        out.push(Found { path, pack, root });
    }
    Ok(out)
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
        assert_eq!(candidates(&names, &top, ""), ["r-main/pets/a/", "r-main/pets/b/"]);
        assert_eq!(candidates(&names, &top, "pets/b"), ["r-main/pets/b/"]);
        let one = n(&["r-main/", "r-main/pet.json", "r-main/anim/idle.webm"]);
        assert_eq!(candidates(&one, &top_dir(&one), ""), ["r-main/"]);
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
}
