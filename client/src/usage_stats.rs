//! 用量统计：定时扫 Claude Code 和 Codex 的会话记录，按小时、助手、模型累计 token，按 models.dev 的
//! API 价格（[`crate::pricing`]）估算费用。订阅用户看到的是“按 API 价格折算”的数，界面上写明。
//!
//! - **Claude Code**：`<配置目录>/projects/**/*.jsonl`（另外登录的官方账号的 `projects/` 链到同一处），
//!   `assistant` 记录的 `message.usage`。同一条回复会写好几行（每个内容块一行），按 `message.id` + `requestId` 只算一次。
//! - **Codex**：`$CODEX_HOME/sessions/**/rollout-*.jsonl` 和 `archived_sessions/`（旧的会被 Codex 压成 `.jsonl.zst`，也读）。`token_count` 事件的 `last_token_usage`
//!   是一次请求的用量；`total_token_usage` 没变的是重复发的，不算。模型取前面最近的 `turn_context`，
//!   走的哪家取开头 `session_meta` 的 `model_provider`（看看的第三方接口是 `looklook-<配置 ID>`，见 codex_profiles.rs）。
//! - 接着以前的对话、分叉出来的对话会把以前的记录抄进新文件：每条用量记一个指纹，别的文件算过的不再算。
//! - 只重新读变过（长度或修改时间变了）的文件。删掉的会话记录（Claude Code 默认 30 天清理）统计照样留着。
//! - 每个文件另外记下工作目录（`cwd`）和标题（Claude Code 的 `custom-title` > `ai-title`；Codex 的在 `session_index.jsonl`），
//!   用来按项目、按会话统计。
//! - 结果存在 `<数据目录>/usage/stats.json`，只在本机，不上传。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const SCAN_EVERY: Duration = Duration::from_secs(10 * 60);
const VERSION: u32 = 1;
/// 每个文件解析的版本：旧版本解析的文件还在时重读一遍；删掉的就留着旧的。
/// 2：记工作目录、标题；3：长提示词、快速模式、联网搜索、顾问（advisor）的用量分开记
const PARSE_V: u32 = 3;
/// 长提示词按提示词大小分几档（千 token）：价格表里长提示词那一档的门槛（Haiku 5.5 是 10 万，models.dev 多是 20 万、GPT 是 27.2 万）
const PROMPT_TIERS: [u32; 3] = [100, 200, 272];
/// 会话表最多列多少个
const TOP_SESSIONS: usize = 50;

