//! 碎碎念文案（docs/PET_PACK.md §3.6）：按宠物的人设，一次性让模型写一批带占位符的句子存起来，
//! 之后宠物说话只是挑一句、填上运行数据（花了几分钟、改了几个文件……），不再花 token。
//!
//! 用哪个模型：设置里配了 OpenAI 兼容接口就用它；没配就用本机的 Claude Code（`claude -p`，haiku）或 Codex（`codex exec`）。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::LocalError;
use crate::pets::{GenApi, GenCfg, Pack, Text};

/// 能写的键、什么时候说、能用的占位符（和前端 `shared/pet/lines.js` 的 KEYS 一致）
pub const KEYS: &[(&str, &str)] = &[
    ("hello", "今天第一次见到主人，打招呼"),
    ("morning", "早上打招呼"),
    ("evening", "晚上打招呼"),
    ("lateNight", "深夜主人还在干活，心疼、催睡觉"),
    ("longSession", "主人连续干了 {min} 分钟，劝歇一会儿"),
    ("sent", "主人刚发出一条消息给 AI"),
    ("thinking", "AI 在思考"),
    ("longThink", "AI 想了 {min} 分钟了还没想完"),
    ("tool", "AI 在用工具干活"),
    ("edit", "AI 在改文件 {file}"),
    ("editMany", "这一轮已经改了 {n} 个文件"),
    ("search", "AI 在代码里翻找东西"),
    ("web", "AI 在上网查资料"),
    ("testRun", "AI 在跑测试"),
    ("testPass", "测试通过了"),
    ("testFail", "测试没过"),
    ("fixed", "之前失败的测试修好了"),
    ("install", "AI 在装依赖"),
    ("build", "AI 在编译 / 构建"),
    ("commit", "AI 提交了代码（git commit）"),
    ("push", "AI 把代码推上去了（git push）"),
    ("agentSpawn", "AI 派出了子代理帮忙"),
    ("errorStreak", "命令连着失败了 {n} 次"),
    ("waiting", "AI 在等主人批准或回答"),
    ("danger", "AI 想执行危险命令（删文件、强推……），提醒主人看仔细"),
    ("done", "这一轮做完了"),
    ("recapQuick", "这一轮很快就做完了（{dur}）"),
    ("recapLong", "这一轮花了 {dur}，辛苦了"),
    ("recapBusy", "这一轮改了 {files} 个文件、跑了 {cmds} 条命令"),
    ("error", "这一轮出错了"),
    ("click", "被主人点了一下"),
    ("petted", "被主人摸头"),
    ("annoyed", "被连着点了好多下，有点烦"),
    ("dragged", "被主人拎起来"),
    ("thrown", "被主人扔出去"),
    ("sleepStart", "闲太久了，要睡觉"),
    ("sleepEnd", "睡醒了"),
    ("wake", "睡觉时被吵醒"),
    ("levelUp", "和主人的好感升到了 {level} 级"),
    ("idleChat", "闲着没事，自言自语、哼歌、发呆"),
    ("roam", "闲着没事，要到屏幕别处溜达"),
    ("peerHello", "看到另一只宠物（另一个窗口里的自己），打招呼"),
    ("peerChat", "和另一只宠物闲聊一句"),
    ("notifyDone", "提醒主人：会话「{name}」做完了"),
    ("notifyWait", "提醒主人：会话「{name}」在等主人确认或回答"),
    ("notifyError", "提醒主人：会话「{name}」出错了"),
];

const PER_KEY: usize = 12;
const MAX_LEN: usize = 80;
const TIMEOUT: Duration = Duration::from_secs(300);

