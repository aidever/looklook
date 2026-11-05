//! 碎碎念文案（docs/PET_PACK.md §3.6）：按宠物的人设，一次性让模型写一批带占位符的句子存起来，
//! 之后宠物说话只是挑一句、填上运行数据（花了几分钟、改了几个文件……），不再花 token。
//!
//! 用哪个模型：设置里配了 OpenAI 兼容接口就用它；没配就用本机的 Claude Code（`claude -p`，haiku）或 Codex（`codex exec`）。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::LocalError;
use crate::pets::{GenApi, Pack, Text};

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
];

const PER_KEY: usize = 12;
const MAX_LEN: usize = 80;
const TIMEOUT: Duration = Duration::from_secs(180);

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

/// 给模型的提示词：一次要中英两套
pub fn prompt(pack: &Pack) -> String {
    let m = &pack.manifest;
    let both = |t: &Option<Text>| {
        let (zh, en) = (text_of(t, "zh-CN"), text_of(t, "en-US"));
        if zh == en { zh } else { format!("{zh} / {en}") }
    };
    let (name, desc, persona) = (both(&m.name), both(&m.description), both(&m.persona));
    let keys: String = KEYS.iter().map(|(k, d)| format!("- {k}: {d}\n")).collect();
    format!(
        "你在给一个桌面宠物写台词。宠物住在 AI 编程助手（Claude Code / Codex）的对话界面角落，陪主人写代码。\n\
         宠物名字：{name}\n介绍：{desc}\n人设：{persona}\n\n\
         按下面每个场景写 6 句台词，简体中文和英文各一套（英文不是逐句翻译，要像母语者说的话，保持同样的性格）。要求：\n\
         - 中文每句不超过 20 个字，英文每句不超过 50 个字母；口语、俏皮、符合人设，可以用颜文字；\n\
         - 同一个场景里的句子要各不相同；\n\
         - 场景说明里的 {{min}} {{n}} {{file}} {{dur}} {{files}} {{cmds}} {{level}} 是占位符，会被换成真实数据，用到的话原样写出（带花括号），不要编造数字；\n\
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

/// 用哪个：`api` / `claude` / `codex`；`auto` = 配了接口用接口，否则本机有哪个用哪个
pub async fn generate(pack: &Pack, via: &str, api: Option<&GenApi>, shell: &str) -> Result<(String, BTreeMap<String, BTreeMap<String, Vec<String>>>), LocalError> {
    let p = prompt(pack);
    let api = api.filter(|a| !a.url.is_empty() && !a.model.is_empty());
    let found = crate::tools::detect(shell).await;
    let claude = found.claude.clone().or_else(crate::agents::claude_fallback);
    let via = match via {
        "auto" if api.is_some() => "api",
        "auto" if claude.is_some() => "claude",
        "auto" if found.codex.is_some() => "codex",
        "auto" => return Err(LocalError::new("PET_GEN_NONE")),
        v => v,
    };
    let answer = match via {
        "api" => by_api(api.ok_or_else(|| LocalError::new("PET_GEN_NONE"))?, &p).await?,
        "claude" => by_cli(claude.ok_or_else(|| LocalError::new("PET_GEN_NONE"))?, &["-p", "--model", "haiku", "--max-turns", "1", "--output-format", "text"], &p, None).await?,
        "codex" => {
            let dir = tempfile::tempdir().map_err(LocalError::from)?;
            let out = dir.path().join("last.txt");
            let o = out.display().to_string();
            let exe = found.codex.clone().ok_or_else(|| LocalError::new("PET_GEN_NONE"))?;
            by_cli(exe, &["exec", "--skip-git-repo-check", "-s", "read-only", "--output-last-message", &o], &p, Some(&out)).await?
        }
        _ => return Err(LocalError::invalid("via", "one_of")),
    };
    let lines = parse(&answer).ok_or_else(|| LocalError::new("PET_GEN_BAD"))?;
    Ok((via.to_string(), lines))
}

async fn by_api(a: &GenApi, prompt: &str) -> Result<String, LocalError> {
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().map_err(|e| LocalError::new("PET_GEN_FAIL").with("detail", e.to_string()))?;
    let body = json!({ "model": a.model, "messages": [{ "role": "user", "content": prompt }], "temperature": 1.0 });
    let mut req = client.post(format!("{}/chat/completions", a.url)).json(&body);
    if !a.key.is_empty() {
        req = req.bearer_auth(&a.key);
    }
    let r = req.send().await.map_err(|e| LocalError::new("PET_GEN_FAIL").with("detail", e.to_string()))?;
    let status = r.status();
    let v: Value = r.json().await.map_err(|e| LocalError::new("PET_GEN_FAIL").with("detail", e.to_string()))?;
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().map(String::from).unwrap_or_else(|| status.to_string());
        return Err(LocalError::new("PET_GEN_FAIL").with("detail", msg));
    }
    v["choices"][0]["message"]["content"].as_str().map(String::from).ok_or_else(|| LocalError::new("PET_GEN_BAD"))
}

/// 在空的临时目录里跑一次，提示词走标准输入（命令行有长度限制），等它说完
async fn by_cli(exe: String, args: &[&str], prompt: &str, out_file: Option<&Path>) -> Result<String, LocalError> {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().map_err(LocalError::from)?;
    let mut cmd = tokio::process::Command::new(&exe);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd.args(args).current_dir(dir.path()).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    // 不要套进看看自己的 hook / 会话里
    cmd.env_remove("TMUX").env("LOOKLOOK_PET_GEN", "1");
    let mut child = cmd.spawn().map_err(|e| LocalError::new("PET_GEN_FAIL").with("detail", e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes()).await;
    }
    let out = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| LocalError::new("PET_GEN_FAIL").with("detail", "timeout"))?
        .map_err(|e| LocalError::new("PET_GEN_FAIL").with("detail", e.to_string()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err: String = err.trim().chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
        return Err(LocalError::new("PET_GEN_FAIL").with("detail", err));
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