/// 一个小时里一个模型的用量
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Row {
    /// 第几个小时（Unix 秒 / 3600，UTC）
    h: i64,
    m: String,
    /// 不含缓存的输入
    #[serde(default)]
    i: u64,
    #[serde(default)]
    o: u64,
    /// 读缓存
    #[serde(default)]
    cr: u64,
    /// 写缓存（5 分钟）
    #[serde(default)]
    cw: u64,
    /// 写缓存（1 小时，价格是输入的两倍）
    #[serde(default)]
    c1: u64,
    /// 请求数
    #[serde(default)]
    n: u64,
    /// 这些请求的提示词（输入 + 读写缓存）至少多少千 token（[`PROMPT_TIERS`] 里的一档，0 是不到最小那档）：长提示词另算价钱
    #[serde(default, skip_serializing_if = "is_zero")]
    x: u32,
    /// 快速模式（Claude Code `usage.speed == "fast"`）：价钱是平时的几倍（[`fast_rate`]）
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    f: bool,
    /// 联网搜索次数（按次收费）
    #[serde(default, skip_serializing_if = "is_zero64")]
    w: u64,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero64(n: &u64) -> bool {
    *n == 0
}

/// 提示词 token 数 → 档（[`Row::x`]）
fn prompt_tier(prompt: u64) -> u32 {
    PROMPT_TIERS.iter().rev().copied().find(|&t| prompt >= u64::from(t) * 1000).unwrap_or(0)
}

/// 快速模式是平时价钱的几倍：同 Claude Code（2.1.321）自己算钱的表——Opus 4.6 / 4.7 是 6 倍，
/// 之后的 Opus 4.8、5、5.5 和 Fable / Mythos 5.2 是 2 倍；别的模型没有快速模式的价，按平时算
fn fast_rate(model: &str) -> f64 {
    let m = model.trim().to_ascii_lowercase();
    let m = m.split('[').next().unwrap_or(&m);
    match m {
        _ if m.starts_with("claude-opus-4-6") || m.starts_with("claude-opus-4-7") => 6.0,
        _ if ["claude-opus-4-8", "claude-opus-5", "claude-fable-5-2", "claude-mythos-5-2"].iter().any(|p| m.starts_with(p)) => 2.0,
        _ => 1.0,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct FileState {
    len: u64,
    mtime: i64,
    agent: String,
    /// Codex 的 `model_provider`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    rows: Vec<Row>,
    /// 这个文件算过的用量的指纹
    ids: Vec<u64>,
    /// 解析的版本（[`PARSE_V`]）
    #[serde(default)]
    v: u32,
    /// 工作目录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// 会话标题（Claude Code 写在记录里的）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Codex 的会话号（`session_meta` 的 `id`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sid: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    version: u32,
    #[serde(default)]
    scanned_at: String,
    files: BTreeMap<String, FileState>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// 同时只扫一遍
static SCANNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn state_file(home: &Path) -> PathBuf {
    home.join("usage").join("stats.json")
}

fn fingerprint(parts: &[&str]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
}

fn hour_of(ts: &str) -> Option<i64> {
    time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339).ok().map(|t| t.unix_timestamp().div_euclid(3600))
}

#[derive(Default)]
struct Parsed {
    provider: Option<String>,
    cwd: Option<String>,
    title: Option<String>,
    sid: Option<String>,
    rows: HashMap<(i64, String, u32, bool), Row>,
    ids: Vec<u64>,
}

impl Parsed {
    fn add(&mut self, h: i64, model: &str, r: Row) {
        let x = prompt_tier(r.i + r.cr + r.cw + r.c1);
        let e = self.rows.entry((h, model.to_string(), x, r.f)).or_insert_with(|| Row { h, m: model.to_string(), x, f: r.f, ..Default::default() });
        e.i += r.i;
        e.o += r.o;
        e.cr += r.cr;
        e.cw += r.cw;
        e.c1 += r.c1;
        e.w += r.w;
        e.n += 1;
    }
}

fn n(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}

/// Claude Code 的会话记录。`seen` 是别的文件算过的指纹
fn parse_claude(text: &str, seen: &HashSet<u64>) -> Parsed {
    // 同一条回复的几行：留输出最多的那行（流式写入时前面的行可能还不全）
    let mut best: HashMap<u64, (i64, String, Value)> = HashMap::new();
    let (mut cwd, mut custom, mut ai) = (None, None, None);
    for line in text.lines() {
        // 标题：`/rename` 起的优先，同一种以最后一行为准
        if line.contains("\"custom-title\"") || line.contains("\"ai-title\"") {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                match v["type"].as_str() {
                    Some("custom-title") => custom = v["customTitle"].as_str().map(String::from).or(custom),
                    Some("ai-title") => ai = v["aiTitle"].as_str().map(String::from).or(ai),
                    _ => {}
                }
            }
            continue;
        }
        if !(line.contains("\"usage\"") && line.contains("\"assistant\"")) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if cwd.is_none() {
            cwd = v["cwd"].as_str().filter(|s| !s.is_empty()).map(String::from);
        }
        if v["type"] != "assistant" {
            continue;
        }
        let m = &v["message"];
        let (Some(model), Some(h)) = (m["model"].as_str(), v["timestamp"].as_str().and_then(hour_of)) else { continue };
        if model.starts_with('<') || !m["usage"].is_object() {
            continue;
        }
        let id = match m["id"].as_str() {
            Some(id) => fingerprint(&[id, v["requestId"].as_str().unwrap_or("")]),
            None => fingerprint(&[line]),
        };
        if seen.contains(&id) {
            continue;
        }
        let u = &m["usage"];
        if best.get(&id).is_none_or(|(_, _, old)| n(u, "output_tokens") >= n(old, "output_tokens")) {
            best.insert(id, (h, model.to_string(), u.clone()));
        }
    }
    let mut out = Parsed { cwd, title: custom.or(ai).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()), ..Default::default() };
    let row = |u: &Value| {
        let c1 = n(&u["cache_creation"], "ephemeral_1h_input_tokens");
        let cw = n(u, "cache_creation_input_tokens").saturating_sub(c1);
        Row { i: n(u, "input_tokens"), o: n(u, "output_tokens"), cr: n(u, "cache_read_input_tokens"), cw, c1, ..Default::default() }
    };
    for (id, (h, model, u)) in best {
        let fast = u["speed"] == "fast";
        out.add(h, &model, Row { f: fast, w: n(&u["server_tool_use"], "web_search_requests"), ..row(&u) });
        // 顾问（advisor 工具）：用的是另一个模型，用量单独记在 iterations 里，不含在上面的数里（Claude Code 也是另算钱）
        for it in u["iterations"].as_array().into_iter().flatten().filter(|it| it["type"] == "advisor_message") {
            if let Some(m) = it["model"].as_str().filter(|m| !m.is_empty()) {
                out.add(h, m, row(it));
            }
        }
        out.ids.push(id);
    }
    out
}

/// Codex 的会话记录
fn parse_codex(text: &str, seen: &HashSet<u64>) -> Parsed {
    let mut out = Parsed::default();
    let mut model = String::from("unknown");
    let mut last_total = None;
    for line in text.lines() {
        let kind = if line.contains("\"token_count\"") {
            "token_count"
        } else if line.contains("\"turn_context\"") {
            "turn_context"
        } else if line.contains("\"session_meta\"") {
            "session_meta"
        } else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let p = &v["payload"];
        match kind {
            "session_meta" if v["type"] == "session_meta" => {
                if out.provider.is_none() {
                    out.provider = p["model_provider"].as_str().map(String::from);
                    out.cwd = p["cwd"].as_str().filter(|s| !s.is_empty()).map(String::from);
                    out.sid = p["id"].as_str().map(String::from);
                }
            }
            "turn_context" if v["type"] == "turn_context" => {
                if let Some(m) = p["model"].as_str() {
                    model = m.to_string();
                }
            }
            "token_count" if p["type"] == "token_count" => {
                let info = &p["info"];
                let total = info["total_token_usage"]["total_tokens"].as_u64();
                if total.is_none() || total == last_total {
                    continue;
                }
                last_total = total;
                let (Some(h), Some(ts)) = (v["timestamp"].as_str().and_then(hour_of), v["timestamp"].as_str()) else { continue };
                let id = fingerprint(&[ts, &total.unwrap_or(0).to_string()]);
                if seen.contains(&id) {
                    continue;
                }
                let u = &info["last_token_usage"];
                let cached = n(u, "cached_input_tokens");
                out.add(h, &model, Row { i: n(u, "input_tokens").saturating_sub(cached), o: n(u, "output_tokens"), cr: cached, cw: n(u, "cache_write_input_tokens"), ..Default::default() });
                out.ids.push(id);
            }
            _ => {}
        }
    }
    out
}

fn walk(dir: &Path, want: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        // 软链接的目录（另外的账号目录里的 projects/）不跟进去：同一处只扫一遍
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() && depth < 8 {
            walk(&p, want, out, depth + 1);
        } else if ft.is_file() && want(&p) {
            out.push(p);
        }
    }
}

