//! 飞书机器人 = 助理的另一个输入框（docs/FEISHU_BOT.md）。
//!
//! - **收**：绑定的人私聊机器人、或在群里 @ 机器人 → 拼成一句话交给助理（`assistant`），跟在助理页上说的一样；
//!   别人发的、别人 @ 机器人的一律不理。引用的那条（谁说的、说了什么）、合并转发的聊天记录、图片和文件一起带上；
//!   1 秒内连发的几条合成一句；收到就加“正在输入”表情，这一轮结束去掉。
//! - **回**：私聊里每一轮一张流式卡片，回复用户那条；群里等这一轮说完，发一条 post 回复用户那条，开头 @ 相关的人（§3）：
//!   被引用的那条的发送人、用户这条里 @ 的人；引用的是机器人自己的回复时，沿用那次 @ 过的人。
//! - 助理派出去的活有了结果，转告推回用户最后说话的地方（`push_assistant`，在 gateway/assistant.rs 里调）。
//! - “停止”停掉助理这一轮；“帮助”回帮助卡片；第一次说话时附一张欢迎卡片。两张卡片上都有飞书 CLI 的下一步（`lark_cli`）。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::assistant::{self, FeishuLink, Who};
use crate::chat::Event;
use crate::error::LocalError;
use crate::feishu::card::{self, Act};
use crate::feishu::{self, Api, Resource};
use crate::gateway::App;
use crate::notify::{self, load_feishu, load_rules, tr, Feishu};

/// 卡片按钮：停止这一轮、飞书 CLI 的下一步
pub const STOP: &str = "bot_stop";
pub const LARK: &str = "bot_lark";

pub fn handles(a: &str) -> bool {
    matches!(a, STOP | LARK)
}

/// 一次最多交给助理多少字
const TEXT_MAX: usize = 4000;
/// 卡片正文最多多少字节（卡片总大小上限 30 KB）
const BODY_BYTES: usize = 18_000;
/// 群里 post 的正文最多多少字节
const POST_BYTES: usize = 12_000;
/// 思考最多带多少字节
const THINK_BYTES: usize = 2_000;
/// 连发的消息隔多久以内算一句
const MERGE_GAP: Duration = Duration::from_millis(1000);
/// 只发了图片、文件时等下一句话等多久
const HOLD: Duration = Duration::from_secs(600);
/// 同一种缺权限的提示多久最多一次
const GRANT_GAP: Duration = Duration::from_secs(12 * 3600);
const TYPING: &str = "Typing";
/// 流式卡片最快多久推一次
const PUSH_GAP: Duration = Duration::from_millis(600);
/// 送进去以后等多久还没跑起来就算了
const START_WAIT_MS: i64 = 120_000;
/// 一轮结束后再等一下（最后的文字可能晚一点到）
const END_SETTLE: Duration = Duration::from_millis(1500);
const FILES_MAX: usize = 9;
const FILE_BYTES: usize = 30 << 20;
const IMAGE_BYTES: u64 = 10 << 20;
/// 一轮最多发回几个产出的文件
const OUTPUTS_MAX: usize = 5;
/// 群里一次最多 @ 几个人
const MENTIONS_MAX: usize = 5;
/// 机器人在群里的回复 → 当时 @ 的人（引用机器人的回复追问时沿用）
const REPLIES_KEY: &str = "feishu_bot.replies";
const REPLIES_MAX: usize = 500;
const REPLY_TTL_MS: i64 = 14 * 24 * 3600 * 1000;
/// 发过欢迎卡片了（换了应用再发一次）
const WELCOMED_KEY: &str = "feishu_bot.welcomed";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 收到的一条（等着跟 1 秒内的下一条合并）
#[derive(Debug, Clone)]
struct In {
    id: String,
    text: String,
    files: Vec<Resource>,
    typing: Option<(String, String)>,
    /// 群里：回复要 @ 的人
    mentions: Vec<Who>,
    group: bool,
    /// 用户说了“别 @”
    no_at: bool,
    at: Instant,
}

#[derive(Default)]
struct Batch {
    gen: u64,
    ins: Vec<In>,
    hinted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Reply {
    m: String,
    at: Vec<Who>,
    t: i64,
}

#[derive(Default)]
pub struct Bot {
    /// 等着合并的消息：会话（私聊 / 群）→ 这一批
    pending: Mutex<HashMap<String, Batch>>,
    /// 这一轮结束时要去掉的“正在输入”表情：大脑 → (消息, 表情 id)
    typing: Mutex<HashMap<String, Vec<(String, String)>>>,
    /// 缺权限的提示上次什么时候发的（按原因）
    asked: Mutex<HashMap<String, Instant>>,
    /// 正在推回复的大脑 → 停止按钮的 nonce
    live: Mutex<HashMap<String, String>>,
    /// 机器人自己的 open_id（应用 id → open_id）
    bot: Mutex<Option<(String, String)>>,
}

impl Bot {
    /// 正在推这个会话的回复
    pub fn following(&self, inst: &str) -> bool {
        lock(&self.live).contains_key(inst)
    }

    /// 流式卡片上的“停止”
    pub async fn stop(&self, app: &App, act: &Act, en: bool) -> Value {
        if lock(&self.live).get(&act.inst) != Some(&act.n) {
            return notify::toast("info", &tr(en, "这一轮已经结束了", "This turn has already ended"));
        }
        if let Err(e) = app.chat.interrupt(&act.inst).await {
            return notify::toast("error", &failed(en, &e));
        }
        tracing::info!(inst = %act.inst, "飞书：停止了助理这一轮");
        notify::toast("success", &tr(en, "已停止", "Stopped"))
    }

    fn hold_typing(&self, inst: &str, list: Vec<(String, String)>) {
        if !list.is_empty() {
            lock(&self.typing).entry(inst.into()).or_default().extend(list);
        }
    }

    async fn release_typing(&self, app: &App, inst: &str) {
        let list = lock(&self.typing).remove(inst).unwrap_or_default();
        if let (false, Some(f)) = (list.is_empty(), load_feishu(&app.store)) {
            drop_typing(&app.notify.api(&f), &list).await;
        }
    }

