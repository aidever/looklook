//! 项目的“运行”按钮（▶）：照 Codex 桌面版的“本地环境 → 操作（actions）”做。
//!
//! - 每个项目（文件夹）可以有几条运行命令（`npm run dev`、`cargo run`…），点 ▶ 在这个项目里一个专门的终端会话里跑，
//!   能看到在不在跑、能停、能重跑；跑完终端留着（`命令; exec shell`），输出还在。
//! - 命令从两处来：看看自己记的（kv `run_actions.{文件夹}`），和项目里 Codex 的 `.codex/environments/*.toml`
//!   的 `[[actions]]`（只读，Codex 里配好的直接能用）。
//! - 一条命令对应一个会话（kv `run_action_inst.{文件夹}\t{key}` → 会话 ID），重跑就是重启这个会话；会话被删了下次新建。
//! - 没配时按项目里的文件猜几条（package.json 的 scripts、Cargo.toml、Makefile…），只列出来让用户点“添加”，不会自己运行。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::LocalError;
use crate::store::Store;

/// 每个项目最多记几条
const MAX_ACTIONS: usize = 20;
/// 命令最长多少个字符（多行的写成脚本文件再跑）
const COMMAND_MAX: usize = 8000;
const NAME_MAX: usize = 30;

/// 看看自己记的一条命令
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Saved {
    pub key: String,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub icon: String,
}

/// 界面上的一条（看看记的或 Codex 的）
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub name: String,
    pub command: String,
    /// Codex 的图标名（`run`、`test`、`debug`、`tool`…），没写时空
    pub icon: String,
    /// `looklook` | `codex`
    pub source: &'static str,
    /// 在哪个文件夹里跑：看看记的是项目文件夹；Codex 的是 `.codex` 所在的文件夹（Codex 从那里跑）
    pub cwd: String,
}

/// 按项目里的文件猜的命令（只给用户挑，不会自己运行）
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub command: String,
    /// 从哪个文件看出来的（`package.json`…）
    pub from: String,
}

pub fn saved_key(workdir: &str) -> String {
    format!("run_actions.{workdir}")
}

pub fn binding_key(workdir: &str, key: &str) -> String {
    format!("run_action_inst.{workdir}\t{key}")
}