fn sources() -> Vec<(PathBuf, &'static str)> {
    let jsonl = |p: &Path| p.extension().is_some_and(|e| e == "jsonl");
    let mut out = vec![];
    let mut files = vec![];
    walk(&crate::chat::models::config_dir().join("projects"), &jsonl, &mut files, 0);
    out.extend(files.drain(..).map(|p| (p, "claude")));
    let codex = crate::agents::codex_home();
    for d in ["sessions", "archived_sessions"] {
        // 新版 Codex 会把旧的会话记录压成 `rollout-*.jsonl.zst`（原来的 `.jsonl` 删掉）
        let rollout = |p: &Path| p.file_name().is_some_and(|n| {
            let n = n.to_string_lossy();
            n.starts_with("rollout-") && (n.ends_with(".jsonl") || n.ends_with(".jsonl.zst"))
        });
        walk(&codex.join(d), &rollout, &mut files, 0);
    }
    out.extend(files.into_iter().map(|p| (p, "codex")));
    out
}

fn mtime_of(m: &std::fs::Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn load(home: &Path) -> State {
    std::fs::read(state_file(home)).ok().and_then(|b| serde_json::from_slice::<State>(&b).ok()).filter(|s| s.version == VERSION).unwrap_or(State { version: VERSION, ..Default::default() })
}

/// 扫一遍（阻塞，放在 `spawn_blocking` 里跑）
fn scan_blocking(home: &Path) {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_else(|| load(home));
    let mut changed = 0;
    for (path, agent) in sources() {
        let key = path.to_string_lossy().into_owned();
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let (len, mtime) = (meta.len(), mtime_of(&meta));
        if st.files.get(&key).is_some_and(|f| f.len == len && f.mtime == mtime && f.v >= PARSE_V) {
            continue;
        }
        // 读不了、解压失败（可能正压到一半）的这次跳过，下次再试
        let Some(bytes) = crate::history::read_log(&path) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let seen: HashSet<u64> = st.files.iter().filter(|(k, _)| **k != key).flat_map(|(_, f)| f.ids.iter().copied()).collect();
        let parsed = if agent == "claude" { parse_claude(&text, &seen) } else { parse_codex(&text, &seen) };
        let mut rows: Vec<Row> = parsed.rows.into_values().collect();
        rows.sort_by(|a, b| (a.h, &a.m).cmp(&(b.h, &b.m)));
        let fs = FileState { len, mtime, agent: agent.into(), provider: parsed.provider, rows, ids: parsed.ids, v: PARSE_V, cwd: parsed.cwd, title: parsed.title, sid: parsed.sid };
        st.files.insert(key, fs);
        changed += 1;
    }
    st.scanned_at = crate::util::now_rfc3339();
    if changed > 0 {
        let file = state_file(home);
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec(&st) {
            let _ = crate::util::write_private(&file, &b);
        }
        tracing::debug!(changed, "用量统计已更新");
    }
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(st);
}

/// 马上扫一遍（界面上点“刷新”、定时任务）
pub async fn scan(home: PathBuf) {
    let _guard = SCANNING.lock().await;
    let _ = tokio::task::spawn_blocking(move || scan_blocking(&home)).await;
}

/// 后台：启动一会儿后扫一遍，之后每 10 分钟扫一遍
pub async fn run(home: PathBuf) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        scan(home.clone()).await;
        tokio::time::sleep(SCAN_EVERY).await;
    }
}

/// Codex 的 `model_provider` → models.dev 的服务商
fn codex_catalog(c: &crate::pricing::Catalog, store: &crate::store::Store, provider: Option<&str>) -> Option<String> {
    match provider? {
        "openai" => Some("openai".into()),
        p => match p.strip_prefix(crate::codex_profiles::PROVIDER_PREFIX) {
            Some(id) => {
                let prof = crate::codex_profiles::get(store, id)?;
                prof.preset.as_deref().and_then(crate::model_presets::get).map(|x| x.catalog.to_string()).or_else(|| crate::pricing::provider_for_url(c, prof.base_url.as_deref()?))
            }
            // 用户自己在 ~/.codex/config.toml 里定义的服务商：名字就是 models.dev 的、或者按它的 base_url 认
            None => c.providers.contains_key(p).then(|| p.to_string()).or_else(|| crate::pricing::provider_for_url(c, &codex_provider_url(p)?)),
        },
    }
}

/// `~/.codex/config.toml` 里 `[model_providers.<名字>]` 的 `base_url`
fn codex_provider_url(name: &str) -> Option<String> {
    let text = std::fs::read_to_string(crate::agents::codex_home().join("config.toml")).ok()?;
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    doc.get("model_providers")?.get(name)?.get("base_url")?.as_str().map(String::from)
}

/// 记下的“对话 → 账号”（kv `usage_convs`，最多 [`CONVS_MAX`] 个）：读一次 store，之后在内存里改，改完写回
static CONVS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
const CONVS_KEY: &str = "usage_convs";
const CONVS_MAX: usize = 20_000;

/// 一段 Claude Code 对话用的账号：`claude:<配置 ID>`（看看里的账号、第三方接口），认不出的第三方接口是 `url:<地址>`。
/// 先看进程（配置目录、`ANTHROPIC_BASE_URL`），看不到时用会话新建时定下的
pub fn claude_account_key(store: &crate::store::Store, home: &Path, acc: Option<&crate::session_probe::Account>, inst: Option<&str>) -> String {
    use crate::claude_profiles as cp;
    let profiles = cp::list(store);
    if let Some(url) = acc.and_then(|a| a.base_url.as_deref()) {
        let url = url.trim().trim_end_matches('/');
        return match profiles.iter().find(|p| p.kind == cp::Kind::Api && p.base_url.as_deref().map(|u| u.trim_end_matches('/')) == Some(url)) {
            Some(p) => format!("claude:{}", p.id),
            None => format!("url:{url}"),
        };
    }
    if let Some(dir) = acc.and_then(|a| a.dir.as_deref()) {
        if let Some(p) = profiles.iter().find(|p| p.kind == cp::Kind::Official && p.id != cp::OFFICIAL && cp::account_dir(home, &p.id) == dir) {
            return format!("claude:{}", p.id);
        }
        return format!("claude:{}", cp::OFFICIAL);
    }
    let id = inst.map(|i| cp::for_instance(store, i).id).unwrap_or_else(|| cp::OFFICIAL.into());
    format!("claude:{id}")
}