    async fn bot_id(&self, f: &Feishu, api: &Api) -> Option<String> {
        if let Some((app_id, id)) = lock(&self.bot).clone() {
            if app_id == f.app_id {
                return Some(id);
            }
        }
        match api.bot_open_id().await {
            Ok(id) => {
                *lock(&self.bot) = Some((f.app_id.clone(), id.clone()));
                Some(id)
            }
            Err(e) => {
                tracing::warn!(error = %e, "飞书：取不到机器人的 open_id");
                None
            }
        }
    }
}

/// 机器人开着：助理页的“飞书机器人”开关
pub fn enabled(app: &App) -> bool {
    assistant::settings(&app.store).feishu
}

// ---------------- 消息里的东西 ----------------

/// 消息里的文字：纯文本、富文本（post）、卡片
pub fn message_text(msg: &Value) -> String {
    let c = content(msg);
    let s = match msg["message_type"].as_str() {
        Some("text") => c["text"].as_str().unwrap_or_default().to_string(),
        Some("post") => post_text(&c),
        Some("interactive") => card_text(&c),
        _ => String::new(),
    };
    s.trim().to_string()
}

/// 卡片消息里的文字（开放接口取回的简化格式，或事件里的完整卡片 JSON）
fn card_text(c: &Value) -> String {
    fn walk(v: &Value, line: &mut String, lines: &mut Vec<String>) {
        match v {
            Value::Array(a) => {
                for x in a {
                    walk(x, line, lines);
                }
            }
            Value::Object(o) => {
                let tag = o.get("tag").and_then(Value::as_str).unwrap_or_default();
                let s = ["text", "content"].iter().find_map(|k| o.get(*k).and_then(Value::as_str));
                match (tag, s) {
                    ("text" | "plain_text" | "lark_md" | "markdown" | "a" | "md", Some(s)) => line.push_str(s),
                    ("at", _) => line.push_str(&format!("@{}", o.get("user_name").and_then(Value::as_str).unwrap_or_default())),
                    _ => {
                        for (k, x) in o {
                            if k != "tag" && (x.is_object() || x.is_array()) {
                                walk(x, line, lines);
                            }
                        }
                    }
                }
                if matches!(tag, "div" | "markdown" | "md" | "note" | "column_set" | "hr") || !line.is_empty() && tag.is_empty() {
                    if !line.trim().is_empty() {
                        lines.push(std::mem::take(line));
                    }
                    line.clear();
                }
            }
            _ => {}
        }
    }
    let mut lines = Vec::new();
    if let Some(t) = c["title"].as_str().or_else(|| c["header"]["title"]["content"].as_str()).filter(|t| !t.trim().is_empty()) {
        lines.push(t.to_string());
    }
    let els = if c["elements"].is_array() { &c["elements"] } else { &c["body"]["elements"] };
    for el in els.as_array().into_iter().flatten() {
        let mut line = String::new();
        walk(el, &mut line, &mut lines);
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    lines.iter().map(|l| l.trim_end()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// 认不出文字的消息写成什么
fn placeholder(kind: &str, en: bool) -> Option<String> {
    let (zh, e) = match kind {
        "sticker" => ("[表情]", "[sticker]"),
        "audio" => ("[语音]", "[voice message]"),
        "share_chat" => ("[群名片]", "[group card]"),
        "share_user" => ("[个人名片]", "[contact card]"),
        "location" => ("[位置]", "[location]"),
        "todo" => ("[任务]", "[task]"),
        "vote" => ("[投票]", "[poll]"),
        "hongbao" => ("[红包]", "[red packet]"),
        "calendar" | "share_calendar_event" | "general_calendar" => ("[日程]", "[calendar event]"),
        "video_chat" => ("[视频会议]", "[video meeting]"),
        "system" => return None,
        _ => ("[消息]", "[message]"),
    };
    Some(tr(en, zh, e))
}

fn content(msg: &Value) -> Value {
    msg["content"].as_str().and_then(|c| serde_json::from_str(c).ok()).unwrap_or_default()
}

fn post_body(c: &Value) -> &Value {
    if c["content"].is_array() {
        c
    } else {
        c.as_object().and_then(|m| m.values().find(|v| v["content"].is_array())).unwrap_or(c)
    }
}

fn post_text(c: &Value) -> String {
    let body = post_body(c);
    let mut lines = Vec::new();
    if let Some(t) = body["title"].as_str().filter(|t| !t.is_empty()) {
        lines.push(t.to_string());
    }
    for para in body["content"].as_array().into_iter().flatten() {
        let mut line = String::new();
        for el in para.as_array().into_iter().flatten() {
            match el["tag"].as_str() {
                Some("text" | "a" | "md") => line.push_str(el["text"].as_str().unwrap_or_default()),
                Some("at") => line.push_str(&format!("@{}", el["user_name"].as_str().unwrap_or_default())),
                Some("code_block") => line.push_str(&format!("```{}\n{}\n```", el["language"].as_str().unwrap_or_default().to_lowercase(), el["text"].as_str().unwrap_or_default().trim_end())),
                _ => {}
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// 消息里的图片和文件
pub(crate) fn attachments(msg: &Value) -> Vec<Resource> {
    let c = content(msg);
    let id = msg["message_id"].as_str().unwrap_or_default();
    let res = |kind: &str, key: &Value, name: &Value| key.as_str().filter(|k| !k.is_empty()).map(|k| Resource { kind: kind.into(), key: k.into(), name: name.as_str().unwrap_or_default().into(), msg: id.into() });
    let mut out: Vec<Resource> = match msg["message_type"].as_str() {
        Some("image") => res("image", &c["image_key"], &Value::Null).into_iter().collect(),
        Some("file" | "media") => res("file", &c["file_key"], &c["file_name"]).into_iter().collect(),
        Some("post") => post_body(&c)["content"].as_array().into_iter().flatten().flat_map(|p| p.as_array().into_iter().flatten()).filter(|el| el["tag"] == "img").filter_map(|el| res("image", &el["image_key"], &Value::Null)).collect(),
        _ => vec![],
    };
    out.truncate(FILES_MAX);
    out
}

/// 一个 @ 的 open_id：事件里是 `id.open_id`，开放接口取回的消息里是字符串 `id`
fn mention_id(m: &Value) -> &str {
    m["id"]["open_id"].as_str().or_else(|| m["id"].as_str()).unwrap_or_default()
}

fn mentions_bot(msg: &Value, bot: &str) -> bool {
    msg["mentions"].as_array().into_iter().flatten().any(|m| mention_id(m) == bot)
}

/// 文字里的 `@_user_1` 占位：@ 机器人的去掉，@ 别人的换成名字
fn clean_mentions(text: &str, mentions: &Value, bot: &str) -> String {
    let mut t = text.to_string();
    for m in mentions.as_array().into_iter().flatten() {
        let Some(key) = m["key"].as_str().filter(|k| !k.is_empty()) else { continue };
        let with = if !bot.is_empty() && mention_id(m) == bot { String::new() } else { format!("@{}", m["name"].as_str().unwrap_or_default()) };
        t = t.replace(key, &with);
    }
    t.trim().to_string()
}

/// 用户这条里 @ 的人（不含机器人、用户自己、@所有人）
fn mentioned(msg: &Value, bot: &str, me: &str) -> Vec<Who> {
    msg["mentions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["key"] != "@_all")
        .map(|m| Who { id: mention_id(m).into(), name: m["name"].as_str().unwrap_or_default().into() })
        .filter(|w| w.id.starts_with("ou_") && w.id != bot && w.id != me)
        .collect()
}

fn add_who(list: &mut Vec<Who>, w: Who) {
    match list.iter_mut().find(|x| x.id == w.id) {
        Some(x) if x.name.is_empty() => x.name = w.name,
        Some(_) => {}
        None => list.push(w),
    }
}

/// “停止”：只认整句
fn is_stop(text: &str) -> bool {
    let t = text.trim().trim_end_matches(['。', '.', '!', '！', '~', '～']).trim().to_lowercase();
    matches!(t.as_str(), "停止" | "停" | "停下" | "停一下" | "别做了" | "stop" | "/stop" | "abort")
}

fn is_help(text: &str) -> bool {
    matches!(text.trim().to_lowercase().as_str(), "/help" | "help" | "帮助" | "/帮助" | "怎么用")
}

/// 用户说了这次别 @ 人
fn says_no_at(text: &str) -> bool {
    let t = text.to_lowercase().replace(' ', "");
    ["别@", "不用@", "不要@", "别艾特", "不用艾特", "不要艾特", "don't@", "dont@", "no@", "without@"].iter().any(|k| t.contains(k))
}

fn as_event_msg(m: &Value) -> Value {
    json!({ "message_id": m["message_id"], "message_type": m["msg_type"], "content": m["body"]["content"] })
}

/// 合并转发的聊天记录：子消息一条一段，写明是谁说的（你自己写“我”，别人按出现顺序 A、B…，只有一个人写“对方”）
fn forwarded(id: &str, items: &[Value], me: &str, en: bool) -> (String, Vec<Resource>) {
    fn subs_of<'a>(items: &'a [Value], id: &str, depth: usize, out: &mut Vec<&'a Value>) {
        for m in items.iter().filter(|m| m["upper_message_id"] == id) {
            if m["msg_type"] == "merge_forward" && depth < 3 {
                if let Some(sub) = m["message_id"].as_str() {
                    subs_of(items, sub, depth + 1, out);
                    continue;
                }
            }
            out.push(m);
        }
    }
    let mut subs: Vec<&Value> = Vec::new();
    subs_of(items, id, 0, &mut subs);
    let who = |m: &Value| m["sender"]["id"].as_str().filter(|_| m["sender"]["sender_type"] == "user").unwrap_or_default().to_string();
    let mut others: Vec<String> = Vec::new();
    for m in &subs {
        let w = who(m);
        if !w.is_empty() && w != me && !others.contains(&w) {
            others.push(w);
        }
    }
    let label = |w: &str| -> String {
        if !me.is_empty() && w == me {
            return tr(en, "我", "Me");
        }
        match others.iter().position(|x| x == w) {
            Some(_) if others.len() == 1 => tr(en, "对方", "Them"),
            Some(i) => char::from(b'A' + (i % 26) as u8).to_string(),
            None => String::new(),
        }
    };
    let mut lines = Vec::new();
    let mut files = Vec::new();
    for m in &subs {
        let e = as_event_msg(m);
        let found = attachments(&e);
        let mut t = clean_mentions(&message_text(&e), &m["mentions"], "");
        if t.is_empty() && found.is_empty() {
            t = placeholder(m["msg_type"].as_str().unwrap_or_default(), en).unwrap_or_default();
        }
        if !t.is_empty() {
            let w = label(&who(m));
            let body = quote_lines(&t);
            lines.push(if w.is_empty() { body } else { format!("> **{w}**：\n{body}") });
        }
        files.extend(found);
    }
    if lines.is_empty() {
        return (String::new(), files);
    }
    (format!("{}\n{}", tr(en, "转发的聊天记录：", "Forwarded chat history:"), lines.join("\n>\n")), files)
}

fn quote_lines(t: &str) -> String {
    t.lines().map(|l| format!("> {l}")).collect::<Vec<_>>().join("\n")
}

/// 引用的那一条
struct Quote {
    text: String,
    /// 发送人 open_id（是人的时候）
    user: Option<String>,
    /// 是这个机器人自己发的
    mine: bool,
    files: Vec<Resource>,
}

async fn quoted(app: &App, api: &Api, f: &Feishu, id: &str, en: bool) -> Option<Quote> {
    match api.get_message(id).await {
        Ok(items) => {
            let first = items.first()?;
            let sender = &first["sender"];
            let mine = sender["sender_type"] == "app" && sender["id"] == f.app_id.as_str();
            let user = sender["id"].as_str().filter(|_| sender["sender_type"] == "user").map(String::from);
            let me = f.open_id.clone().unwrap_or_default();
            let (text, files) = if first["msg_type"] == "merge_forward" {
                forwarded(first["message_id"].as_str().unwrap_or(id), &items, &me, en)
            } else {
                let e = as_event_msg(first);
                (clean_mentions(&message_text(&e), &first["mentions"], ""), attachments(&e))
            };
            let max = if first["msg_type"] == "merge_forward" { 6000 } else { 2000 };
            Some(Quote { text: text.chars().take(max).collect(), user, mine, files })
        }
        Err(e) => {
            scope_hint(app, api, &e, "read").await;
            None
        }
    }
}

// ---------------- 记下的回复 ----------------

fn replies(app: &App) -> Vec<Reply> {
    app.store.get::<Vec<Reply>>(REPLIES_KEY).ok().flatten().unwrap_or_default()
}

fn note_reply(app: &App, m: &str, at: &[Who]) {
    let now = crate::util::system_ms();
    let mut all = replies(app);
    all.retain(|r| now - r.t < REPLY_TTL_MS && r.m != m);
    all.push(Reply { m: m.into(), at: at.to_vec(), t: now });
    if all.len() > REPLIES_MAX {
        all.drain(..all.len() - REPLIES_MAX);
    }
    if let Err(e) = app.store.set(REPLIES_KEY, &all) {
        tracing::warn!(error = %e, "飞书：回复 @ 了谁没存上");
    }
}

fn reply_mentions(app: &App, m: &str) -> Vec<Who> {
    replies(app).into_iter().rev().find(|r| r.m == m).map(|r| r.at).unwrap_or_default()
}

// ---------------- 小工具 ----------------

async fn reply(api: &Api, to: &str, text: &str) {
    if let Err(e) = api.reply_text(to, text).await {
        tracing::warn!(error = %e, "飞书：回复没发出去");
    }
}

fn failed(en: bool, e: &LocalError) -> String {
    tr(en, &format!("没办成：{}", e.code), &format!("That didn't work: {}", e.code))
}

async fn drop_typing(api: &Api, list: &[(String, String)]) {
    for (m, r) in list {
        if let Err(e) = api.remove_reaction(m, r).await {
            tracing::debug!(error = %e, "飞书：表情没去掉");
        }
    }
}

/// 缺权限：发一张卡片，带一个补权限的链接。同一种原因 12 小时最多一次
async fn scope_hint(app: &App, api: &Api, e: &anyhow::Error, why: &str) {
    if !feishu::code_of(e).is_some_and(feishu::is_scope_error) {
        return;
    }
    {
        let mut g = lock(&app.notify.bot.asked);
        if g.get(why).is_some_and(|at| at.elapsed() < GRANT_GAP) {
            return;
        }
        g.insert(why.into(), Instant::now());
    }
    let Some(f) = load_feishu(&app.store) else { return };
    let Some(to) = f.open_id.clone() else { return };
    let en = load_rules(&app.store).lang.starts_with("en");
    let reg = match app.notify.reg_start(app, true).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = ?e, "飞书：补权限的链接没拿到");
            return;
        }
    };
    let text = match why {
        "typing" => tr(en, "收到消息时加“正在输入”的表情，要开通“发送、删除消息表情回复”。", "Showing a “typing” reaction needs the message-reactions scope."),
        "read" => tr(en, "读引用、合并转发的消息，要开通“读取消息”。", "Reading quoted and forwarded messages needs the read-messages scope."),
        "cardkit" => tr(en, "边写边出的回复卡片要开通“创建与更新卡片”。", "Streaming reply cards need the card-update scope."),
        _ => tr(en, "看看用到的功能缺少权限。", "A feature LookLook uses is missing a scope."),
    };
    let msg = match api.send_card(&to, &card::grant(en, &text, &reg.url)).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "飞书：补权限的卡片没发出去");
            return;
        }
    };
    let (app, id) = (app.clone(), reg.id);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            match app.notify.reg() {
                Some(r) if r.id == id && r.state == "pending" => continue,
                Some(r) if r.id == id && r.state == "done" => {
                    let Some(f) = load_feishu(&app.store) else { return };
                    let _ = app.notify.api(&f).patch_card(&msg, &card::granted(en)).await;
                    return;
                }
                _ => return,
            }
        }
    });
}