/// 会话是哪条运行命令开的（删会话时清掉）
pub fn mark_key(id: &str) -> String {
    format!("instance_run.{id}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mark {
    pub workdir: String,
    pub key: String,
}

pub fn check_workdir(workdir: &str) -> Result<(), LocalError> {
    let p = Path::new(workdir);
    if workdir.is_empty() || !p.is_absolute() || !p.is_dir() {
        return Err(LocalError::new("WORKDIR_NOT_FOUND").with("path", workdir));
    }
    Ok(())
}

pub fn saved(store: &Store, workdir: &str) -> Vec<Saved> {
    store.get::<Vec<Saved>>(&saved_key(workdir)).ok().flatten().unwrap_or_default()
}

/// 保存看看自己的命令：名字、命令都要有；没有 key 的（新加的）给一个
pub fn save(store: &Store, workdir: &str, list: Vec<Saved>) -> Result<Vec<Saved>, LocalError> {
    if list.len() > MAX_ACTIONS {
        return Err(LocalError::invalid("actions", "too_many"));
    }
    let mut out: Vec<Saved> = Vec::with_capacity(list.len());
    for mut a in list {
        a.name = a.name.trim().chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
        a.command = a.command.replace("\r\n", "\n").trim().to_string();
        a.icon = a.icon.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(20).collect();
        if a.name.is_empty() {
            return Err(LocalError::invalid("name", "required"));
        }
        if a.command.is_empty() {
            return Err(LocalError::invalid("command", "required"));
        }
        if a.command.chars().count() > COMMAND_MAX || a.command.contains('\0') {
            return Err(LocalError::invalid("command", "too_long"));
        }
        let ok_key = a.key.starts_with("ll:") && a.key.len() <= 24 && a.key[3..].chars().all(|c| c.is_ascii_alphanumeric());
        if !ok_key || out.iter().any(|b| b.key == a.key) {
            a.key = format!("ll:{}", crate::util::random_id(8));
        }
        out.push(a);
    }
    if out.is_empty() {
        store.remove(&saved_key(workdir))?;
    } else {
        store.set(&saved_key(workdir), &out)?;
    }
    Ok(out)
}

/// 这个项目的全部命令：看看记的在前，Codex 的在后。第二个是读到的 Codex 配置文件（界面上告诉用户去哪改）。
pub fn actions(store: &Store, workdir: &str) -> (Vec<Action>, Vec<String>) {
    let mut out: Vec<Action> = saved(store, workdir)
        .into_iter()
        .map(|s| Action { key: s.key, name: s.name, command: s.command, icon: s.icon, source: "looklook", cwd: workdir.to_string() })
        .collect();
    let (files, codex) = codex_actions(Path::new(workdir));
    out.extend(codex);
    (out, files.iter().map(|p| p.display().to_string()).collect())
}

pub fn find(store: &Store, workdir: &str, key: &str) -> Option<Action> {
    actions(store, workdir).0.into_iter().find(|a| a.key == key)
}

// ---------------- Codex 的 .codex/environments ----------------

/// Codex 的本地环境文件：`.codex/environments/*.toml`（Codex 自己写的是 `environment.toml`）。
/// 先找项目文件夹，没有时找它所在的 git 仓库根目录（Codex 的项目一般是仓库根）。
fn codex_env_dir(workdir: &Path) -> Option<PathBuf> {
    let mut dir = Some(workdir);
    while let Some(d) = dir {
        let env = d.join(".codex").join("environments");
        if env.is_dir() {
            return Some(env);
        }
        // 到仓库根为止，不往上乱找（主目录里的 ~/.codex 是 Codex 自己的配置）
        if d.join(".git").exists() {
            return None;
        }
        dir = d.parent();
    }
    None
}

fn codex_actions(workdir: &Path) -> (Vec<PathBuf>, Vec<Action>) {
    let Some(dir) = codex_env_dir(workdir) else { return (vec![], vec![]) };
    // `.codex` 所在的文件夹：Codex 在这里跑命令
    let root = dir.parent().and_then(Path::parent).unwrap_or(workdir).display().to_string();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file()).collect())
        .unwrap_or_default();
    // environment.toml 在前，其余按名字
    files.sort_by_key(|p| (p.file_stem().is_none_or(|s| s != "environment"), p.clone()));
    files.truncate(10);
    let mut out: Vec<Action> = vec![];
    let mut used: Vec<PathBuf> = vec![];
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        if text.len() > 256 * 1024 {
            continue;
        }
        let stem = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let parsed = parse_codex(&text, current_platform());
        if parsed.is_empty() {
            continue;
        }
        used.push(f.clone());
        for (name, icon, command) in parsed {
            let mut key = format!("codex:{stem}:{name}");
            let mut n = 2;
            while out.iter().any(|a| a.key == key) {
                key = format!("codex:{stem}:{name}#{n}");
                n += 1;
            }
            out.push(Action { key, name, command, icon, source: "codex", cwd: root.clone() });
        }
    }
    (used, out)
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "linux"
    }
}

/// `platform` 的几种写法都认（Codex 写的是 `darwin`；文档说的是 macOS / Windows / Linux）
fn platform_matches(want: &str, current: &str) -> bool {
    let norm = |s: &str| match s.trim().to_ascii_lowercase().as_str() {
        "darwin" | "macos" | "mac" | "osx" => "darwin",
        "win32" | "windows" | "win" => "win32",
        "linux" => "linux",
        _ => "?",
    };
    norm(want) == norm(current)
}

/// 解析 Codex 的环境文件，返回这台电脑能用的 `(名字, 图标, 命令)`。
/// ```toml
/// version = 1
/// name = "my-app"
/// [setup]
/// script = ""
/// [[actions]]
/// name = "Run"
/// icon = "run"
/// command = "npm run dev"
/// platform = "darwin"   # 可选：只在这个系统上
/// ```
/// 同名的命令有通用的也有本系统专用的时，用本系统的。
fn parse_codex(text: &str, current: &str) -> Vec<(String, String, String)> {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return vec![] };
    let Some(item) = doc.get("actions") else { return vec![] };
    let mut tables: Vec<(Option<String>, Option<String>, Option<String>, Option<String>)> = vec![];
    let field = |v: Option<&toml_edit::Item>| v.and_then(|i| i.as_str()).map(str::to_string);
    if let Some(arr) = item.as_array_of_tables() {
        for t in arr.iter() {
            tables.push((field(t.get("name")), field(t.get("icon")), field(t.get("command")), field(t.get("platform"))));
        }
    } else if let Some(arr) = item.as_array() {
        for v in arr.iter() {
            if let Some(t) = v.as_inline_table() {
                let f = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
                tables.push((f("name"), f("icon"), f("command"), f("platform")));
            }
        }
    }
    let mut out: Vec<(String, String, String, bool)> = vec![];
    for (name, icon, command, platform) in tables {
        let Some(command) = command.map(|c| c.replace("\r\n", "\n").trim().to_string()).filter(|c| !c.is_empty()) else { continue };
        let specific = match platform.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) if !platform_matches(p, current) => continue,
            Some(_) => true,
            None => false,
        };
        let name: String = name.unwrap_or_default().trim().chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
        let name = if name.is_empty() { "Run".to_string() } else { name };
        let icon = icon.unwrap_or_default().trim().to_string();
        match out.iter_mut().find(|x| x.0 == name) {
            // 同名：本系统专用的替换通用的
            Some(x) if specific && !x.3 => *x = (name, icon, command, true),
            Some(_) => {}
            None => out.push((name, icon, command, specific)),
        }
    }
    out.into_iter().map(|(n, i, c, _)| (n, i, c)).collect()
}