/// 记下一段对话用的账号（变了才写）
pub fn note_conv(store: &crate::store::Store, conv: &str, key: &str) {
    let mut g = CONVS.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(|| store.get(CONVS_KEY).ok().flatten().unwrap_or_default());
    if m.get(conv).map(String::as_str) == Some(key) {
        return;
    }
    // 太多了：去掉一些（记录文件早被 Claude Code 清理掉的那些对话，统计里已经算好了，只是以后不会再变）
    if m.len() >= CONVS_MAX {
        let old: Vec<String> = m.keys().take(CONVS_MAX / 10).cloned().collect();
        for k in old {
            m.remove(&k);
        }
    }
    m.insert(conv.into(), key.into());
    if let Err(e) = store.set(CONVS_KEY, &*m) {
        tracing::debug!(error = %e, "用量统计：对话的账号没记上");
    }
}

fn conv_accounts(store: &crate::store::Store) -> HashMap<String, String> {
    let mut g = CONVS.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(|| store.get(CONVS_KEY).ok().flatten().unwrap_or_default()).clone()
}

/// Claude Code 记录文件 → 对话号（UUID）：`<对话号>.jsonl`，子代理的在 `<对话号>/subagents/agent-*.jsonl`
fn claude_conv_of(path: &str) -> Option<String> {
    let uuid = |s: &str| s.len() == 36 && s.split('-').map(str::len).eq([8, 4, 4, 4, 12]) && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    let p = Path::new(path);
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    if uuid(&stem) {
        return Some(stem);
    }
    p.ancestors().skip(1).filter_map(|a| a.file_name()).map(|n| n.to_string_lossy().into_owned()).find(|n| uuid(n))
}

/// 账号的名字和价格按哪家算：(名字, 种类 official / api / unknown, models.dev 的服务商)
fn account_info(c: Option<&crate::pricing::Catalog>, store: &crate::store::Store, agent: &str, key: &str) -> (String, &'static str, Option<String>) {
    if agent == "codex" {
        let name = match key {
            "openai" => String::new(),
            p => p.strip_prefix(crate::codex_profiles::PROVIDER_PREFIX).and_then(|id| crate::codex_profiles::get(store, id)).map(|x| x.name).unwrap_or_else(|| p.to_string()),
        };
        let kind = if key == "openai" { "official" } else { "api" };
        return (name, kind, c.and_then(|c| codex_catalog(c, store, Some(key))));
    }
    if let Some(url) = key.strip_prefix("url:") {
        return (url.to_string(), "api", c.and_then(|c| crate::pricing::provider_for_url(c, url)));
    }
    let id = key.strip_prefix("claude:").unwrap_or(crate::claude_profiles::OFFICIAL);
    match crate::claude_profiles::get(store, id) {
        Some(p) if p.kind == crate::claude_profiles::Kind::Api => {
            let hint = p.preset.as_deref().and_then(crate::model_presets::get).map(|x| x.catalog.to_string()).or_else(|| c.and_then(|c| crate::pricing::provider_for_url(c, p.base_url.as_deref()?)));
            (p.name, "api", hint)
        }
        Some(p) => (p.name, "official", Some("anthropic".into())),
        None => (String::new(), "official", Some("anthropic".into())),
    }
}

/// 按价格算钱：长提示词那一档、快速模式的倍数、联网搜索按次（Claude 没写价时 1 美分一次）
fn cost_of(r: &Row, p: &crate::pricing::Cost) -> f64 {
    let p = p.at(u64::from(r.x) * 1000);
    let read = p.cache_read.unwrap_or(p.input);
    let write = p.cache_write.unwrap_or(p.input * 1.25);
    let write_1h = p.cache_write_1h.unwrap_or(p.input * 2.0);
    let tokens = (r.i as f64 * p.input + r.o as f64 * p.output + r.cr as f64 * read + r.cw as f64 * write + r.c1 as f64 * write_1h) / 1_000_000.0;
    let rate = if r.f { fast_rate(&r.m) } else { 1.0 };
    let search = p.web_search.or_else(|| r.m.to_ascii_lowercase().starts_with("claude-").then_some(0.01)).unwrap_or(0.0);
    tokens * rate + r.w as f64 * search
}

#[derive(Default, Clone)]
struct Sum {
    tokens: [u64; 5],
    requests: u64,
    cost: f64,
}

impl Sum {
    fn add(&mut self, r: &Row, cost: f64) {
        for (s, v) in self.tokens.iter_mut().zip([r.i, r.o, r.cr, r.cw + r.c1, 0]) {
            *s += v;
        }
        self.tokens[4] += r.i + r.o + r.cr + r.cw + r.c1;
        self.requests += r.n;
        self.cost += cost;
    }
    fn merge(&mut self, o: &Sum) {
        self.tokens.iter_mut().zip(o.tokens).for_each(|(x, y)| *x += y);
        self.requests += o.requests;
        self.cost += o.cost;
    }
    fn view(&self) -> Value {
        let [input, output, cache_read, cache_write, total] = self.tokens;
        json!({ "input": input, "output": output, "cache_read": cache_read, "cache_write": cache_write, "tokens": total, "requests": self.requests, "cost": (self.cost * 10000.0).round() / 10000.0 })
    }
}

/// 一组（一天、一小时）里每个助手的用量 → `{合计…, agents: {助手: 用量}}`
fn bucket_view(per: Option<&BTreeMap<String, Sum>>) -> Value {
    let mut sum = Sum::default();
    let mut agents = serde_json::Map::new();
    for (a, s) in per.into_iter().flatten() {
        agents.insert(a.clone(), s.view());
        sum.merge(s);
    }
    let mut v = sum.view();
    v["agents"] = Value::Object(agents);
    v
}

fn by_cost(a: &Value, b: &Value) -> std::cmp::Ordering {
    b["cost"].as_f64().partial_cmp(&a["cost"].as_f64()).unwrap_or(std::cmp::Ordering::Equal).then(b["tokens"].as_u64().cmp(&a["tokens"].as_u64()))
}