fn page(app: &App, path: &str) -> String {
    notify::page_url(app, path)
}

// ---------------- 收消息 ----------------

/// 绑定的人发来一条消息（私聊里绑定码之外的；群里的）
pub async fn on_message(app: &App, f: &Feishu, ev: &Value) {
    let msg = &ev["message"];
    let Some(id) = msg["message_id"].as_str() else { return };
    let en = load_rules(&app.store).lang.starts_with("en");
    let api = app.notify.api(f);
    let chat = msg["chat_id"].as_str().unwrap_or_default().to_string();
    let group = msg["chat_type"] == "group";
    let me = f.open_id.clone().unwrap_or_default();
    let mut text = message_text(msg);
    let mut mentions = Vec::new();
    if group {
        // 群里只认 @ 了机器人的（飞书默认也只推这些）
        let Some(bot) = app.notify.bot.bot_id(f, &api).await else { return };
        if !mentions_bot(msg, &bot) {
            return;
        }
        text = clean_mentions(&text, &msg["mentions"], &bot);
        mentions = mentioned(msg, &bot, &me);
    }
    if !enabled(app) {
        return reply(&api, id, &tr(en, "飞书机器人没有打开：到看看的助理页打开「飞书机器人」。", "The Feishu bot is off. Turn it on in LookLook → Assistant.")).await;
    }
    if is_stop(&text) {
        return stop_cmd(app, &api, id, en).await;
    }
    if is_help(&text) {
        return help_card(app, &api, id, en).await;
    }
    let no_at = says_no_at(&text);
    let mut files = attachments(msg);
    if msg["message_type"] == "merge_forward" {
        match api.get_message(id).await {
            Ok(items) => {
                let (t, fs) = forwarded(id, &items, &me, en);
                text = [text, t].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
                files.extend(fs);
            }
            Err(e) => {
                scope_hint(app, &api, &e, "read").await;
                return reply(&api, id, &tr(en, "读不了这段转发的聊天记录。", "Couldn't read the forwarded chat history.")).await;
            }
        }
    }
    // 引用的那条：原文带上；群里被引用的人是别人时，回复 @ 他；引用的是机器人自己的回复时沿用那次的 @
    if let Some(parent) = msg["parent_id"].as_str().filter(|p| !p.is_empty() && *p != id) {
        if let Some(q) = quoted(app, &api, f, parent, en).await {
            let name = q.user.as_ref().and_then(|u| mentions.iter().find(|w: &&Who| &w.id == u)).map(|w| w.name.clone()).unwrap_or_default();
            let who = if q.mine {
                tr(en, "你（机器人）之前的回复", "your (the bot's) earlier reply")
            } else if q.user.as_deref() == Some(me.as_str()) {
                tr(en, "用户自己", "the user")
            } else if !name.is_empty() {
                name.clone()
            } else {
                tr(en, "别人", "someone else")
            };
            if !q.text.is_empty() {
                text = format!("{}\n{}\n\n{text}", tr(en, &format!("引用了{who}的消息："), &format!("Quoting {who}:")), quote_lines(&q.text));
            }
            files.extend(q.files);
            if group {
                match (&q.user, q.mine) {
                    (Some(u), _) if *u != me => {
                        let w = Who { id: u.clone(), name };
                        // 被引用的人排第一个
                        mentions.retain(|x| x.id != w.id);
                        mentions.insert(0, w);
                    }
                    (_, true) => {
                        for w in reply_mentions(app, parent) {
                            add_who(&mut mentions, w);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    files.truncate(FILES_MAX);
    if text.is_empty() && files.is_empty() {
        return reply(&api, id, &tr(en, "暂时只支持文字、图片、文件和合并转发的聊天记录。", "Only text, images, files and forwarded chat history are supported for now.")).await;
    }
    if text.chars().count() > TEXT_MAX + 6000 {
        return reply(&api, id, &tr(en, &format!("太长了，一次最多 {TEXT_MAX} 字。"), &format!("Too long: at most {TEXT_MAX} characters."))).await;
    }
    let typing = match api.add_reaction(id, TYPING).await {
        Ok(r) => Some((id.to_string(), r)),
        Err(e) => {
            tracing::debug!(error = %e, "飞书：表情没加上");
            scope_hint(app, &api, &e, "typing").await;
            None
        }
    };
    enqueue(app, &chat, In { id: id.into(), text, files, typing, mentions, group, no_at, at: Instant::now() });
}

/// 1 秒内连发的合成一批；只有文件时先存着
fn enqueue(app: &App, chat: &str, input: In) {
    let key = chat.to_string();
    let me = &app.notify.bot;
    let gen = {
        let mut g = lock(&me.pending);
        let b = g.entry(key.clone()).or_default();
        b.ins.retain(|x| x.at.elapsed() < HOLD);
        b.ins.push(input);
        b.gen += 1;
        b.gen
    };
    let (app, chat) = (app.clone(), chat.to_string());
    tokio::spawn(async move {
        tokio::time::sleep(MERGE_GAP).await;
        let me = &app.notify.bot;
        let (ins, hint) = {
            let mut g = lock(&me.pending);
            let Some(b) = g.get_mut(&key).filter(|b| b.gen == gen) else { return };
            if b.ins.iter().all(|x| x.text.is_empty()) {
                let first = !std::mem::replace(&mut b.hinted, true);
                (b.ins.clone(), first)
            } else {
                (g.remove(&key).map(|b| b.ins).unwrap_or_default(), false)
            }
        };
        let Some(f) = load_feishu(&app.store) else { return };
        let api = app.notify.api(&f);
        let en = load_rules(&app.store).lang.starts_with("en");
        let typing: Vec<(String, String)> = ins.iter().filter_map(|x| x.typing.clone()).collect();
        let Some(last) = ins.last().cloned() else { return };
        if ins.iter().all(|x| x.text.is_empty()) {
            drop_typing(&api, &typing).await;
            if hint {
                let n: usize = ins.iter().map(|x| x.files.len()).sum();
                reply(&api, &last.id, &tr(en, &format!("收到 {n} 个图片 / 文件。接着说要拿它们做什么（10 分钟内）。"), &format!("Got {n} image(s) / file(s). Tell me what to do with them (within 10 minutes)."))).await;
            }
            return;
        }
        if let Err(e) = deliver(&app, &api, &chat, ins, typing.clone(), en).await {
            drop_typing(&api, &typing).await;
            reply(&api, &last.id, &failed(en, &e)).await;
        }
    });
}

/// 群里回复要 @ 的人：几条合起来，说了“别 @”就不 @
fn merged_mentions(ins: &[In]) -> Vec<Who> {
    if ins.iter().any(|x| x.no_at) {
        return vec![];
    }
    let mut out = Vec::new();
    for x in ins {
        for w in &x.mentions {
            add_who(&mut out, w.clone());
        }
    }
    out.truncate(MENTIONS_MAX);
    out
}

/// 送进助理的那句话前面的来源行
fn source_line(group: bool, mentions: &[Who], lark: Option<bool>, en: bool) -> String {
    let mut parts = vec![tr(en, "飞书", "Feishu")];
    if group {
        parts.push(tr(en, "群聊（群里的人都看得到你的回答）", "group chat (everyone in it sees your answer)"));
        if !mentions.is_empty() {
            let names: Vec<String> = mentions.iter().map(|w| if w.name.is_empty() { tr(en, "被引用的人", "the quoted person") } else { w.name.clone() }).collect();
            parts.push(tr(en, &format!("回答会自动 @ {}，你不用写 @", names.join("、")), &format!("your answer will @ {} automatically; don't write @ yourself", names.join(", "))));
        }
    } else {
        parts.push(tr(en, "私聊", "direct message"));
    }
    if lark == Some(false) {
        parts.push(tr(en, "飞书 CLI 还没就绪：要读写飞书（文档、日历、聊天记录）时请用户发“帮助”按卡片装好", "Feishu CLI isn't ready: to read or write Feishu, ask the user to send “help” and follow the card"));
    }
    format!("[{}]", parts.join(" · "))
}

/// 一批消息交给助理
async fn deliver(app: &App, api: &Api, chat: &str, ins: Vec<In>, typing: Vec<(String, String)>, en: bool) -> Result<(), LocalError> {
    let Some(last) = ins.last().cloned() else { return Ok(()) };
    let Some(brain) = assistant::feishu_brain(&app.store) else {
        drop_typing(api, &typing).await;
        let url = page(app, "assistant");
        reply(api, &last.id, &tr(en, &format!("助理还没打开。在看看里打开助理就能在这里跟它说话：{url}"), &format!("The assistant isn't on yet. Turn it on in LookLook and you can talk to it here: {url}"))).await;
        return Ok(());
    };
    let text = ins.iter().map(|x| x.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n");
    let files: Vec<Resource> = ins.iter().flat_map(|x| x.files.clone()).take(FILES_MAX).collect();
    let group = last.group;
    let mentions = if group { merged_mentions(&ins) } else { vec![] };
    let lark = crate::lark_cli::ready_cached();
    let text = format!("{}\n{text}", source_line(group, &mentions, lark, en));
    assistant::note_feishu(&app.store, FeishuLink { chat: chat.into(), msg: last.id.clone(), at: crate::util::system_ms(), group, mentions: mentions.clone() });
    say(app, &brain, &text, &last.id, &files, typing, group.then_some(mentions)).await?;
    welcome(app, api, &last.id, en).await;
    // 刷新飞书 CLI 的状态（下一句用）
    if lark.is_none() {
        let shell = app.instances.shell().to_string();
        tokio::spawn(async move {
            crate::lark_cli::status(&shell, false).await;
        });
    }
    Ok(())
}

/// 送进大脑，开一个任务把这一轮的回复推回飞书。`group`：群里，要 @ 的人
async fn say(app: &App, inst: &str, text: &str, msg: &str, files: &[Resource], typing: Vec<(String, String)>, group: Option<Vec<Who>>) -> Result<(), LocalError> {
    let me = &app.notify.bot;
    let base = if me.following(inst) { None } else { app.chat.snapshot(inst).await?["total"].as_u64() };
    let since = crate::util::system_ms();
    let (text, images) = with_files(app, inst, msg, files, text.to_string()).await?;
    app.instances.note_input(inst);
    app.chat.input(inst, &text, &images).await?;
    me.hold_typing(inst, typing);
    tracing::info!(group = group.is_some(), "飞书：一句话交给了助理");
    match base {
        Some(base) => drop(tokio::spawn(follow(app.clone(), inst.to_string(), msg.to_string(), base as usize, since, group))),
        None if !me.following(inst) => me.release_typing(app, inst).await,
        None => {}
    }
    Ok(())
}

/// 助理（gateway/assistant.rs）：派出去的活有了结果，大脑这一轮的转告推回用户最后说话的地方
pub fn push_assistant(app: &App, inst: &str, link: &FeishuLink, base: usize, since: i64) {
    let ok = load_feishu(&app.store).is_some_and(|f| f.open_id.is_some());
    if !ok || !enabled(app) || app.notify.bot.following(inst) {
        return;
    }
    let group = link.group.then(|| link.mentions.clone());
    tokio::spawn(follow(app.clone(), inst.to_string(), link.msg.clone(), base, since, group));
}

async fn follow(app: App, inst: String, reply_to: String, base: usize, since: i64, group: Option<Vec<Who>>) {
    let me = &app.notify.bot;
    let nonce = notify::nonce();
    {
        let mut g = lock(&me.live);
        if g.contains_key(&inst) {
            return;
        }
        g.insert(inst.clone(), nonce.clone());
    }
    let r = match &group {
        Some(at) => group_turn(&app, &inst, &reply_to, base, since, at).await,
        None => stream_turn(&app, &inst, &reply_to, base, since, &nonce).await,
    };
    if let Err(e) = r {
        tracing::warn!(error = %e, "飞书：助理的回复没推完");
    }
    lock(&me.live).remove(&inst);
    me.release_typing(&app, &inst).await;
}

async fn stop_cmd(app: &App, api: &Api, id: &str, en: bool) {
    let Some(brain) = assistant::brain_id(&app.store) else { return reply(api, id, &tr(en, "助理没在干活。", "The assistant isn't doing anything.")).await };
    let running = app.chat.brief(&brain).is_some_and(|b| matches!(b["board"].as_str(), Some("working" | "attn")));
    if !running {
        return reply(api, id, &tr(en, "助理没在干活。派出去的活要停，就直接说停哪个。", "The assistant isn't working right now. To stop a task it handed off, say which one.")).await;
    }
    match app.chat.interrupt(&brain).await {
        Ok(()) => reply(api, id, &tr(en, "已停止。", "Stopped.")).await,
        Err(e) => reply(api, id, &failed(en, &e)).await,
    }
}

// ---------------- 帮助、欢迎、飞书 CLI ----------------

async fn help_card(app: &App, api: &Api, id: &str, en: bool) {
    let st = crate::lark_cli::status(app.instances.shell(), true).await;
    let c = card::bot_help(en, &st, &lark_act(app), &page(app, "assistant"), false);
    if let Err(e) = api.reply(id, "interactive", &c, false).await {
        app.notify.record_err(&e);
    }
}

/// 第一次说话：附一张欢迎卡片（怎么用 + 飞书 CLI）。换了应用再发一次
async fn welcome(app: &App, api: &Api, id: &str, en: bool) {
    let Some(f) = load_feishu(&app.store) else { return };
    if app.store.get::<String>(WELCOMED_KEY).ok().flatten().as_deref() == Some(f.app_id.as_str()) {
        return;
    }
    let _ = app.store.set(WELCOMED_KEY, &f.app_id);
    let st = crate::lark_cli::status(app.instances.shell(), true).await;
    let c = card::bot_help(en, &st, &lark_act(app), &page(app, "assistant"), true);
    if let Err(e) = api.reply(id, "interactive", &c, false).await {
        app.notify.record_err(&e);
    }
}

fn lark_act(app: &App) -> Act {
    let own = app.account.session().map(|s| s.device_id).unwrap_or_default();
    Act { a: LARK.into(), dev: own, n: notify::nonce(), ..Default::default() }
}

/// 卡片按钮
pub async fn on_card(app: &App, act: &Act, ev: &Value) -> Value {
    let en = load_rules(&app.store).lang.starts_with("en");
    match act.a.as_str() {
        STOP => app.notify.bot.stop(app, act, en).await,
        LARK => {
            let Some(card_msg) = ev["context"]["open_message_id"].as_str().map(String::from) else { return notify::toast("error", "?") };
            let app2 = app.clone();
            let shell = app.instances.shell().to_string();
            // 这一步结束时：把卡片换成下一步
            let done_app = app.clone();
            let done_msg = card_msg.clone();
            let done = move |r: Result<(), String>| {
                tokio::spawn(async move {
                    if let Err(e) = &r {
                        tracing::warn!(error = %e, "飞书 CLI：这一步没成");
                    }
                    lark_refresh(&done_app, &done_msg, en).await;
                });
            };
            tokio::spawn(async move {
                match crate::lark_cli::next(&shell, done).await {
                    Ok(_) => lark_refresh(&app2, &card_msg, en).await,
                    Err(e) => {
                        let Some(f) = load_feishu(&app2.store) else { return };
                        let mut st = crate::lark_cli::status(&shell, true).await;
                        st.error = Some(if e.code == "LARK_NEED_NODE" { tr(en, "这台电脑没有 npm，先装 Node.js。", "This computer has no npm. Install Node.js first.") } else { e.code.clone() });
                        let _ = app2.notify.api(&f).patch_card(&card_msg, &card::bot_help(en, &st, &lark_act(&app2), &page(&app2, "assistant"), false)).await;
                    }
                }
            });
            notify::toast("info", &tr(en, "处理中…", "Working on it…"))
        }
        _ => notify::toast("error", "?"),
    }
}

async fn lark_refresh(app: &App, card_msg: &str, en: bool) {
    let Some(f) = load_feishu(&app.store) else { return };
    let st = crate::lark_cli::status(app.instances.shell(), true).await;
    if let Err(e) = app.notify.api(&f).patch_card(card_msg, &card::bot_help(en, &st, &lark_act(app), &page(app, "assistant"), false)).await {
        tracing::debug!(error = %e, "飞书：飞书 CLI 卡片没改上");
    }
}

// ---------------- 文件 ----------------

/// 把消息里的图片和文件下载到会话的附件目录：图片交给 AI，其它文件把路径写在话后面
async fn with_files(app: &App, inst: &str, src: &str, files: &[Resource], text: String) -> Result<(String, Vec<String>), LocalError> {
    if files.is_empty() {
        return Ok((text, vec![]));
    }
    let f = load_feishu(&app.store).ok_or_else(|| LocalError::new("NOTIFY_NOT_CONFIGURED"))?;
    let api = app.notify.api(&f);
    let dir = app.paths.attach_dir(inst);
    tokio::fs::create_dir_all(&dir).await.map_err(anyhow::Error::from)?;
    let (mut images, mut others) = (Vec::new(), Vec::new());
    let stamp = crate::util::system_ms();
    for (n, r) in files.iter().take(FILES_MAX).enumerate() {
        let kind = if r.kind == "image" { "image" } else { "file" };
        let from = if r.msg.is_empty() { src } else { &r.msg };
        let bytes = api.download(from, &r.key, kind, FILE_BYTES).await.map_err(|e| notify::feishu_error(&e))?;
        let name = if kind == "image" { format!("image.{}", image_ext(&bytes)) } else { crate::transfer::sanitize_name(&r.name) };
        let name = if name.is_empty() { "file".to_string() } else { name };
        let path = dir.join(format!("feishu-{stamp}-{n}-{name}"));
        tokio::fs::write(&path, &bytes).await.map_err(anyhow::Error::from)?;
        let p = path.display().to_string();
        if kind == "image" { images.push(p) } else { others.push(p) }
    }
    let mut text = text;
    if !others.is_empty() {
        let en = load_rules(&app.store).lang.starts_with("en");
        let list = others.join("\n");
        text = format!("{text}\n\n{}\n{list}", tr(en, "附件：", "Attachments:"));
    }
    Ok((text, images))
}

fn image_ext(b: &[u8]) -> &'static str {
    match b {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'G', b'I', b'F', ..] => "gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'B', b'M', ..] => "bmp",
        _ => "png",
    }
}

// ---------------- 回复 ----------------

struct Streamer<'a> {
    api: &'a Api,
    card: String,
    seq: u32,
    body: String,
    status: String,
    tools: String,
}

impl Streamer<'_> {
    async fn text(&mut self, element: &str, content: &str) -> anyhow::Result<()> {
        self.seq += 1;
        match self.api.card_text(&self.card, element, content, self.seq).await {
            // 流式开了 10 分钟会自动关：重新打开再推
            Err(e) if feishu::code_of(&e).is_some_and(feishu::is_stream_closed) => {
                self.seq += 1;
                self.api.card_settings(&self.card, &json!({ "streaming_mode": true }), self.seq).await?;
                self.seq += 1;
                self.api.card_text(&self.card, element, content, self.seq).await
            }
            r => r,
        }
    }

    async fn push(&mut self, body: String, status: String, tools: String) {
        if !tools.is_empty() && tools != self.tools {
            if let Err(e) = self.text(card::STREAM_TOOLS, &tools).await {
                tracing::debug!(error = %e, "飞书：工具步骤没推上");
            }
            self.tools = tools;
        }
        if body != self.body {
            if let Err(e) = self.text(card::STREAM_BODY, &body).await {
                tracing::debug!(error = %e, "飞书：正文没推上");
            }
            self.body = body;
        }
        if status != self.status {
            if let Err(e) = self.text(card::STREAM_STATUS, &status).await {
                tracing::debug!(error = %e, "飞书：状态行没推上");
            }
            self.status = status;
        }
    }

    async fn finish(&mut self, c: &Value) -> anyhow::Result<()> {
        self.seq += 1;
        if let Err(e) = self.api.card_settings(&self.card, &json!({ "streaming_mode": false }), self.seq).await {
            tracing::debug!(error = %e, "飞书：流式没关上");
        }
        self.seq += 1;
        self.api.card_update(&self.card, c, self.seq).await
    }
}

/// 卡片上的信息：助理 · 这台电脑
fn info_strings(app: &App, en: bool) -> (String, String, String) {
    let device = crate::settings::Settings::load(&app.store).device_name();
    (tr(en, "看看助理", "LookLook assistant"), device, page(app, "assistant"))
}

/// 这一轮结束了没有：(结果, 用时)
fn turn_end(b: &Value, since: i64) -> Option<(&'static str, i64)> {
    let turn = &b["turn"];
    let done = turn["ended_at"].as_i64().is_some_and(|e| e > since) && !matches!(b["phase"].as_str(), Some("running" | "approval"));
    if !done {
        return None;
    }
    let outcome = match turn["outcome"].as_str() {
        Some("done") => "done",
        Some("failed") => "failed",
        _ => "stopped",
    };
    let end = turn["ended_at"].as_i64().unwrap_or(since);
    Some((outcome, end - turn["started_at"].as_i64().unwrap_or(end)))
}

/// 私聊：一轮的回复推到一张流式卡片上，直到这一轮结束
async fn stream_turn(app: &App, inst: &str, reply_to: &str, base: usize, since: i64, nonce: &str) -> anyhow::Result<()> {
    let f = load_feishu(&app.store).ok_or_else(|| anyhow!("没有配置飞书"))?;
    let api = app.notify.api(&f);
    let en = load_rules(&app.store).lang.starts_with("en");
    let (mut rx, _viewer) = app.chat.subscribe(inst).map_err(|e| anyhow!("订阅不了会话：{}", e.code))?;
    let (agent, device, url) = info_strings(app, en);
    let info = card::Info { en, agent: &agent, title: "", project: "", device: &device, url: &url, content: true, actions: true };
    let own = app.account.session().map(|s| s.device_id).unwrap_or_default();
    let stop = Act { a: STOP.into(), dev: own, inst: inst.into(), n: nonce.into(), ..Default::default() };
    let card_id = match api.card_create(&card::stream(&info, &stop)).await {
        Ok(c) => c,
        Err(e) => {
            app.notify.record_err(&e);
            scope_hint(app, &api, &e, "cardkit").await;
            return Err(e);
        }
    };
    let msg = api.reply_card_entity(reply_to, &card_id, false).await?;
    let mut s = Streamer { api: &api, card: card_id, seq: 0, body: "…".into(), status: String::new(), tools: String::new() };
    let mut items: BTreeMap<usize, Value> = BTreeMap::new();
    resnap(app, inst, base, &mut items).await;
    let mut last_push = Instant::now() - PUSH_GAP;
    let mut ran = false;
    let mut settling: Option<Instant> = None;
    let (outcome, took) = loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(Event::Items { start, items: got }) => {
                    for (k, it) in got.into_iter().enumerate() {
                        if start + k >= base {
                            items.insert(start + k, serde_json::to_value(it).unwrap_or_default());
                        }
                    }
                }
                Ok(Event::Reset) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => resnap(app, inst, base, &mut items).await,
                Ok(Event::Status(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break ("stopped", 0),
            },
            _ = tokio::time::sleep(PUSH_GAP) => {}
        }
        let Some(b) = app.chat.brief(inst) else { break ("stopped", 0) };
        ran |= matches!(b["board"].as_str(), Some("working" | "attn"));
        if last_push.elapsed() >= PUSH_GAP {
            let steps = steps_of(&items);
            let tools = if steps.is_empty() { String::new() } else { card::steps_markdown(&steps, en) };
            s.push(body_of(&items, BODY_BYTES, en), status_line(&b, en), tools).await;
            last_push = Instant::now();
        }
        if let Some(end) = turn_end(&b, since) {
            if settling.get_or_insert_with(Instant::now).elapsed() >= END_SETTLE {
                break end;
            }
        } else {
            settling = None;
        }
        if b["board"] == "stopped" && (ran || crate::util::system_ms() - since > START_WAIT_MS) {
            break ("stopped", 0);
        }
    };
    resnap(app, inst, base, &mut items).await;
    let body = body_of(&items, BODY_BYTES, en);
    let done = card::stream_done(&info, outcome, if body == "…" { "" } else { &body }, &steps_of(&items), &thinking_of(&items), took);
    let r = s.finish(&done).await;
    send_outputs(app, &api, &msg, inst, &items, since).await;
    r
}

/// 群里：等这一轮说完，发一条 post 回复用户那条，开头 @ 相关的人
async fn group_turn(app: &App, inst: &str, reply_to: &str, base: usize, since: i64, at: &[Who]) -> anyhow::Result<()> {
    let f = load_feishu(&app.store).ok_or_else(|| anyhow!("没有配置飞书"))?;
    let api = app.notify.api(&f);
    let en = load_rules(&app.store).lang.starts_with("en");
    let mut ran = false;
    let mut settling: Option<Instant> = None;
    let outcome = loop {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let Some(b) = app.chat.brief(inst) else { break "stopped" };
        ran |= matches!(b["board"].as_str(), Some("working" | "attn"));
        if let Some((o, _)) = turn_end(&b, since) {
            if settling.get_or_insert_with(Instant::now).elapsed() >= END_SETTLE {
                break o;
            }
        } else {
            settling = None;
        }
        if b["board"] == "stopped" && (ran || crate::util::system_ms() - since > START_WAIT_MS) {
            break "stopped";
        }
    };
    let mut items: BTreeMap<usize, Value> = BTreeMap::new();
    resnap(app, inst, base, &mut items).await;
    let mut body = body_of(&items, POST_BYTES, en);
    if body == "…" {
        body = match outcome {
            "failed" => tr(en, "出错了，没答上来。", "Something went wrong; no answer."),
            "stopped" => tr(en, "已停止。", "Stopped."),
            _ => return Ok(()),
        };
    } else if outcome == "failed" {
        body.push_str(&tr(en, "\n\n（出错了，可能没说完）", "\n\n(Something went wrong; this may be incomplete)"));
    }
    let msg = api.reply(reply_to, "post", &post_content(at, &body), false).await?;
    note_reply(app, &msg, at);
    send_outputs(app, &api, &msg, inst, &items, since).await;
    Ok(())
}

/// 群里的回复：第一段 @ 人，第二段 markdown 正文
fn post_content(at: &[Who], body: &str) -> Value {
    let mut content = Vec::new();
    if !at.is_empty() {
        let mut p: Vec<Value> = at.iter().map(|w| json!({ "tag": "at", "user_id": w.id })).collect();
        p.push(json!({ "tag": "text", "text": " " }));
        content.push(Value::Array(p));
    }
    content.push(json!([{ "tag": "md", "text": body }]));
    json!({ "zh_cn": { "content": content } })
}

/// 这一轮回复里提到的、这一轮新写出来的文件（图片、PDF、文档等）发回去
async fn send_outputs(app: &App, api: &Api, to: &str, inst: &str, items: &BTreeMap<usize, Value>, since: i64) {
    let Ok(Some(row)) = app.store.instance(inst) else { return };
    let texts: Vec<String> = assistant_texts(items).into_iter().map(String::from).collect();
    if texts.is_empty() {
        return;
    }
    let workdir = row.workdir.clone();
    let found = tokio::task::spawn_blocking(move || {
        use crate::chat::codex_artifacts as art;
        art::find(&texts, Path::new(&workdir), &art::system_dirs())
    })
    .await
    .unwrap_or_default();
    let mut sent = 0;
    for (p, _) in found {
        if sent >= OUTPUTS_MAX {
            break;
        }
        let kind = crate::chat::codex_artifacts::kind(&p);
        let Ok(m) = tokio::fs::metadata(&p).await else { continue };
        let fresh = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).is_some_and(|d| d.as_millis() as i64 >= since - 5_000);
        let limit = if kind == "image" { IMAGE_BYTES } else { FILE_BYTES as u64 };
        if kind == "text" || !fresh || m.len() == 0 || m.len() > limit {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&p).await else { continue };
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let r = if kind == "image" {
            match api.upload_image(&name, bytes).await {
                Ok(k) => api.reply(to, "image", &json!({ "image_key": k }), false).await,
                Err(e) => Err(e),
            }
        } else {
            match api.upload_file(&name, bytes).await {
                Ok(k) => api.reply(to, "file", &json!({ "file_key": k }), false).await,
                Err(e) => Err(e),
            }
        };
        match r {
            Ok(_) => sent += 1,
            Err(e) => {
                app.notify.record_err(&e);
                break;
            }
        }
    }
}

async fn resnap(app: &App, inst: &str, base: usize, items: &mut BTreeMap<usize, Value>) {
    let Ok(snap) = app.chat.snapshot(inst).await else { return };
    let start = snap["start"].as_u64().unwrap_or(0) as usize;
    for (k, it) in snap["items"].as_array().into_iter().flatten().enumerate() {
        if start + k >= base {
            items.insert(start + k, it.clone());
        }
    }
}

fn assistant_texts(items: &BTreeMap<usize, Value>) -> Vec<&str> {
    items
        .values()
        .filter(|it| it["k"] == "assistant")
        .flat_map(|it| it["blocks"].as_array().into_iter().flatten())
        .filter(|b| b["t"] == "text")
        .filter_map(|b| b["text"].as_str().map(str::trim))
        .filter(|t| !t.is_empty())
        .collect()
}

/// 这一轮 AI 说的话；太长只留最后的
fn body_of(items: &BTreeMap<usize, Value>, max: usize, en: bool) -> String {
    let text = assistant_texts(items).join("\n\n");
    if text.is_empty() {
        return "…".into();
    }
    let text = if text.len() > max {
        let mut cut = text.len() - max;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        format!("{}\n\n{}", tr(en, "…（前面省略，到看看里看全文）", "… (earlier part omitted; open LookLook to read it all)"), &text[cut..])
    } else {
        text
    };
    card::body_markdown(&text)
}

fn thinking_of(items: &BTreeMap<usize, Value>) -> String {
    let all: Vec<&str> = items
        .values()
        .filter(|it| it["k"] == "assistant")
        .flat_map(|it| it["blocks"].as_array().into_iter().flatten())
        .filter(|b| b["t"] == "thinking")
        .filter_map(|b| b["text"].as_str().map(str::trim))
        .filter(|t| !t.is_empty())
        .collect();
    let text = all.join("\n\n");
    if text.len() <= THINK_BYTES {
        return text;
    }
    let mut cut = THINK_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

fn tool_detail(input: &Value) -> String {
    let s = |k: &str| input[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let base = |p: &str| Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    let raw = s("command")
        .map(|c| c.lines().next().unwrap_or_default().to_string())
        .or_else(|| s("file_path").or(s("notebook_path")).or(s("path")).map(base))
        .or_else(|| s("pattern").or(s("query")).or(s("url")).or(s("description")).or(s("skill")).or(s("title")).map(String::from))
        .unwrap_or_default();
    let mut t: String = raw.chars().take(60).collect();
    if raw.chars().count() > 60 {
        t.push('…');
    }
    t
}

fn steps_of(items: &BTreeMap<usize, Value>) -> Vec<card::Step> {
    let mut steps = Vec::new();
    let mut ids: HashMap<String, usize> = HashMap::new();
    for it in items.values() {
        if it["k"] == "assistant" {
            for b in it["blocks"].as_array().into_iter().flatten().filter(|b| b["t"] == "tool_use") {
                let name = b["name"].as_str().unwrap_or_default();
                let name = name.rsplit("__").next().unwrap_or(name);
                if let Some(id) = b["id"].as_str() {
                    ids.insert(id.into(), steps.len());
                }
                steps.push(card::Step { name: name.into(), detail: tool_detail(&b["input"]), done: false, failed: false });
            }
        } else if it["k"] == "tool_result" {
            if let Some(&n) = it["tool_use_id"].as_str().and_then(|id| ids.get(id)) {
                steps[n].done = true;
                steps[n].failed = it["is_error"] == true;
            }
        }
    }
    steps
}

fn status_line(b: &Value, en: bool) -> String {
    let short = |s: &str| -> String {
        let one = s.lines().next().unwrap_or_default();
        let mut t: String = one.chars().take(80).collect();
        if one.chars().count() > 80 || s.lines().nth(1).is_some() {
            t.push('…');
        }
        card::body_markdown(&t.replace('`', "'"))
    };
    match b["board"].as_str() {
        Some("attn") => tr(en, "在等你点头（审批卡片发在私聊里）", "Waiting for your approval (card in our private chat)"),
        Some("stopped") => tr(en, "正在启动…", "Starting…"),
        _ => match (b["doing"]["tool"].as_str(), b["doing"]["detail"].as_str()) {
            (Some(t), Some(d)) if !d.is_empty() => tr(en, &format!("正在用 {t}：`{}`", short(d)), &format!("Using {t}: `{}`", short(d))),
            (Some(t), _) => tr(en, &format!("正在用 {t}"), &format!("Using {t}")),
            _ => tr(en, "正在思考…", "Thinking…"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(kind: &str, content: Value) -> Value {
        json!({ "message_id": "om_1", "message_type": kind, "content": content.to_string() })
    }

    #[test]
    fn text_from_messages() {
        assert_eq!(message_text(&msg("text", json!({ "text": "  修一下登录页 " }))), "修一下登录页");
        let post = json!({ "title": "需求", "content": [[{ "tag": "text", "text": "看看 " }, { "tag": "a", "text": "这个", "href": "https://x" }], [{ "tag": "code_block", "language": "RUST", "text": "fn main() {}\n" }]] });
        assert_eq!(message_text(&msg("post", post.clone())), "需求\n看看 这个\n```rust\nfn main() {}\n```");
        assert_eq!(message_text(&msg("post", json!({ "zh_cn": post }))), "需求\n看看 这个\n```rust\nfn main() {}\n```");
        assert_eq!(message_text(&msg("image", json!({ "image_key": "k" }))), "");
        let c = json!({ "title": "做完了", "elements": [[{ "tag": "text", "text": "改了 2 个文件" }]] });
        assert_eq!(card_text(&c), "做完了\n改了 2 个文件");
    }

    #[test]
    fn files_in_messages() {
        let img = |k: &str| Resource { kind: "image".into(), key: k.into(), name: String::new(), msg: "om_1".into() };
        assert_eq!(attachments(&msg("image", json!({ "image_key": "img_1" }))), vec![img("img_1")]);
        let post = json!({ "content": [[{ "tag": "text", "text": "看图" }, { "tag": "img", "image_key": "img_2" }]] });
        assert_eq!(attachments(&msg("post", post)), vec![img("img_2")]);
    }

    #[test]
    fn mentions_and_commands() {
        // 事件里的 mentions：@机器人、@张三、@所有人、@自己
        let ev = json!({ "mentions": [
            { "key": "@_user_1", "id": { "open_id": "ou_bot" }, "name": "看看" },
            { "key": "@_user_2", "id": { "open_id": "ou_zs" }, "name": "张三" },
            { "key": "@_all", "id": { "open_id": "" }, "name": "所有人" },
            { "key": "@_user_3", "id": { "open_id": "ou_me" }, "name": "我" },
        ] });
        assert!(mentions_bot(&ev, "ou_bot"));
        assert_eq!(clean_mentions("@_user_1 帮 @_user_2 看下", &ev["mentions"], "ou_bot"), "帮 @张三 看下");
        assert_eq!(mentioned(&ev, "ou_bot", "ou_me"), vec![Who { id: "ou_zs".into(), name: "张三".into() }]);
        // 开放接口取回的消息：id 是字符串
        let got = json!([{ "key": "@_user_1", "id": "ou_ls", "id_type": "open_id", "name": "李四" }]);
        assert_eq!(clean_mentions("@_user_1 接口挂了？", &got, ""), "@李四 接口挂了？");
        assert!(is_stop("停止。") && is_stop("Stop") && !is_stop("停止那个服务"));
        assert!(is_help("帮助") && !is_help("帮助我看看"));
        assert!(says_no_at("回答一下，别@他") && says_no_at("不用 @ 张三") && !says_no_at("回答并 @ 他"));
    }

    #[test]
    fn merged_mentions_rules() {
        let w = |id: &str, n: &str| Who { id: id.into(), name: n.into() };
        let i = |m: Vec<Who>, no_at: bool| In { id: "om".into(), text: "x".into(), files: vec![], typing: None, mentions: m, group: true, no_at, at: Instant::now() };
        let ins = vec![i(vec![w("ou_a", "")], false), i(vec![w("ou_a", "阿"), w("ou_b", "B")], false)];
        assert_eq!(merged_mentions(&ins), vec![w("ou_a", "阿"), w("ou_b", "B")]);
        let ins = vec![i(vec![w("ou_a", "")], false), i(vec![], true)];
        assert!(merged_mentions(&ins).is_empty());
        let many: Vec<Who> = (0..9).map(|n| w(&format!("ou_{n}"), "")).collect();
        assert_eq!(merged_mentions(&[i(many, false)]).len(), MENTIONS_MAX);
    }

    #[test]
    fn source_and_post() {
        let at = vec![Who { id: "ou_zs".into(), name: "张三".into() }, Who { id: "ou_x".into(), name: String::new() }];
        let s = source_line(true, &at, Some(false), false);
        assert!(s.starts_with("[飞书 · 群聊") && s.contains("@ 张三、被引用的人") && s.contains("飞书 CLI 还没就绪"), "{s}");
        assert_eq!(source_line(false, &[], Some(true), false), "[飞书 · 私聊]");
        let p = post_content(&at, "**好了**");
        assert_eq!(p["zh_cn"]["content"][0][0], json!({ "tag": "at", "user_id": "ou_zs" }));
        assert_eq!(p["zh_cn"]["content"][1][0], json!({ "tag": "md", "text": "**好了**" }));
        assert_eq!(post_content(&[], "x")["zh_cn"]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn forwarded_history() {
        let items = vec![
            json!({ "message_id": "om_f", "msg_type": "merge_forward", "body": { "content": "Merged and Forwarded Message" } }),
            json!({ "message_id": "om_a", "upper_message_id": "om_f", "msg_type": "text", "sender": { "id": "ou_zs", "sender_type": "user" }, "body": { "content": "{\"text\":\"接口挂了\"}" } }),
            json!({ "message_id": "om_b", "upper_message_id": "om_f", "msg_type": "text", "sender": { "id": "ou_me", "sender_type": "user" }, "body": { "content": "{\"text\":\"我看看\"}" } }),
            json!({ "message_id": "om_s", "upper_message_id": "om_f", "msg_type": "sticker", "body": { "content": "{}" } }),
        ];
        let (t, _) = forwarded("om_f", &items, "ou_me", false);
        assert!(t.contains("**对方**：\n> 接口挂了") && t.contains("**我**：\n> 我看看") && t.contains("[表情]"), "{t}");
    }

    async fn said_text(chat: &std::sync::Arc<crate::chat::Chat>, brain: &str, needle: &str) -> String {
        for _ in 0..40 {
            let snap = chat.snapshot(brain).await.unwrap_or_default();
            if let Some(t) = snap["items"].as_array().into_iter().flatten().filter(|i| i["k"] == "user").filter_map(|i| i["text"].as_str()).find(|t| t.contains(needle)) {
                return t.to_string();
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("助理没收到：{needle}");
    }

    /// 端到端（假飞书）：真的 App、真的 Claude Code 当助理的大脑（haiku），飞书是本机的假服务器（`feishu::fake`）。
    /// 要登录好的 claude，会花一点 haiku 额度，默认不跑：
    /// `cargo build && TTYD_BIN=<ttyd> FEISHU_E2E_DIR=<空目录> cargo test feishu_bot_e2e -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn feishu_bot_e2e() {
        use crate::feishu::fake::{Fake, Req};
        use std::sync::Arc;
        let dir = std::env::var("FEISHU_E2E_DIR").expect("FEISHU_E2E_DIR");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("looklook");
        assert!(exe.is_file(), "先 cargo build");
        std::env::set_var("LOOKLOOK_HOOK_EXE", exe);
        let fake = Fake::start().await;
        crate::feishu::TEST_BASE.set(Box::leak(fake.base.clone().into_boxed_str())).unwrap();

        let home = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::discover(Some(home.path().to_path_buf())).unwrap();
        assert!(paths.ttyd.is_some(), "要 ttyd：设置 TTYD_BIN");
        let store = Arc::new(crate::store::Store::memory().unwrap());
        let shutdown = tokio_util::sync::CancellationToken::new();
        let account = crate::account::test_support::logged_in(store.clone(), paths.clone(), "http://127.0.0.1:9", "dev1", ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]));
        let instances = crate::instances::Instances::new(store.clone(), paths.clone());
        let crate::instances::Backend::Tmux(tmux) = instances.backend.clone() else { panic!("需要 tmux") };
        let chat = crate::chat::Chat::new(store.clone(), paths.clone(), instances.clone());
        let ui = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let ui_addr = ui.local_addr().unwrap();
        let app: App = Arc::new(crate::gateway::Inner {
            account: account.clone(),
            instances: instances.clone(),
            updater: crate::updater::Updater::new(store.clone(), paths.clone(), account.clone(), shutdown.clone()),
            metrics: crate::metrics::Metrics::new(),
            relay_watch: crate::relay_watch::RelayWatch::new(account.clone()),
            transfers: crate::transfer::Transfers::new(paths.clone()),
            chat: chat.clone(),
            store: store.clone(),
            paths: paths.clone(),
            ui_addr,
            relay_addr: ui_addr,
            http: crate::gateway::proxy_client(),
            access: std::sync::RwLock::new(crate::gateway::access::AccessConfig::load(&store)),
            limiter: Default::default(),
            resolved: Default::default(),
            lan_ips: vec![],
            remote: Default::default(),
            rtc: Default::default(),
            outbound: Default::default(),
            ssh_transfer: Default::default(),
            sync: Default::default(),
            notify: Default::default(),
        });
        tokio::spawn(crate::gateway::serve(ui, crate::gateway::local_app(app.clone()), shutdown.clone()));
        chat.publish(ui_addr).unwrap();
        tokio::spawn(chat.clone().background());
        tokio::spawn(app.notify.clone().run(app.clone(), shutdown.clone()));
        let kill = || {
            let mut args = instances.tmux_socket_args();
            args.push("kill-server".into());
            let _ = std::process::Command::new(&tmux).args(&args).status();
        };
        let mut st = crate::settings::Settings::load(&store);
        st.default_workdir = dir.clone();
        st.save(&store, account.clock.now_ms()).unwrap();
        notify::save_feishu(&app, Some(&Feishu { app_id: "cli_test".into(), app_secret: "secret".into(), domain: "feishu".into(), open_id: Some("ou_me".into()), bound_at: Some(1), callback_ok: true, ..Default::default() })).unwrap();
        for _ in 0..150 {
            if fake.connected.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert!(fake.connected.load(std::sync::atomic::Ordering::SeqCst), "长连接没连上");
        let msg_ev = |id: &str, chat_type: &str, sender: &str, text: &str, parent: Option<&str>, mentions: Value| {
            json!({
                "schema": "2.0",
                "header": { "event_id": format!("ev-{id}"), "event_type": "im.message.receive_v1" },
                "event": {
                    "sender": { "sender_id": { "open_id": sender } },
                    "message": { "message_id": id, "chat_id": if chat_type == "group" { "oc_group" } else { "oc_dm" }, "chat_type": chat_type, "message_type": "text", "content": json!({ "text": text }).to_string(), "root_id": parent, "parent_id": parent, "mentions": mentions },
                },
            })
        };
        let at_bot = json!([{ "key": "@_user_1", "id": { "open_id": "ou_bot" }, "name": "看看" }]);
        let is_card_update = |r: &Req| r.method == "PUT" && r.path.starts_with("/open-apis/cardkit/v1/cards/") && !r.path.contains("/elements/");
        // 0. 助理没打开：回一句怎么打开
        let before = fake.reqs().len();
        fake.push("event", msg_ev("om_z0", "p2p", "ou_me", "在吗", None, json!([]))).await;
        fake.wait_after(before, "助理没打开的提示", 30, |r| r.path.contains("/om_z0/reply") && r.text().contains("助理还没打开")).await;
        println!("0. 没打开 ok");

        let brain = crate::gateway::assistant::ensure_brain(&app, crate::assistant::Engine::Claude).await.expect("建大脑");
        chat.setting(&brain, "model", "haiku").await.ok();

        // 1. 私聊：交给助理，流式卡片回在这句下面；第一次说话附欢迎卡片
        let before = fake.reqs().len();
        let n = store.instances().unwrap().len();
        fake.push("event", msg_ev("om_a1", "p2p", "ou_me", "只回复一个字：助。不要用任何工具。", None, json!([]))).await;
        fake.wait_after(before, "流式卡片回在这句下", 60, |r| r.path.contains("/om_a1/reply") && r.text().contains("card_id")).await;
        fake.wait_after(before, "欢迎卡片", 30, |r| r.path.contains("/om_a1/reply") && r.text().contains("你好，我是看看助理")).await;
        fake.wait_after(before, "这一轮做完", 240, |r| is_card_update(r) && r.text().contains("已完成")).await;
        assert_eq!(store.instances().unwrap().len(), n, "没开新会话");
        let t = said_text(&chat, &brain, "只回复一个字：助").await;
        assert!(t.starts_with("[飞书 · 私聊"), "{t}");
        tokio::time::sleep(END_SETTLE * 2).await;
        println!("1. 私聊 ok");

        // 2. 群里：别人 @ 机器人不理；用户引用张三的话再 @ 机器人 → 助理收到引用，回复是 post，@ 张三
        let before = fake.reqs().len();
        fake.push("event", msg_ev("om_x1", "group", "ou_other", "@_user_1 你好", None, at_bot.clone())).await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(fake.reqs().iter().skip(before).all(|r| !r.path.contains("om_x1")), "别人 @ 机器人不该理");
        fake.push("event", msg_ev("om_g1", "group", "ou_me", "@_user_1 只回复三个字：在这里。不要用任何工具。", Some("om_q"), at_bot.clone())).await;
        let post = fake.wait_after(before, "群里的回复（post）", 240, |r| r.path.contains("/om_g1/reply") && r.json()["msg_type"] == "post").await;
        let c = post.content();
        assert_eq!(c["zh_cn"]["content"][0][0], json!({ "tag": "at", "user_id": "ou_zhang" }), "{c}");
        assert!(fake.reqs().iter().skip(before).all(|r| !(r.path.contains("/om_g1/reply") && r.text().contains("card_id"))), "群里不发流式卡片");
        let t = said_text(&chat, &brain, "只回复三个字").await;
        assert!(t.starts_with("[飞书 · 群聊") && t.contains("回答会自动 @ 被引用的人") && t.contains("发布脚本在哪个目录"), "{t}");
        tokio::time::sleep(END_SETTLE * 2).await;
        println!("2. 群里引用 + @ ok");

        // 3. 派出去的活有了结果（直接送一条事件进大脑）：转告回到群里那条，还 @ 张三
        let before = fake.reqs().len();
        let ev = "<looklook-event type=\"turn_end\" session=\"zzzzzzzz\" title=\"测试\" project=\"/tmp\" outcome=\"done\">\n它最后说的：做完了。\n</looklook-event>";
        crate::gateway::assistant::tell_brain_push(&app, ev.to_string(), true).await;
        let post = fake.wait_after(before, "转告回到群里", 240, |r| r.path.contains("/om_g1/reply") && r.json()["msg_type"] == "post").await;
        assert_eq!(post.content()["zh_cn"]["content"][0][0]["user_id"], "ou_zhang");
        tokio::time::sleep(END_SETTLE * 2).await;
        println!("3. 转告 ok");

        // 4. 引用机器人自己的回复追问：沿用那次 @ 的人；说了“别 @”就不 @
        let mine = replies(&app).last().map(|r| r.m.clone()).expect("记下了机器人的回复");
        let before = fake.reqs().len();
        fake.push("event", msg_ev("om_g2", "group", "ou_me", "@_user_1 只回复一个字：好。不要用任何工具。", Some(&mine), at_bot.clone())).await;
        let post = fake.wait_after(before, "追问的回复", 240, |r| r.path.contains("/om_g2/reply") && r.json()["msg_type"] == "post").await;
        assert_eq!(post.content()["zh_cn"]["content"][0][0]["user_id"], "ou_zhang");
        tokio::time::sleep(END_SETTLE * 2).await;
        let before = fake.reqs().len();
        fake.push("event", msg_ev("om_g3", "group", "ou_me", "@_user_1 只回复一个字：嗯，别@他。不要用任何工具。", Some("om_q"), at_bot.clone())).await;
        let post = fake.wait_after(before, "别 @ 的回复", 240, |r| r.path.contains("/om_g3/reply") && r.json()["msg_type"] == "post").await;
        assert_eq!(post.content()["zh_cn"]["content"][0][0]["tag"], "md", "说了别 @ 就不 @");
        tokio::time::sleep(END_SETTLE * 2).await;
        println!("4. 追问 / 别 @ ok");

        // 5. 帮助卡片；关掉机器人后只回一句怎么打开
        let before = fake.reqs().len();
        fake.push("event", msg_ev("om_h", "p2p", "ou_me", "帮助", None, json!([]))).await;
        fake.wait_after(before, "帮助卡片", 30, |r| r.path.contains("/om_h/reply") && r.text().contains("飞书 CLI")).await;
        let mut s = crate::assistant::settings(&store);
        s.feishu = false;
        crate::assistant::save_settings(&store, s).unwrap();
        fake.push("event", msg_ev("om_off", "p2p", "ou_me", "在吗", None, json!([]))).await;
        fake.wait_after(before, "关着的提示", 30, |r| r.path.contains("/om_off/reply") && r.text().contains("没有打开")).await;
        println!("5. 帮助 / 开关 ok");
        shutdown.cancel();
        kill();
    }
}