// ---------------- 猜命令 ----------------

/// 按项目里的文件猜几条常用命令。只是建议：界面上列出来，用户点了“添加”才记下，点了 ▶ 才运行。
pub fn suggest(workdir: &Path) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = vec![];
    let mut add = |name: &str, command: String, from: &str| {
        if out.len() < 8 && !out.iter().any(|s| s.command == command) {
            out.push(Suggestion { name: name.to_string(), command, from: from.to_string() });
        }
    };
    let has = |f: &str| workdir.join(f).exists();
    if has("script/build_and_run.sh") {
        add("Run", "./script/build_and_run.sh".into(), "script/build_and_run.sh");
    }
    if let Some(scripts) = package_scripts(workdir) {
        let pm = if has("pnpm-lock.yaml") {
            "pnpm"
        } else if has("yarn.lock") {
            "yarn"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun"
        } else {
            "npm"
        };
        for s in ["dev", "start", "serve", "preview", "watch", "test", "build"] {
            if scripts.iter().any(|x| x == s) {
                let cmd = match pm {
                    "yarn" => format!("yarn {s}"),
                    "npm" if s == "start" || s == "test" => format!("npm {s}"),
                    _ => format!("{pm} run {s}"),
                };
                add(s, cmd, "package.json");
            }
        }
    }
    if has("Cargo.toml") {
        if has("src/main.rs") || has("src/bin") {
            add("run", "cargo run".into(), "Cargo.toml");
        }
        add("test", "cargo test".into(), "Cargo.toml");
    }
    if has("go.mod") {
        if has("main.go") {
            add("run", "go run .".into(), "go.mod");
        }
        add("test", "go test ./...".into(), "go.mod");
    }
    if has("manage.py") {
        let py = if cfg!(windows) { "python" } else { "python3" };
        add("runserver", format!("{py} manage.py runserver"), "manage.py");
    }
    for f in ["Makefile", "makefile", "GNUmakefile"] {
        let Ok(text) = std::fs::read_to_string(workdir.join(f)) else { continue };
        for t in ["run", "dev", "start", "serve", "test"] {
            if text.lines().any(|l| l.strip_prefix(t).is_some_and(|r| r.starts_with(':') && !r.starts_with(":="))) {
                add(t, format!("make {t}"), f);
            }
        }
        break;
    }
    for f in ["compose.yaml", "compose.yml", "docker-compose.yml", "docker-compose.yaml"] {
        if has(f) {
            add("compose", "docker compose up".into(), f);
            break;
        }
    }
    out
}

/// package.json 里 scripts 的名字
fn package_scripts(workdir: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(workdir.join("package.json")).ok()?;
    if text.len() > 2 * 1024 * 1024 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(v.get("scripts")?.as_object()?.keys().cloned().collect())
}

// ---------------- 跑起来 ----------------

/// 会话里要跑的那一行命令。一行的原样用；多行的（Codex 的 `'''…'''`）写成脚本文件
/// （`{数据目录}/run-actions/{哈希}.sh`，Windows 上是 `.ps1`），命令是运行这个脚本。
pub fn command_line(home: &Path, workdir: &str, a: &Action) -> Result<String, LocalError> {
    let cmd = a.command.trim();
    if !cmd.contains('\n') && cmd.len() <= 1800 {
        return Ok(cmd.to_string());
    }
    let dir = home.join("run-actions");
    std::fs::create_dir_all(&dir)?;
    let name = format!("{:016x}", fnv(&format!("{workdir}\t{}", a.key)));
    if cfg!(windows) {
        let path = dir.join(format!("{name}.ps1"));
        std::fs::write(&path, cmd)?;
        return Ok(format!("& '{}'", path.display().to_string().replace('\'', "''")));
    }
    let path = dir.join(format!("{name}.sh"));
    // 和 Codex 一样按 bash 跑；没有 bash 用 sh
    let sh = if crate::shells::which("bash").is_some() { "bash" } else { "sh" };
    std::fs::write(&path, format!("#!/usr/bin/env {sh}\n{cmd}\n"))?;
    Ok(format!("{sh} '{}'", path.display().to_string().replace('\'', "'\\''")))
}