/// 一段会话
#[derive(Default)]
struct Sess {
    sum: Sum,
    title: Option<String>,
    cwd: Option<String>,
    /// 最后一次用量在第几个小时
    last: i64,
    /// 模型 → token（列出用得最多的）
    models: HashMap<String, u64>,
}

/// 最近 `days` 天的统计。`tz` 是界面所在时区比 UTC 早多少分钟（东八区是 480），按当地的日期分天。
/// 另带上一个同样长的时间段的合计（`prev`，看涨了还是降了）和最后一天每小时的用量（`hours`）。
/// `on`（当地日期 YYYY-MM-DD）：只看那一天（`days` 不管），`prev` 是前一天
pub fn report(store: &crate::store::Store, days: u32, tz: i32, on: Option<&str>) -> Value {
    let guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(st) = guard.as_ref() else { return json!({ "ready": false }) };
    let cat = crate::pricing::catalog();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let tz_secs = i64::from(tz.clamp(-14 * 60, 14 * 60)) * 60;
    let today = (now + tz_secs).div_euclid(86400);
    let picked = on.and_then(|d| time::Date::parse(d, time::macros::format_description!("[year]-[month]-[day]")).ok()).map(|d| i64::from(d.to_julian_day()) - 2_440_588).filter(|&d| d <= today);
    // 最后一天：一般是今天，点了某一天时是那一天
    let end = picked.unwrap_or(today);
    let span = if picked.is_some() { 1 } else { i64::from(days.clamp(1, 366)) };
    let first = end - span + 1;
    let date = |d: i64| time::OffsetDateTime::from_unix_timestamp(d * 86400).map(|t| t.date().to_string()).unwrap_or_default();
    let hour_ts = |h: i64| time::OffsetDateTime::from_unix_timestamp(h * 3600).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok());

    let mut by_day: BTreeMap<i64, BTreeMap<String, Sum>> = BTreeMap::new();
    // 今天当地的第几个小时 → 助手 → 用量
    let mut by_hour: BTreeMap<i64, BTreeMap<String, Sum>> = BTreeMap::new();
    // (助手, 模型) → (用量, 价格来自哪家)
    let mut by_model: BTreeMap<(String, String), (Sum, Option<String>, u64)> = BTreeMap::new();
    // 助手（Claude Code、Codex）→ 用量
    let mut by_agent: BTreeMap<String, Sum> = BTreeMap::new();
    // (助手, 账号) → 用量
    let mut by_account: BTreeMap<(String, String), Sum> = BTreeMap::new();
    // 工作目录 → (用量, 给人看的目录, 会话)
    type Project = (Sum, String, HashSet<(String, String)>);
    let mut by_project: HashMap<String, Project> = HashMap::new();
    // (助手, 会话号) → 会话
    let mut by_session: HashMap<(String, String), Sess> = HashMap::new();
    let (mut total, mut prev, mut today_sum) = (Sum::default(), Sum::default(), Sum::default());
    // 账号 → (名字, 种类, 价格按哪家)
    let mut info_of: HashMap<(String, String), (String, &'static str, Option<String>)> = HashMap::new();
    let convs = conv_accounts(store);
    for (path, f) in &st.files {
        // Claude Code 的账号、第三方接口都写在同一个 projects/ 里：按记下的“对话 → 账号”分；Codex 的记录里写着服务商
        let claude_conv = (f.agent != "codex").then(|| claude_conv_of(path)).flatten();
        let account = match f.agent.as_str() {
            "codex" => f.provider.clone().unwrap_or_else(|| "openai".into()),
            _ => claude_conv.as_ref().and_then(|c| convs.get(c).cloned()).unwrap_or_else(|| format!("claude:{}", crate::claude_profiles::OFFICIAL)),
        };
        // 一段会话：Claude Code 的子代理算在它的对话里
        let conv = if f.agent == "codex" { f.sid.clone() } else { claude_conv }.unwrap_or_else(|| path.clone());
        let sess_key = (f.agent.clone(), conv);
        let project = f.cwd.as_deref().map(crate::history::dir_key).unwrap_or_default();
        let hint = info_of.entry((f.agent.clone(), account.clone())).or_insert_with(|| account_info(cat.as_deref(), store, &f.agent, &account)).2.clone();
        for r in &f.rows {
            let day = (r.h * 3600 + tz_secs).div_euclid(86400);
            if day < first - span || day > end {
                continue;
            }
            // 哪里都还没有价格的新 Claude 模型：按同一系列最新的估（price_from 带 `~`，界面上标“估算”）
            let priced = cat.as_ref().and_then(|c| crate::pricing::price(c, hint.as_deref(), &r.m).or_else(|| crate::pricing::guess_price(c, &r.m)));
            let cost = priced.as_ref().map(|(p, _)| cost_of(r, p)).unwrap_or(0.0);
            if day < first {
                prev.add(r, cost);
                continue;
            }
            by_day.entry(day).or_default().entry(f.agent.clone()).or_default().add(r, cost);
            by_agent.entry(f.agent.clone()).or_default().add(r, cost);
            by_account.entry((f.agent.clone(), account.clone())).or_default().add(r, cost);
            let e = by_model.entry((f.agent.clone(), r.m.clone())).or_default();
            e.0.add(r, cost);
            if let Some((_, pid)) = priced {
                e.1 = Some(pid);
            }
            if r.f {
                e.2 += r.n;
            }
            let p = by_project.entry(project.clone()).or_insert_with(|| (Sum::default(), f.cwd.clone().unwrap_or_default(), HashSet::new()));
            p.0.add(r, cost);
            p.2.insert(sess_key.clone());
            let s = by_session.entry(sess_key.clone()).or_default();
            s.sum.add(r, cost);
            s.last = s.last.max(r.h);
            *s.models.entry(r.m.clone()).or_default() += r.i + r.o + r.cr + r.cw + r.c1;
            if s.title.is_none() {
                s.title = f.title.clone();
            }
            if s.cwd.is_none() {
                s.cwd = f.cwd.clone();
            }
            total.add(r, cost);
            if day == end {
                today_sum.add(r, cost);
                let hour = (r.h * 3600 + tz_secs).div_euclid(3600) - end * 24;
                by_hour.entry(hour).or_default().entry(f.agent.clone()).or_default().add(r, cost);
            }
        }
    }
    let names = |pid: &Option<String>| pid.as_ref().and_then(|p| cat.as_ref()?.providers.get(p.trim_end_matches('~')).map(|x| x.name.clone()));
    let mut models: Vec<Value> = by_model
        .iter()
        .map(|((agent, model), (s, pid, fast))| {
            let mut v = s.view();
            v["agent"] = json!(agent);
            v["model"] = json!(model);
            v["priced"] = json!(pid.is_some());
            v["price_from"] = json!(names(pid));
            // 没有这个模型的价格，按同一系列最新的估的
            v["estimated"] = json!(pid.as_deref().is_some_and(|p| p.ends_with('~')));
            // 快速模式的请求数（价钱另算）
            v["fast"] = json!(fast);
            v
        })
        .collect();
    models.sort_by(by_cost);
    let mut accounts: Vec<Value> = by_account
        .iter()
        .map(|((agent, key), s)| {
            let (name, kind, _) = info_of.get(&(agent.clone(), key.clone())).cloned().unwrap_or_default();
            let mut v = s.view();
            v["agent"] = json!(agent);
            v["account"] = json!(key);
            v["name"] = json!(name);
            v["kind"] = json!(kind);
            v
        })
        .collect();
    accounts.sort_by(by_cost);
    let mut projects: Vec<Value> = by_project
        .into_values()
        .map(|(s, dir, sessions)| {
            let mut v = s.view();
            v["dir"] = json!(dir);
            v["sessions"] = json!(sessions.len());
            v
        })
        .collect();
    projects.sort_by(by_cost);
    let codex_titles = if by_session.keys().any(|(a, _)| a == "codex") { crate::history::codex_titles() } else { HashMap::new() };
    let session_count = by_session.len();
    let mut agents: Vec<Value> = by_agent
        .iter()
        .map(|(agent, s)| {
            let mut v = s.view();
            v["agent"] = json!(agent);
            v["sessions"] = json!(by_session.keys().filter(|(a, _)| a == agent).count());
            v
        })
        .collect();
    agents.sort_by(by_cost);
    let mut sessions: Vec<Value> = by_session
        .into_iter()
        .map(|((agent, id), s)| {
            let title = s.title.or_else(|| (agent == "codex").then(|| codex_titles.get(&id).cloned()).flatten());
            let model = s.models.into_iter().max_by_key(|(_, n)| *n).map(|(m, _)| m);
            let mut v = s.sum.view();
            v["agent"] = json!(agent);
            // 认不出会话号的（文件名不是会话号）不给号，界面上就不能点开
            v["id"] = json!((crate::history::valid_id(&id)).then_some(&id));
            v["title"] = json!(title);
            v["dir"] = json!(s.cwd);
            v["model"] = json!(model);
            v["last"] = json!(hour_ts(s.last));
            v
        })
        .collect();
    sessions.sort_by(by_cost);
    sessions.truncate(TOP_SESSIONS);
    let days: Vec<Value> = (first..=end)
        .map(|d| {
            let mut v = bucket_view(by_day.get(&d));
            v["date"] = json!(date(d));
            v
        })
        .collect();
    let now_hour = if end == today { (now + tz_secs).div_euclid(3600) - today * 24 } else { 23 };
    let hours: Vec<Value> = (0..=now_hour)
        .map(|h| {
            let mut v = bucket_view(by_hour.get(&h));
            v["hour"] = json!(h);
            v
        })
        .collect();
    json!({
        "ready": true,
        "date": picked.map(|_| date(end)),
        "scanned_at": st.scanned_at,
        "pricing": cat.as_ref().map(|c| json!({ "source": "models.dev", "fetched_at": c.fetched_at, "claude_code": c.providers.contains_key(crate::pricing::CLAUDE_CODE) })),
        "total": total.view(),
        "prev": prev.view(),
        "today": today_sum.view(),
        "days": days,
        "hours": hours,
        "models": models,
        "agents": agents,
        "accounts": accounts,
        "projects": projects,
        "sessions": sessions,
        "session_count": session_count,
    })
}