fn text_of(t: &Option<Text>, lang: &str) -> String {
    let v = serde_json::to_value(t).unwrap_or(Value::Null);
    match v {
        Value::String(s) => s,
        Value::Object(m) => m.get(lang).or_else(|| m.get("zh-CN")).or_else(|| m.values().next()).and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// 生成哪几种语言（和界面一致）
pub const LANGS: &[&str] = &["zh-CN", "en-US"];

/// 给模型的提示词：一次要中英两套；`style` 是用户另加的风格要求
pub fn prompt(pack: &Pack, style: &str) -> String {
    let m = &pack.manifest;
    let both = |t: &Option<Text>| {
        let (zh, en) = (text_of(t, "zh-CN"), text_of(t, "en-US"));
        if zh == en { zh } else { format!("{zh} / {en}") }
    };
    let (name, desc, persona) = (both(&m.name), both(&m.description), both(&m.persona));
    let keys: String = KEYS.iter().map(|(k, d)| format!("- {k}: {d}\n")).collect();
    let style = style.trim();
    let style = if style.is_empty() { String::new() } else { format!("风格要求（优先于人设里的说话方式）：{style}\n") };
    format!(
        "你在给一个桌面宠物写台词。宠物住在 AI 编程助手（Claude Code / Codex）的对话界面角落，陪主人写代码。\n\
         宠物名字：{name}\n介绍：{desc}\n人设：{persona}\n{style}\n\
         按下面每个场景写 6 句台词，简体中文和英文各一套（英文不是逐句翻译，要像母语者说的话，保持同样的性格）。要求：\n\
         - 中文每句不超过 20 个字，英文每句不超过 50 个字母；口语、俏皮、符合人设，可以用颜文字；\n\
         - 同一个场景里的句子要各不相同；\n\
         - 场景说明里的 {{min}} {{n}} {{file}} {{dur}} {{files}} {{cmds}} {{level}} {{name}} 是占位符，会被换成真实数据，用到的话原样写出（带花括号），不要编造数字；\n\
         - 只输出一个 JSON 对象：{{\"zh-CN\": {{场景名: [句子]}}, \"en-US\": {{场景名: [句子]}}}}，不要别的文字、不要代码块。\n\n场景：\n{keys}"
    )
}

/// 一种语言的 `{ 键: [句子] }`：只收认识的键，去重、截短、每键最多 12 句；少于 5 个键算没有
fn parse_one(v: &Value) -> Option<BTreeMap<String, Vec<String>>> {
    let mut out = BTreeMap::new();
    for (k, _) in KEYS {
        let Some(list) = v[*k].as_array() else { continue };
        let mut lines: Vec<String> = Vec::new();
        for s in list.iter().filter_map(Value::as_str) {
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if s.is_empty() || s.chars().count() > MAX_LEN || lines.contains(&s) {
                continue;
            }
            lines.push(s);
            if lines.len() >= PER_KEY {
                break;
            }
        }
        if !lines.is_empty() {
            out.insert(k.to_string(), lines);
        }
    }
    (out.len() >= 5).then_some(out)
}

/// 模型的回答 → `{ 语言: { 键: [句子] } }`。模型只给了一套（没按语言分）时当成中文
pub fn parse(answer: &str) -> Option<BTreeMap<String, BTreeMap<String, Vec<String>>>> {
    let start = answer.find('{')?;
    let end = answer.rfind('}')?;
    let v: Value = serde_json::from_str(answer.get(start..=end)?).ok()?;
    let mut out = BTreeMap::new();
    for l in LANGS {
        if let Some(m) = parse_one(&v[*l]) {
            out.insert(l.to_string(), m);
        }
    }
    if out.is_empty() {
        out.insert("zh-CN".to_string(), parse_one(&v)?);
    }
    Some(out)
}

// ---------------- 后台生成 ----------------
// 点“生成”马上返回，在后台跑（模型要一两分钟）；页面隔一会儿问进度。做完存好、通知页面（宠物马上换上新台词）。

/// 一只宠物的生成进度
#[derive(Debug, Clone, serde::Serialize)]
pub struct Job {
    /// `running` / `done` / `failed`
    pub state: &'static str,
    pub via: String,
    pub model: String,
    /// 开始的时间（毫秒）
    pub at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
}

static JOBS: std::sync::Mutex<BTreeMap<String, Job>> = std::sync::Mutex::new(BTreeMap::new());

fn jobs() -> std::sync::MutexGuard<'static, BTreeMap<String, Job>> {
    JOBS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn job(pet: &str) -> Option<Job> {
    jobs().get(pet).cloned()
}

/// 开始生成（同一只已经在生成时返回那一次的进度）
pub fn start(store: std::sync::Arc<crate::store::Store>, pack: Pack, cfg: GenCfg, api: Option<GenApi>, shell: String) -> Job {
    let key = pack.key();
    {
        let mut all = jobs();
        if let Some(j) = all.get(&key).filter(|j| j.state == "running") {
            return j.clone();
        }
        let j = Job { state: "running", via: cfg.via.clone(), model: cfg.model.clone(), at: crate::util::system_ms(), error: None };
        all.insert(key.clone(), j.clone());
    }
    crate::pets::changed();
    let j = job(&key).expect("刚放进去");
    tokio::spawn(async move {
        let r = generate(&pack, &cfg, api.as_ref(), &shell).await;
        let mut done = Err(LocalError::new("PET_GEN_FAIL"));
        if let Ok((used, langs)) = r.as_ref() {
            let mut all = json!(langs);
            all["at"] = json!(crate::util::system_ms());
            all["via"] = json!(used);
            done = crate::pets::set_gen_lines(&store, &key, Some(&all));
        } else if let Err(e) = r {
            done = Err(e);
        }
        let mut g = jobs();
        if let Some(j) = g.get_mut(&key) {
            match done {
                Ok(()) => j.state = "done",
                Err(e) => {
                    tracing::warn!(pet = %key, code = %e.code, "生成宠物台词失败");
                    j.state = "failed";
                    j.error = Some(json!({ "code": e.code, "params": e.params }));
                }
            }
        }
        drop(g);
        crate::pets::changed();
    });
    j
}

/// 用哪个：`api` / `claude` / `codex`；`auto` = 配了接口用接口，否则本机有哪个用哪个
pub async fn generate(pack: &Pack, cfg: &GenCfg, api: Option<&GenApi>, shell: &str) -> Result<(String, BTreeMap<String, BTreeMap<String, Vec<String>>>), LocalError> {
    let p = prompt(pack, &cfg.style);
    let api = api.filter(|a| !a.url.is_empty() && !a.model.is_empty());
    let found = crate::tools::detect(shell).await;
    let claude = found.claude.clone().or_else(crate::agents::claude_fallback);
    let via = match cfg.via.as_str() {
        "" | "auto" if api.is_some() => "api",
        "" | "auto" if claude.is_some() => "claude",
        "" | "auto" if found.codex.is_some() => "codex",
        "" | "auto" => return Err(LocalError::new("PET_GEN_NONE")),
        v => v,
    };
    let model = cfg.model.trim();
    let effort = cfg.effort.trim();
    let answer = match via {
        "api" => by_api(api.ok_or_else(|| LocalError::new("PET_GEN_NONE"))?, &p).await?,
        "claude" => {
            let exe = claude.ok_or_else(|| LocalError::new("PET_GEN_NONE").with("tool", "claude"))?;
            let mut args: Vec<String> = ["-p", "--max-turns", "1", "--output-format", "text"].map(String::from).to_vec();
            // 没选模型：用 haiku（快、便宜，写台词够了）
            args.extend(["--model".into(), if model.is_empty() { "haiku".into() } else { model.to_string() }]);
            if !effort.is_empty() {
                args.extend(["--effort".into(), effort.to_string()]);
            }
            by_cli(exe, &args, &p, None, shell).await?
        }
        "codex" => {
            let dir = tempfile::tempdir().map_err(LocalError::from)?;
            let out = dir.path().join("last.txt");
            let exe = found.codex.clone().ok_or_else(|| LocalError::new("PET_GEN_NONE").with("tool", "codex"))?;
            let mut args: Vec<String> = ["exec", "--skip-git-repo-check", "-s", "read-only", "--output-last-message"].map(String::from).to_vec();
            args.push(out.display().to_string());
            if !model.is_empty() {
                args.extend(["-m".into(), model.to_string()]);
            }
            if !effort.is_empty() {
                args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
            }
            by_cli(exe, &args, &p, Some(&out), shell).await?
        }
        _ => return Err(LocalError::invalid("via", "one_of")),
    };
    let lines = parse(&answer).ok_or_else(|| LocalError::new("PET_GEN_BAD"))?;
    Ok((via.to_string(), lines))
}

fn fail(e: impl std::fmt::Display) -> LocalError {
    LocalError::new("PET_GEN_FAIL").with("detail", e.to_string())
}

/// 接口地址：用户可能填到 `/v1` 为止，也可能只填域名
fn endpoint(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path == "/messages" && !base.ends_with("/v1") {
        return format!("{base}/v1/messages");
    }
    format!("{base}{path}")
}

async fn by_api(a: &GenApi, prompt: &str) -> Result<String, LocalError> {
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(fail)?;
    let effort = a.effort.trim();
    let req = match a.protocol.as_str() {
        "anthropic" => {
            let mut body = json!({ "model": a.model, "max_tokens": 8000, "messages": [{ "role": "user", "content": prompt }] });
            // 思考等级 → 思考预算（max_tokens 要比预算大）
            let budget = match effort {
                "minimal" | "low" => 2048,
                "medium" => 6000,
                "high" => 12000,
                "xhigh" | "max" => 24000,
                _ => 0,
            };
            if budget > 0 {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
                body["max_tokens"] = json!(budget + 8000);
            } else {
                body["temperature"] = json!(1.0);
            }
            let r = client.post(endpoint(&a.url, "/messages")).header("anthropic-version", "2023-06-01").json(&body);
            if a.key.is_empty() { r } else { r.header("x-api-key", &a.key).bearer_auth(&a.key) }
        }
        "chat" => {
            let mut body = json!({ "model": a.model, "messages": [{ "role": "user", "content": prompt }] });
            if effort.is_empty() {
                body["temperature"] = json!(1.0);
            } else {
                body["reasoning_effort"] = json!(effort);
            }
            let r = client.post(endpoint(&a.url, "/chat/completions")).json(&body);
            if a.key.is_empty() { r } else { r.bearer_auth(&a.key) }
        }
        _ => {
            let mut body = json!({ "model": a.model, "input": prompt });
            if !effort.is_empty() {
                body["reasoning"] = json!({ "effort": effort });
            }
            let r = client.post(endpoint(&a.url, "/responses")).json(&body);
            if a.key.is_empty() { r } else { r.bearer_auth(&a.key) }
        }
    };
    let r = req.send().await.map_err(fail)?;
    let status = r.status();
    let text = r.text().await.map_err(fail)?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().or_else(|| v["message"].as_str()).map(String::from).unwrap_or_else(|| format!("{status} {}", text.chars().take(200).collect::<String>()));
        return Err(fail(msg));
    }
    answer_text(&a.protocol, &v).ok_or_else(|| LocalError::new("PET_GEN_BAD"))
}

/// 三种协议的回答里取出文字
fn answer_text(protocol: &str, v: &Value) -> Option<String> {
    let join = |parts: Vec<&str>| (!parts.is_empty()).then(|| parts.join(""));
    match protocol {
        "anthropic" => join(v["content"].as_array()?.iter().filter(|c| c["type"] == "text").filter_map(|c| c["text"].as_str()).collect()),
        "chat" => v["choices"][0]["message"]["content"].as_str().map(String::from),
        _ => v["output_text"].as_str().map(String::from).or_else(|| {
            join(v["output"].as_array()?.iter().flat_map(|o| o["content"].as_array().into_iter().flatten()).filter(|c| c["type"] == "output_text").filter_map(|c| c["text"].as_str()).collect())
        }),
    }
}

/// 在空的临时目录里跑一次，提示词走标准输入（命令行有长度限制），等它说完
async fn by_cli(exe: String, args: &[String], prompt: &str, out_file: Option<&Path>, shell: &str) -> Result<String, LocalError> {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().map_err(LocalError::from)?;
    let mut cmd = tokio::process::Command::new(&exe);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd.args(args).current_dir(dir.path()).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    // 从程序坞打开的应用只有系统默认的 PATH：npm 装的 codex / claude 是 node 脚本（`#!/usr/bin/env node`），
    // 要带上它所在的目录和用户登录 shell 的 PATH 才找得到 node
    if let Some(p) = crate::chat::codex_app::launch_path(shell, Path::new(&exe)).await {
        cmd.env("PATH", p);
    }
    // 不要套进看看自己的 hook / 会话里
    cmd.env_remove("TMUX").env("LOOKLOOK_PET_GEN", "1");
    let mut child = cmd.spawn().map_err(fail)?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes()).await;
    }
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output()).await.map_err(|_| fail("timeout"))?.map_err(fail)?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = if err.trim().is_empty() { String::from_utf8_lossy(&out.stdout) } else { err };
        let err: String = err.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(fail(err));
    }
    if let Some(f) = out_file {
        if let Ok(s) = std::fs::read_to_string(f) {
            return Ok(s);
        }
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answer_text_by_protocol() {
        let r = json!({ "output": [{ "type": "reasoning" }, { "type": "message", "content": [{ "type": "output_text", "text": "{\"a\":" }, { "type": "output_text", "text": "1}" }] }] });
        assert_eq!(answer_text("responses", &r).as_deref(), Some("{\"a\":1}"));
        let a = json!({ "content": [{ "type": "thinking", "thinking": "x" }, { "type": "text", "text": "hi" }] });
        assert_eq!(answer_text("anthropic", &a).as_deref(), Some("hi"));
        assert_eq!(answer_text("chat", &json!({ "choices": [{ "message": { "content": "c" } }] })).as_deref(), Some("c"));
        assert_eq!(endpoint("https://api.x.com", "/messages"), "https://api.x.com/v1/messages");
        assert_eq!(endpoint("https://api.x.com/v1/", "/messages"), "https://api.x.com/v1/messages");
        assert_eq!(endpoint("https://api.x.com/v1", "/responses"), "https://api.x.com/v1/responses");
    }

    #[test]
    fn parse_keeps_known_keys() {
        let a = r#"好的：{"zh-CN":{"hello":["早呀","早呀"," 你来啦 "],"done":["搞定"],"testPass":["过啦"],"click":["干嘛"],"error":["呜"],"bogus":["x"],"wake":[1,"别吵"]},
                   "en-US":{"hello":["Hi"],"done":["Done"],"click":["Hey"],"error":["Oops"],"wake":["Zzz"]}}"#;
        let m = parse(a).unwrap();
        assert_eq!(m["zh-CN"]["hello"], vec!["早呀", "你来啦"]);
        assert!(!m["zh-CN"].contains_key("bogus"));
        assert_eq!(m["zh-CN"]["wake"], vec!["别吵"]);
        assert_eq!(m["en-US"]["hello"], vec!["Hi"]);
        // 只给了一套：当中文
        let flat = parse(r#"{"hello":["a"],"done":["b"],"click":["c"],"error":["d"],"wake":["e"]}"#).unwrap();
        assert_eq!(flat.keys().collect::<Vec<_>>(), vec!["zh-CN"]);
        assert!(parse(r#"{"hello":["a"]}"#).is_none());
        assert!(parse("没有 JSON").is_none());
    }
}