/// 按 Ctrl+C（或“停止”）停下命令后终端留着、输出还在：会话里是 `shell -c '命令; exec shell'`，
/// 命令被 Ctrl+C 打断时 shell 也收到 SIGINT，后面的 `exec shell` 不跑，整个会话就没了。
/// 前面加 `trap true INT`：shell 自己不因 Ctrl+C 退出，命令照样被打断（trap 了的信号在子进程里恢复默认）。
/// 只对 bash、zsh、sh 这类加；fish、PowerShell、cmd 原样。
pub fn keep_on_interrupt(line: &str, shell: Option<&crate::shells::Shell>) -> String {
    let posix = shell.is_some_and(|s| {
        let name = Path::new(&s.program).file_stem().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        s.family == crate::shells::Family::Posix && name != "fish"
    });
    if posix {
        format!("trap true INT; {line}")
    } else {
        line.to_string()
    }
}

fn fnv(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

/// 会话名：`▶ dev`
pub fn session_name(a: &Action) -> String {
    format!("▶ {}", a.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_environment() {
        let text = r#"
# THIS IS AUTOGENERATED. DO NOT EDIT MANUALLY
version = 1
name = "demo"

[setup]
script = "npm install"

[[actions]]
name = "Run"
icon = "run"
command = "./script/build_and_run.sh"

[[actions]]
name = "Run"
icon = "run"
command = "open -a Demo"
platform = "darwin"

[[actions]]
name = "Test"
icon = "test"
command = '''
npm test
npm run lint
'''

[[actions]]
name = "Mac only"
command = "say hi"
platform = "darwin"
"#;
        let linux = parse_codex(text, "linux");
        assert_eq!(
            linux,
            vec![
                ("Run".into(), "run".into(), "./script/build_and_run.sh".into()),
                ("Test".into(), "test".into(), "npm test\nnpm run lint".into()),
            ]
        );
        let mac = parse_codex(text, "darwin");
        assert_eq!(mac[0], ("Run".into(), "run".into(), "open -a Demo".into()));
        assert_eq!(mac.len(), 3);
        assert!(parse_codex("not toml [[[", "linux").is_empty());
        assert!(parse_codex("version = 1", "linux").is_empty());
        // 内联数组的写法也认
        let inline = parse_codex(r#"actions = [{ name = "Dev", command = "npm run dev" }]"#, "linux");
        assert_eq!(inline, vec![("Dev".into(), String::new(), "npm run dev".into())]);
    }

    #[test]
    fn platform_aliases() {
        assert!(platform_matches("macOS", "darwin"));
        assert!(platform_matches("windows", "win32"));
        assert!(!platform_matches("linux", "darwin"));
        assert!(!platform_matches("beos", "linux"));
    }

    #[test]
    fn suggests_from_project_files() {
        let dir = std::env::temp_dir().join(format!("ll-run-suggest-{}", crate::util::random_id(6)));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"build":"vite build","dev":"vite","lint":"eslint ."}}"#).unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(dir.join("Makefile"), "VAR := 1\nrun: build\n\t./app\ntest:\n\tgo test\n").unwrap();
        let s = suggest(&dir);
        let cmds: Vec<&str> = s.iter().map(|x| x.command.as_str()).collect();
        assert_eq!(cmds, ["pnpm run dev", "pnpm run build", "make run", "make test"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_codex_file_at_repo_root() {
        let dir = std::env::temp_dir().join(format!("ll-run-codex-{}", crate::util::random_id(6)));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".codex/environments")).unwrap();
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::write(dir.join(".codex/environments/environment.toml"), "version = 1\n[[actions]]\nname = \"Dev\"\ncommand = \"npm run dev\"\n").unwrap();
        let (files, got) = codex_actions(&dir.join("web"));
        assert_eq!(files.len(), 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "codex:environment:Dev");
        assert_eq!(got[0].cwd, dir.display().to_string());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn multi_line_becomes_script() {
        let home = std::env::temp_dir().join(format!("ll-run-home-{}", crate::util::random_id(6)));
        let a = Action { key: "ll:x".into(), name: "t".into(), command: "echo a\necho b".into(), icon: String::new(), source: "looklook", cwd: "/tmp".into() };
        let line = command_line(&home, "/tmp", &a).unwrap();
        assert!(!line.contains('\n'));
        let one = Action { command: "npm run dev".into(), ..a };
        assert_eq!(command_line(&home, "/tmp", &one).unwrap(), "npm run dev");
        let _ = std::fs::remove_dir_all(&home);
    }
}