/// 启动时先把上次存的读进来（还没扫完也能先看到）
pub fn preload(home: &Path) {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_none() && state_file(home).exists() {
        *g = Some(load(home));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        format!("{v}\n")
    }

    #[test]
    fn claude_dedupes_by_message() {
        let u = |out: u64| json!({ "input_tokens": 3, "output_tokens": out, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 50, "cache_creation": { "ephemeral_1h_input_tokens": 20 } });
        let mut s = String::new();
        s += &line(json!({ "type": "assistant", "timestamp": "2026-10-27T10:17:26.913Z", "requestId": "r1", "message": { "id": "m1", "model": "claude-opus-5-5", "usage": u(1) } }));
        s += &line(json!({ "type": "assistant", "timestamp": "2026-10-27T10:17:27.000Z", "requestId": "r1", "message": { "id": "m1", "model": "claude-opus-5-5", "usage": u(9) } }));
        s += &line(json!({ "type": "assistant", "timestamp": "2026-10-27T11:00:00Z", "requestId": "r2", "message": { "id": "m2", "model": "<synthetic>", "usage": u(1) } }));
        s += &line(json!({ "type": "user", "timestamp": "2026-10-27T11:00:00Z", "message": { "usage": u(1) } }));
        let p = parse_claude(&s, &HashSet::new());
        assert_eq!(p.ids.len(), 1);
        let r = p.rows.values().next().unwrap();
        assert_eq!((r.i, r.o, r.cr, r.cw, r.c1, r.n), (3, 9, 100, 30, 20, 1));
        // 别的文件算过的不再算
        assert!(parse_claude(&s, &p.ids.iter().copied().collect()).rows.is_empty());
    }

    #[test]
    fn codex_skips_repeated_counts() {
        let tc = |ts: &str, total: u64, input: u64| {
            line(json!({ "timestamp": ts, "type": "event_msg", "payload": { "type": "token_count", "info": { "total_token_usage": { "total_tokens": total }, "last_token_usage": { "input_tokens": input, "cached_input_tokens": 40, "output_tokens": 7 } } } }))
        };
        let mut s = line(json!({ "timestamp": "2026-10-23T07:38:16Z", "type": "session_meta", "payload": { "model_provider": "looklook-ab12" } }));
        s += &line(json!({ "timestamp": "2026-10-23T07:38:28Z", "type": "turn_context", "payload": { "model": "glm-5.3" } }));
        s += &tc("2026-10-23T07:40:00Z", 100, 50);
        s += &tc("2026-10-23T07:40:01Z", 100, 50);
        s += &line(json!({ "timestamp": "2026-10-23T07:41:00Z", "type": "event_msg", "payload": { "type": "token_count", "info": null } }));
        s += &tc("2026-10-23T08:01:00Z", 200, 60);
        let p = parse_codex(&s, &HashSet::new());
        assert_eq!(p.provider.as_deref(), Some("looklook-ab12"));
        assert_eq!(p.ids.len(), 2);
        let mut rows: Vec<&Row> = p.rows.values().collect();
        rows.sort_by_key(|r| r.h);
        assert_eq!((rows[0].m.as_str(), rows[0].i, rows[0].cr, rows[0].o), ("glm-5.3", 10, 40, 7));
        assert_eq!(rows[1].i, 20);
    }

    /// 端到端：扫本机真实的会话记录，用 models.dev 的价格算一遍（不联网：价格从文件读）。
    /// `USAGE_E2E_PRICING=<models.dev 的 api.json> cargo test real_scan -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_scan() {
        let raw: Value = serde_json::from_slice(&std::fs::read(std::env::var("USAGE_E2E_PRICING").unwrap()).unwrap()).unwrap();
        crate::pricing::set_for_test(crate::pricing::parse_for_test(&raw));
        let home = std::env::temp_dir().join(format!("ll-usage-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let t = std::time::Instant::now();
        scan_blocking(&home);
        let first = t.elapsed();
        let t = std::time::Instant::now();
        scan_blocking(&home);
        let store = crate::store::Store::open(&home.join("t.db")).unwrap();
        let r = report(&store, 30, 480, None);
        println!("首次 {first:?}，再扫 {:?}；文件 {}", t.elapsed(), STATE.lock().unwrap().as_ref().unwrap().files.len());
        println!("合计 {}\n今天 {}", r["total"], r["today"]);
        for m in r["models"].as_array().unwrap().iter().take(12) {
            println!("{} {} tokens={} cost={} 价格={}", m["agent"], m["model"], m["tokens"], m["cost"], m["price_from"]);
        }
        println!("上一段 {}\n工具 {}", r["prev"], r["agents"]);
        for p in r["projects"].as_array().unwrap().iter().take(8) {
            println!("项目 {} 会话={} cost={}", p["dir"], p["sessions"], p["cost"]);
        }
        for s in r["sessions"].as_array().unwrap().iter().take(8) {
            println!("会话 {} {} {} {} cost={}", s["agent"], s["title"], s["dir"], s["last"], s["cost"]);
        }
        if let Ok(out) = std::env::var("USAGE_E2E_OUT") {
            std::fs::write(out, r.to_string()).unwrap();
        }
        println!("今天每小时 {}", r["hours"].as_array().unwrap().iter().map(|h| h["tokens"].to_string()).collect::<Vec<_>>().join(","));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn reads_zstd_rollouts() {
        let dir = std::env::temp_dir().join(format!("ll-usage-zst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = b"{\"type\":\"session_meta\"}\n".repeat(50);
        let plain = dir.join("rollout-a.jsonl");
        std::fs::write(&plain, &text).unwrap();
        let zst = dir.join("rollout-b.jsonl.zst");
        std::fs::write(&zst, ruzstd::encoding::compress_to_vec(text.as_slice(), ruzstd::encoding::CompressionLevel::Fastest)).unwrap();
        assert_eq!(crate::history::read_log(&plain).unwrap(), text);
        assert_eq!(crate::history::read_log(&zst).unwrap(), text);
        // 压到一半（不完整）的读不出来，这次跳过
        std::fs::write(&zst, &std::fs::read(&zst).unwrap()[..8]).unwrap();
        assert!(crate::history::read_log(&zst).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_cwd_and_title() {
        let u = json!({ "input_tokens": 1, "output_tokens": 2 });
        let mut s = line(json!({ "type": "ai-title", "aiTitle": "旧标题" }));
        s += &line(json!({ "type": "assistant", "cwd": "/w/app", "timestamp": "2026-10-27T10:00:00Z", "requestId": "r", "message": { "id": "m", "model": "claude-opus-5-5", "usage": u } }));
        s += &line(json!({ "type": "ai-title", "aiTitle": "新标题" }));
        let p = parse_claude(&s, &HashSet::new());
        assert_eq!((p.cwd.as_deref(), p.title.as_deref()), (Some("/w/app"), Some("新标题")));
        s += &line(json!({ "type": "custom-title", "customTitle": "改过的名字" }));
        assert_eq!(parse_claude(&s, &HashSet::new()).title.as_deref(), Some("改过的名字"), "/rename 起的优先");
        let c = line(json!({ "timestamp": "2026-10-23T07:38:16Z", "type": "session_meta", "payload": { "id": "abc-1", "cwd": "/w/x", "model_provider": "openai" } }));
        let p = parse_codex(&c, &HashSet::new());
        assert_eq!((p.cwd.as_deref(), p.sid.as_deref()), (Some("/w/x"), Some("abc-1")));
    }

    /// 报表：上一段时间、今天每小时、按项目、按会话（子代理算在它的对话里）
    #[test]
    fn report_groups() {
        let now_h = time::OffsetDateTime::now_utc().unix_timestamp().div_euclid(3600);
        let row = |h: i64, o: u64| Row { h, m: "m1".into(), o, n: 1, ..Default::default() };
        let id = "e7256753-67bc-473c-89c9-63df2a292e33";
        let file = |rows: Vec<Row>, cwd: &str, title: Option<&str>| FileState { agent: "claude".into(), rows, v: PARSE_V, cwd: Some(cwd.into()), title: title.map(String::from), ..Default::default() };
        let mut st = State { version: VERSION, ..Default::default() };
        st.files.insert(format!("/p/-w-a/{id}.jsonl"), file(vec![row(now_h, 10), row(now_h - 24 * 10, 5)], "/w/a", Some("修登录")));
        st.files.insert(format!("/p/-w-a/{id}/subagents/agent-1.jsonl"), file(vec![row(now_h, 3)], "/w/a", None));
        st.files.insert("/p/-w-b/x.jsonl".into(), file(vec![row(now_h, 1)], "/w/b/", None));
        *STATE.lock().unwrap() = Some(st);
        let home = std::env::temp_dir().join(format!("ll-usage-rep-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let store = crate::store::Store::open(&home.join("t.db")).unwrap();
        let r = report(&store, 7, 0, None);
        *STATE.lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!((r["total"]["output"].as_u64(), r["prev"]["output"].as_u64(), r["today"]["output"].as_u64()), (Some(14), Some(5), Some(14)));
        let hours = r["hours"].as_array().unwrap();
        assert_eq!(hours.last().unwrap()["output"], 14, "这个小时的用量");
        let projects = r["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!((projects[0]["dir"].as_str(), projects[0]["output"].as_u64(), projects[0]["sessions"].as_u64()), (Some("/w/a"), Some(13), Some(1)));
        let sessions = r["sessions"].as_array().unwrap();
        assert_eq!((sessions[0]["id"].as_str(), sessions[0]["title"].as_str(), sessions[0]["output"].as_u64()), (Some(id), Some("修登录"), Some(13)));
        assert!(sessions[1]["id"].is_null(), "认不出会话号的不给号");
        assert_eq!((r["agents"][0]["agent"].as_str(), r["agents"][0]["sessions"].as_u64()), (Some("claude"), Some(2)));
    }

    #[test]
    fn claude_conv_from_path() {
        let id = "e7256753-67bc-473c-89c9-63df2a292e33";
        assert_eq!(claude_conv_of(&format!("/h/.claude/projects/-w/{id}.jsonl")).as_deref(), Some(id));
        assert_eq!(claude_conv_of(&format!("/h/.claude/projects/-w/{id}/subagents/agent-a1b2c3.jsonl")).as_deref(), Some(id));
        assert_eq!(claude_conv_of("/h/.claude/projects/-w/notes.jsonl"), None);
    }

    #[test]
    fn cost_formula() {
        let p = crate::pricing::Cost { input: 3.0, output: 15.0, cache_read: Some(0.3), cache_write: Some(3.75), ..Default::default() };
        let r = Row { i: 1_000_000, o: 1_000_000, cr: 1_000_000, cw: 1_000_000, c1: 1_000_000, ..Default::default() };
        assert!((cost_of(&r, &p) - (3.0 + 15.0 + 0.3 + 3.75 + 6.0)).abs() < 1e-9);
        let no_cache = crate::pricing::Cost { input: 1.0, output: 2.0, ..Default::default() };
        assert!((cost_of(&Row { cr: 1_000_000, ..Default::default() }, &no_cache) - 1.0).abs() < 1e-9, "没有缓存价时按输入价");
        // 快速模式、联网搜索
        let opus = crate::pricing::Cost { input: 4.0, output: 20.0, ..Default::default() };
        let fast = Row { m: "claude-opus-5-5".into(), o: 1_000_000, f: true, w: 3, ..Default::default() };
        assert!((cost_of(&fast, &opus) - (40.0 + 0.03)).abs() < 1e-9);
        assert!((cost_of(&Row { m: "claude-opus-4-6".into(), o: 1_000_000, f: true, ..Default::default() }, &opus) - 120.0).abs() < 1e-9);
        // 长提示词那一档
        let haiku = crate::pricing::Cost { input: 0.1, output: 0.5, long: Some(crate::pricing::Long { above: 100_000, input: 0.5, output: 2.5, ..Default::default() }), ..Default::default() };
        assert!((cost_of(&Row { i: 1_000_000, x: 100, ..Default::default() }, &haiku) - 0.5).abs() < 1e-9);
        assert!((cost_of(&Row { i: 1_000_000, ..Default::default() }, &haiku) - 0.1).abs() < 1e-9);
    }

    #[test]
    fn claude_fast_long_advisor() {
        let mut s = String::new();
        let usage = json!({ "input_tokens": 2, "output_tokens": 10, "cache_read_input_tokens": 150_000, "cache_creation_input_tokens": 0, "speed": "fast",
            "server_tool_use": { "web_search_requests": 2 },
            "iterations": [{ "type": "message", "input_tokens": 2, "output_tokens": 10 }, { "type": "advisor_message", "model": "claude-fable-5-2", "input_tokens": 500, "output_tokens": 40, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 }] });
        s += &line(json!({ "type": "assistant", "timestamp": "2026-10-27T10:17:26.913Z", "requestId": "r1", "message": { "id": "m1", "model": "claude-opus-5-5", "usage": usage } }));
        let p = parse_claude(&s, &HashSet::new());
        let main = p.rows.values().find(|r| r.m == "claude-opus-5-5").unwrap();
        assert_eq!((main.x, main.f, main.w), (100, true, 2));
        let adv = p.rows.values().find(|r| r.m == "claude-fable-5-2").unwrap();
        assert_eq!((adv.i, adv.o, adv.f, adv.x), (500, 40, false, 0));
        assert_eq!(p.ids.len(), 1);
    }
}
