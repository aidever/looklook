//! 助理碰会话的那一半（docs/ASSISTANT.md）：大脑用的工具、盯结果的看守、管理台用的接口。规矩和闸门在 `crate::assistant`。
//!
//! - **助理工具**（`POST /mcp/assistant`，MCP）：只认大脑的令牌、只收本机。`projects_list`、`sessions_list`、
//!   `session_start`、`session_send`、`session_get`、`session_stop`、`agreement_add`、`agreement_done`。
//!   没有“批准审批”、没有“取全文”：审批只有用户能点；大脑的上下文留给跟用户说话，不背干活的记录；
//! - **开会话**就是会话页的“新建”（[`super::create_session`]），送话就是 `Chat::input`：干活的是普通会话，会话列表里看得见、
//!   用户随时能自己接着说；
//! - **同一个项目默认排队**：那边有会话在干活时，新开的先记下（`assistant.queue`），等它这一轮做完再开，并告诉大脑；
//! - **看守**（[`run`]）：每两秒看一遍助理送过话的会话。一轮结束、它在提问 → 整理成一条 `<looklook-event …>` 消息送进大脑
//!   （最后一段回复截到 1500 字）。审批不告诉大脑，卡片直接给用户；
//! - **管理台**（`/api/assistant…`）：状态（大脑是哪个会话、在干的活、最近的分派、约定）、打开、改去向。

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::access::Peer;
use super::{Access, App};
use crate::assistant::{self as core, Dispatch, Fix, EVENT_OPEN, MCP_SERVER};
use crate::error::LocalError;
use crate::instances::Input;

type R<T> = Result<T, LocalError>;

pub const MCP_PATH: &str = "/mcp/assistant";
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// 告诉大脑的那段回复最多多少个字符
const REPLY_MAX: usize = 1500;
/// 一轮结束后等多久再去读最后一段（等记录写完）
const SETTLE: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_secs(2);
/// 第一句话太长时不当启动参数，开好以后再发（会话的第一句话有长度上限）
const FIRST_MAX: usize = 3500;
const ENGINES: &[&str] = &["claude", "codex"];
const QUEUE_KEY: &str = "assistant.queue";

/// 排着队的新会话：`after` 那个会话这一轮做完再开
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Queued {
    dispatch: String,
    after: String,
    project: String,
    title: String,
    engine: String,
    /// 整理好的第一句话
    first: String,
}

fn queue(app: &App) -> Vec<Queued> {
    app.store.get(QUEUE_KEY).ok().flatten().unwrap_or_default()
}

// ---------------- 大脑 ----------------

/// 大脑那个会话：没有就建一个（不启动终端，对话页用到时才起进程）
pub async fn ensure_brain(app: &App) -> R<String> {
    if let Some(id) = core::brain_id(&app.store) {
        if app.store.instance(&id)?.is_some() {
            return Ok(id);
        }
    }
    core::prepare(&app.paths, &app.store, app.ui_addr).map_err(|e| LocalError::new("INTERNAL").with("detail", e.to_string()))?;
    let input = Input {
        name: Some("助理".into()),
        name_custom: Some(true),
        workdir: Some(core::dir(&app.paths).display().to_string()),
        launch: Some("claude".into()),
        chat_ui: Some(true),
        // 大脑不带全部权限：它用工具都要过闸门
        full_access: Some(false),
        ..Default::default()
    };
    let v = super::create_session(app, input, false).await?;
    if !app.instances.claude_sdk_mode(&v.row) {
        // 这台电脑上对话模式起不来（没有 tmux 后端等）：助理用不了
        let _ = app.instances.delete(&v.row.id).await;
        return Err(LocalError::new("ASSISTANT_UNAVAILABLE"));
    }
    app.store.set(core::BRAIN_KEY, &v.row.id)?;
    tracing::info!(id = %v.row.id, "助理：建好了大脑会话");
    Ok(v.row.id)
}

// ---------------- 项目和会话 ----------------

fn base_name(dir: &str) -> String {
    FsPath::new(dir).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.to_string())
}

/// 不算进项目的目录：助理自己的、附件目录
fn own_dir(app: &App, dir: &str) -> bool {
    FsPath::new(dir).starts_with(app.paths.home.join("assistant"))
}

/// 已知的项目：现有会话的目录 + 以前聊过的目录（新到旧）
async fn projects(app: &App) -> Vec<Value> {
    let brain = core::brain_id(&app.store);
    let rows: Vec<_> = app.store.instances().unwrap_or_default().into_iter().filter(|r| Some(&r.id) != brain.as_ref()).collect();
    let mut out: Vec<Value> = vec![];
    let mut seen: Vec<String> = vec![];
    for f in crate::history::folders(1, |_| true).await {
        if own_dir(app, &f.workdir) || !FsPath::new(&f.workdir).is_dir() {
            continue;
        }
        let key = crate::history::dir_key(&f.workdir);
        let open = rows.iter().filter(|r| crate::history::dir_key(&r.workdir) == key).count();
        out.push(json!({ "name": base_name(&f.workdir), "path": f.workdir, "last_used": f.updated_at, "past_conversations": f.total, "open_sessions": open, "last_topic": f.items.first().map(|c| &c.title) }));
        seen.push(key);
    }
    for r in &rows {
        let key = crate::history::dir_key(&r.workdir);
        if key.is_empty() || seen.contains(&key) || own_dir(app, &r.workdir) {
            continue;
        }
        let open = rows.iter().filter(|x| crate::history::dir_key(&x.workdir) == key).count();
        out.push(json!({ "name": base_name(&r.workdir), "path": r.workdir, "open_sessions": open }));
        seen.push(key);
    }
    out.truncate(60);
    out
}

/// 项目可以写路径，也可以写名字（目录名，对得上唯一一个才算）
async fn resolve_project(app: &App, project: &str) -> Result<String, String> {
    let project = project.trim();
    if project.is_empty() {
        return Err("project is required: a path, or a name from projects_list".into());
    }
    if project == "scratch" {
        return Ok(core::scratch(&app.paths).display().to_string());
    }
    let p = FsPath::new(project);
    if p.is_absolute() {
        return if p.is_dir() { Ok(project.trim_end_matches('/').to_string()) } else { Err(format!("no such directory: {project}")) };
    }
    let want = project.to_lowercase();
    let hits: Vec<String> = projects(app).await.iter().filter(|x| x["name"].as_str().is_some_and(|n| n.to_lowercase() == want)).filter_map(|x| x["path"].as_str().map(String::from)).collect();
    match hits.len() {
        1 => Ok(hits.into_iter().next().unwrap()),
        0 => Err(format!("no project named {project}; call projects_list, or pass an absolute path")),
        _ => Err(format!("more than one project is named {project}: {}. Pass the path, or ask the user which one.", hits.join(", "))),
    }
}

/// 按 id 取会话标题（没有这个会话时用 id）
fn title_by_id(app: &App, id: &str) -> String {
    match app.store.instance(id).ok().flatten() {
        Some(row) => title_of(app, &row, &app.chat.brief(id).unwrap_or_default()),
        None => id.to_string(),
    }
}

/// 会话的标题：起了名字的（助理开的会话用它起的标题）用名字，别的用对话的标题
fn title_of(app: &App, row: &crate::store::InstanceRow, b: &Value) -> String {
    if app.instances.opts(&row.id).name_custom && !row.name.is_empty() {
        return row.name.clone();
    }
    b["title"].as_str().unwrap_or(&row.name).to_string()
}

fn session_view(app: &App, row: &crate::store::InstanceRow, watched: bool) -> Value {
    let b = app.chat.brief(&row.id).unwrap_or_default();
    let state = match b["board"].as_str() {
        Some("working") => "working",
        Some("attn") => "waiting_for_user",
        Some("ended") => "turn_finished",
        Some("stopped") => "stopped",
        Some(_) => "idle",
        None => "not_started",
    };
    json!({
        "session": row.id,
        "title": title_of(app, row, &b),
        "project": row.workdir,
        "engine": row.launch,
        "state": state,
        "doing": b["doing"]["detail"].as_str().or_else(|| b["doing"]["label"].as_str()),
        "last_summary": b["turn"]["summary"],
        "context_tokens": app.chat.context(&row.id),
        "last_active": b["active_at"].as_i64().filter(|t| *t > 0).map(crate::util::fmt_ms),
        "started_by_assistant": watched,
    })
}

async fn sessions(app: &App, project: Option<&str>) -> Result<Vec<Value>, String> {
    let brain = core::brain_id(&app.store);
    let dir = match project.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Some(resolve_project(app, p).await?),
        None => None,
    };
    let watches = core::watches(&app.store);
    Ok(app
        .store
        .instances()
        .unwrap_or_default()
        .iter()
        .filter(|r| Some(&r.id) != brain.as_ref() && matches!(r.launch.as_str(), "claude" | "codex" | "dsh" | "opencode"))
        .filter(|r| dir.as_ref().is_none_or(|d| crate::history::same_dir(&r.workdir, d) || core::project_root(FsPath::new(&r.workdir)) == core::project_root(FsPath::new(d))))
        .map(|r| session_view(app, r, watches.contains_key(&r.id)))
        .collect())
}

/// 这一轮里它最后说的话（从最近一条用户消息之后算起）
async fn last_reply(app: &App, id: &str) -> String {
    let Ok(snap) = app.chat.snapshot(id).await else { return String::new() };
    let items = snap["items"].as_array().cloned().unwrap_or_default();
    let mut texts: Vec<String> = vec![];
    for it in items.iter().rev() {
        match it["k"].as_str() {
            Some("user") if !it["mid"].as_bool().unwrap_or(false) => break,
            Some("assistant") => {
                let t: Vec<&str> = it["blocks"].as_array().into_iter().flatten().filter(|b| b["t"] == "text").filter_map(|b| b["text"].as_str()).map(str::trim).filter(|t| !t.is_empty()).collect();
                if !t.is_empty() {
                    texts.push(t.join("\n\n"));
                }
            }
            _ => {}
        }
    }
    texts.reverse();
    texts.join("\n\n")
}

// ---------------- 分派 ----------------

fn new_dispatch(kind: &str, session: &str, project: &str, title: &str, engine: &str, reason: &str, said: &str, brief: &str) -> Dispatch {
    Dispatch {
        id: format!("d{}", crate::util::random_id(8)),
        at: crate::util::system_ms(),
        kind: kind.into(),
        session: session.into(),
        project: project.into(),
        title: title.chars().take(80).collect(),
        engine: engine.into(),
        reason: reason.chars().take(300).collect(),
        said: said.into(),
        brief: brief.chars().take(4000).collect(),
        queued_after: None,
        fixed: None,
    }
}

fn watch(app: &App, session: &str, dispatch: &str) {
    let now = crate::util::system_ms();
    core::watch_update(&app.store, |m| {
        let w = m.entry(session.to_string()).or_default();
        // 之前结束的那些轮不算
        w.told_end = w.told_end.max(now);
        w.dispatch = dispatch.to_string();
    });
}

/// 真的开一个会话，把第一句话交给它
async fn open_session(app: &App, project: &str, title: &str, engine: &str, first: &str) -> R<String> {
    let short = first.chars().count() <= FIRST_MAX;
    let input = Input {
        name: Some(title.chars().take(40).collect()).filter(|t: &String| !t.trim().is_empty()),
        // 用助理起的标题（第一句话是交代，拿它当标题没法看）
        name_custom: Some(true),
        workdir: Some(project.to_string()),
        launch: Some(engine.to_string()),
        chat_ui: Some(true),
        full_access: Some(crate::settings::Settings::load(&app.store).default_full_access),
        prompt: short.then(|| first.to_string()),
        ..Default::default()
    };
    let v = super::create_session(app, input, true).await?;
    if !short {
        app.chat.input(&v.row.id, first, &[]).await?;
    }
    Ok(v.row.id)
}

async fn session_start(app: &App, a: &Value) -> Result<Value, String> {
    let s = |k: &str| a[k].as_str().unwrap_or("").trim().to_string();
    let (title, brief, reason) = (s("title"), s("brief"), s("reason"));
    if title.is_empty() {
        return Err("title is required: a few words for the session list".into());
    }
    if reason.is_empty() {
        return Err("reason is required: one sentence the user will see about why this goes to a new session here".into());
    }
    let project = resolve_project(app, &s("project")).await?;
    let engine = match s("engine").as_str() {
        "" => "claude".to_string(),
        e if ENGINES.contains(&e) => e.to_string(),
        e => return Err(format!("unknown engine {e}; use one of: {}", ENGINES.join(", "))),
    };
    let said = core::said(&app.store).map(|x| x.text).unwrap_or_default();
    let brain = core::brain_id(&app.store).unwrap_or_default();
    let busy = app.chat.busy_session(FsPath::new(&project), &brain);
    let now = a["now"].as_bool().unwrap_or(false);
    let mut d = new_dispatch("start", "", &project, &title, &engine, &reason, &said, &brief);
    if let (Some(after), false) = (&busy, now) {
        // 默认排队：同一个项目里并行，没进 git 的文件、依赖、端口都会撞
        let first = core::compose_brief(Some(&said), &brief, false);
        d.queued_after = Some(after.clone());
        let mut q = queue(app);
        q.push(Queued { dispatch: d.id.clone(), after: after.clone(), project: project.clone(), title: title.clone(), engine: engine.clone(), first });
        app.store.set(QUEUE_KEY, &q).map_err(|e| e.to_string())?;
        core::dispatch_add(&app.paths, d.clone()).map_err(|e| e.to_string())?;
        let after_title = title_by_id(app, after);
        return Ok(json!({
            "queued": true,
            "dispatch": d.id,
            "after_session": after,
            "note": format!("Session \"{after_title}\" is working in this project, so this one is queued and will start by itself when that turn ends; you will get an event then. Tell the user it is queued behind \"{after_title}\". Only if the user asks to start right now, call session_start again with now=true."),
        }));
    }
    let first = core::compose_brief(Some(&said), &brief, busy.is_some());
    let id = open_session(app, &project, &title, &engine, &first).await.map_err(|e| format!("could not start the session: {} {}", e.code, serde_json::to_string(&e.params).unwrap_or_default()))?;
    d.session = id.clone();
    core::dispatch_add(&app.paths, d.clone()).map_err(|e| e.to_string())?;
    watch(app, &id, &d.id);
    Ok(json!({ "session": id, "dispatch": d.id, "project": project, "engine": engine, "note": "Started. You will get an event when it has a result or a question; do not poll." }))
}

fn session_row(app: &App, id: &str) -> Result<crate::store::InstanceRow, String> {
    let brain = core::brain_id(&app.store);
    match app.store.instance(id.trim()).ok().flatten() {
        Some(r) if Some(&r.id) != brain.as_ref() => Ok(r),
        _ => Err(format!("no such session: {id}; call sessions_list")),
    }
}

async fn session_send(app: &App, a: &Value) -> Result<Value, String> {
    let s = |k: &str| a[k].as_str().unwrap_or("").trim().to_string();
    let (text, reason) = (s("text"), s("reason"));
    if text.is_empty() {
        return Err("text is required".into());
    }
    if reason.is_empty() {
        return Err("reason is required: one sentence the user will see about why this goes to that session".into());
    }
    let row = session_row(app, &s("session"))?;
    let said = core::said(&app.store).map(|x| x.text).unwrap_or_default();
    let d = new_dispatch("send", &row.id, &row.workdir, &title_by_id(app, &row.id), &row.launch, &reason, &said, &text);
    // 先记去向再送：送出去以后那边马上会有动静
    watch(app, &row.id, &d.id);
    app.chat.input(&row.id, &core::compose_followup(Some(&said), &text), &[]).await.map_err(|e| format!("could not send: {}", e.code))?;
    core::dispatch_add(&app.paths, d.clone()).map_err(|e| e.to_string())?;
    Ok(json!({ "session": row.id, "dispatch": d.id, "note": "Sent. If it is in the middle of a turn, it reads this after the current step. You will get an event when it has a result." }))
}

async fn session_get(app: &App, a: &Value) -> Result<Value, String> {
    let row = session_row(app, a["session"].as_str().unwrap_or(""))?;
    let mut v = session_view(app, &row, core::watches(&app.store).contains_key(&row.id));
    let b = app.chat.brief(&row.id).unwrap_or_default();
    v["waiting_for"] = b["attn"].get("kind").cloned().unwrap_or(Value::Null);
    v["question"] = b["attn"]["question"].clone();
    v["last_reply"] = json!(core::tail_chars(&last_reply(app, &row.id).await, REPLY_MAX));
    Ok(v)
}

async fn call_tool(app: &App, name: &str, a: &Value) -> Option<Result<Value, String>> {
    Some(match name {
        "projects_list" => Ok(json!({ "projects": projects(app).await, "scratch": core::scratch(&app.paths), "note": "For work that belongs to no project, use project=\"scratch\"." })),
        "sessions_list" => sessions(app, a["project"].as_str()).await.map(|s| json!({ "sessions": s })),
        "session_start" => session_start(app, a).await,
        "session_send" => session_send(app, a).await,
        "session_get" => session_get(app, a).await,
        "session_stop" => match session_row(app, a["session"].as_str().unwrap_or("")) {
            Ok(row) => app.chat.interrupt(&row.id).await.map(|_| json!({ "stopped": row.id })).map_err(|e| format!("could not stop: {}", e.code)),
            Err(e) => Err(e),
        },
        "agreement_add" => core::agreement_add(&app.paths, a["text"].as_str().unwrap_or(""), a["until"].as_str()).map(|x| json!({ "saved": x })).map_err(String::from),
        "agreement_done" => core::agreement_done(&app.paths, a["id"].as_str().unwrap_or("")).map(|x| json!({ "removed": x })).ok_or_else(|| "no such agreement".to_string()),
        _ => return None,
    })
}

fn tool_defs() -> Value {
    let obj = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false });
    json!([
        { "name": "projects_list", "description": "The projects on this computer that Looklook knows about (folders with open sessions or past AI conversations), newest first.", "inputSchema": obj(json!({}), &[]) },
        { "name": "sessions_list", "description": "The AI coding sessions (Claude Code / Codex) currently open in Looklook, including the ones the user opened by hand. Call this before deciding between session_start and session_send. state: working | waiting_for_user | turn_finished | idle | stopped | not_started.",
          "inputSchema": obj(json!({ "project": { "type": "string", "description": "Only sessions in this project (name or path). Omit for all." } }), &[]) },
        { "name": "session_start", "description": "Start a NEW coding session for a piece of work. The user's own words are attached verbatim by Looklook; `brief` is only your added background. If another session is working in the same project this is queued behind it by default.",
          "inputSchema": obj(json!({
              "project": { "type": "string", "description": "Project name from projects_list, an absolute path, or \"scratch\" for work that belongs to no project." },
              "title": { "type": "string", "description": "A few words for the session list, in the user's language." },
              "brief": { "type": "string", "description": "Background the session needs, as up to four parts: what is already decided / what is still open / hard constraints (including standing agreements) / when to stop and ask. No implementation plan. May be empty when the user's words say it all." },
              "reason": { "type": "string", "description": "One sentence, in the user's language, shown to the user: why a new session, why here." },
              "engine": { "type": "string", "enum": ENGINES, "description": "Only when the user named one, or the project is known to use one. Default: claude." },
              "now": { "type": "boolean", "description": "Start right away even though another session is working in the same project. Only when the user asked for that." },
          }), &["project", "title", "reason"]) },
        { "name": "session_send", "description": "Send a follow-up into an EXISTING session: more requirements, a correction, an answer to its question. Use for the same piece of work, or the same area of code while its context is not full.",
          "inputSchema": obj(json!({
              "session": { "type": "string", "description": "Session id from sessions_list." },
              "text": { "type": "string", "description": "What to tell it. The user's own words are attached by Looklook." },
              "reason": { "type": "string", "description": "One sentence, in the user's language, shown to the user: why this goes to that session." },
          }), &["session", "text", "reason"]) },
        { "name": "session_get", "description": "The current state of one session and the end of its last reply (at most 1500 characters). Use this before answering \"how is it going\". There is no way to fetch a whole transcript; give the user the session instead.",
          "inputSchema": obj(json!({ "session": { "type": "string" } }), &["session"]) },
        { "name": "session_stop", "description": "Interrupt a session's current turn. Only when the user just asked for it.", "inputSchema": obj(json!({ "session": { "type": "string" } }), &["session"]) },
        { "name": "agreement_add", "description": "Record a standing instruction the user gave in conversation that has a condition or an end (\"don't touch payments until I confirm\", \"no pushes this week\"). It is shown to you on every turn from now on, so it survives long conversations.",
          "inputSchema": obj(json!({ "text": { "type": "string", "description": "The instruction, in the user's words, under 300 characters." }, "until": { "type": "string", "description": "When it ends, if the user said (their words)." } }), &["text"]) },
        { "name": "agreement_done", "description": "Remove a standing agreement that has expired or that the user cancelled.", "inputSchema": obj(json!({ "id": { "type": "string" } }), &["id"]) },
    ])
}

// ---------------- MCP ----------------

pub async fn mcp(State(app): State<App>, access: Option<Extension<Access>>, peer: Option<Extension<Peer>>, headers: HeaderMap, body: Bytes) -> Response {
    let local = access.is_none_or(|a| a.0.is_local()) && peer.is_none_or(|p| p.0 == Peer::Local);
    // 浏览器发不出这个请求：带 Origin 的一律拒绝
    if !local || headers.contains_key(header::ORIGIN) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim).unwrap_or("");
    if bearer.is_empty() || !super::ct_eq(bearer, &core::token(&app.store)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })).into_response();
    };
    let list = match msg {
        Value::Array(l) => l,
        m => vec![m],
    };
    let mut out = vec![];
    for m in list {
        if let Some(r) = rpc(&app, m).await {
            out.push(r);
        }
    }
    match out.len() {
        0 => StatusCode::ACCEPTED.into_response(),
        1 => Json(out.remove(0)).into_response(),
        _ => Json(Value::Array(out)).into_response(),
    }
}

pub async fn mcp_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

async fn rpc(app: &App, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str)?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let err = |code: i64, m: &str| Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": m } }));
    let result = match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or("");
            json!({
                "protocolVersion": PROTOCOL_VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOL_VERSIONS[0]),
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": MCP_SERVER, "title": "Looklook assistant tools", "version": crate::account::VERSION },
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_defs() }),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call_tool(app, name, &args).await {
                None => return err(-32602, &format!("unknown tool: {name}")),
                Some(Ok(v)) => json!({ "content": [{ "type": "text", "text": v.to_string() }], "isError": false }),
                Some(Err(e)) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            }
        }
        _ => return err(-32601, "method not found"),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

// ---------------- 看守 ----------------

fn attr(s: &str) -> String {
    s.replace(['"', '<', '>', '\n'], " ")
}

/// 送进大脑的消息（界面上画成卡片）
fn event(kind: &str, session: &str, title: &str, project: &str, extra: &str, body: &str) -> String {
    format!("{EVENT_OPEN} type=\"{kind}\" session=\"{}\" title=\"{}\" project=\"{}\"{extra}>\n{}\n</looklook-event>", attr(session), attr(title), attr(project), body.trim())
}

async fn tell_brain(app: &App, text: String) {
    let Some(brain) = core::brain_id(&app.store) else { return };
    if let Err(e) = app.chat.input(&brain, &text, &[]).await {
        tracing::warn!(error = ?e, "助理：结果没能送进大脑");
    }
}

/// 后台：盯着助理送过话的会话，开排着队的会话
pub async fn run(app: App) {
    if let Err(e) = core::prepare(&app.paths, &app.store, app.ui_addr) {
        tracing::warn!(error = %e, "助理：没能写好配置");
    }
    // 一轮结束后先等一下再读（会话 → 第一次看到结束的时间）
    let mut settling: HashMap<String, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(TICK).await;
        if core::brain_id(&app.store).is_none() {
            continue;
        }
        start_queued(&app).await;
        for (sid, w) in core::watches(&app.store) {
            let Some(row) = app.store.instance(&sid).ok().flatten() else {
                core::watch_update(&app.store, |m| {
                    m.remove(&sid);
                });
                continue;
            };
            let Some(b) = app.chat.brief(&sid) else { continue };
            let title = title_of(&app, &row, &b);
            // 它在提问：告诉大脑（它可能答得上，答不上就转给用户）。审批不告诉：卡片直接给用户
            let pending = b["attn"]["pending_id"].as_u64().filter(|_| b["attn"]["kind"] == "question");
            if pending.is_some() && pending != w.told_pending {
                core::watch_update(&app.store, |m| {
                    if let Some(x) = m.get_mut(&sid) {
                        x.told_pending = pending;
                    }
                });
                let q = b["attn"]["question"].as_str().unwrap_or("").to_string();
                let opts: Vec<&str> = b["attn"]["options"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let body = format!("它在问：{q}\n选项：{}\n用户能在看看里直接回答这张卡片。只有答案就在用户交代这件活时说过的话里，你才用 session_send 替他答，并告诉他。", if opts.is_empty() { "（自己填）".to_string() } else { opts.join(" / ") });
                tell_brain(&app, event("question", &sid, &title, &row.workdir, "", &body)).await;
            }
            let ended = b["turn"]["ended_at"].as_i64().unwrap_or(0);
            let running = matches!(b["phase"].as_str(), Some("running" | "approval"));
            if ended <= w.told_end || running {
                settling.remove(&sid);
                continue;
            }
            if settling.entry(sid.clone()).or_insert_with(Instant::now).elapsed() < SETTLE {
                continue;
            }
            settling.remove(&sid);
            core::watch_update(&app.store, |m| {
                if let Some(x) = m.get_mut(&sid) {
                    x.told_end = ended;
                }
            });
            let outcome = b["turn"]["outcome"].as_str().unwrap_or("done");
            let reply = last_reply(&app, &sid).await;
            let body = match (outcome, reply.is_empty()) {
                ("interrupted", _) => "这一轮被打断了（用户按了停止，或者会话结束了）。".to_string(),
                ("failed", true) => format!("这一轮出错了：{}", b["turn"]["error"].as_str().unwrap_or("没有更多信息")),
                (_, true) => "这一轮结束了，它没有留下文字回复。".to_string(),
                _ => format!("它最后说的（最多 {REPLY_MAX} 字，更早的看会话）：\n{}", core::tail_chars(&reply, REPLY_MAX)),
            };
            tell_brain(&app, event("turn_end", &sid, &title, &row.workdir, &format!(" outcome=\"{outcome}\""), &body)).await;
        }
    }
}

/// 排着队的：前面那个会话不忙了就开
async fn start_queued(app: &App) {
    let q = queue(app);
    if q.is_empty() {
        return;
    }
    let brain = core::brain_id(&app.store).unwrap_or_default();
    let mut left = vec![];
    for item in q {
        if app.chat.busy_session(FsPath::new(&item.project), &brain).is_some() {
            left.push(item);
            continue;
        }
        match open_session(app, &item.project, &item.title, &item.engine, &item.first).await {
            Ok(id) => {
                core::dispatch_update(&app.paths, &item.dispatch, |d| d.session = id.clone());
                watch(app, &id, &item.dispatch);
                tell_brain(app, event("started", &id, &item.title, &item.project, "", "排着队的这件活现在开了。不用告诉用户，除非他问。")).await;
            }
            Err(e) => {
                tracing::warn!(error = ?e, "助理：排队的会话没开起来");
                tell_brain(app, event("start_failed", "", &item.title, &item.project, "", &format!("排着队的这件活没能开起来：{}。告诉用户。", e.code))).await;
            }
        }
    }
    let _ = app.store.set(QUEUE_KEY, &left);
}

// ---------------- 管理台 ----------------

pub async fn state(State(app): State<App>) -> R<Json<Value>> {
    let brain = core::brain_id(&app.store).filter(|id| app.store.instance(id).ok().flatten().is_some());
    let watches = core::watches(&app.store);
    let tasks: Vec<Value> = watches.keys().filter_map(|sid| app.store.instance(sid).ok().flatten()).map(|r| session_view(&app, &r, true)).collect();
    let all = core::dispatches(&app.paths);
    let (fixed, of) = core::fix_rate(&all, 50);
    let recent: Vec<&Dispatch> = all.iter().rev().take(200).collect();
    Ok(Json(json!({
        "brain": brain,
        "tasks": tasks,
        "dispatches": recent,
        "agreements": core::agreements(&app.paths),
        "stats": { "fixed": fixed, "of": of },
        "default_model": core::DEFAULT_MODEL,
    })))
}

pub async fn enable(State(app): State<App>) -> R<Json<Value>> {
    Ok(Json(json!({ "brain": ensure_brain(&app).await? })))
}

#[derive(Deserialize)]
pub struct FixReq {
    /// `new`：改成新开一个；`into`：改成发给 `session`
    mode: String,
    #[serde(default)]
    session: Option<String>,
}

/// 用户改了去向：把话送到对的地方，并且让大脑知道（它下次照着来）
pub async fn fix(State(app): State<App>, Path(id): Path<String>, Json(req): Json<FixReq>) -> R<Json<Value>> {
    let d = core::dispatches(&app.paths).into_iter().find(|d| d.id == id).ok_or_else(LocalError::not_found)?;
    if d.fixed.is_some() {
        return Err(LocalError::new("CONFLICT"));
    }
    let wrong = (!d.session.is_empty()).then(|| d.session.clone());
    let to = match req.mode.as_str() {
        "new" if d.kind == "send" => {
            let first = core::compose_brief(Some(&d.said), &d.brief, false);
            let title = d.said.lines().next().unwrap_or(&d.title).chars().take(30).collect::<String>();
            open_session(&app, &d.project, &title, if ENGINES.contains(&d.engine.as_str()) { &d.engine } else { "claude" }, &first).await?
        }
        "into" => {
            let target = req.session.as_deref().map(str::trim).filter(|s| !s.is_empty() && Some(*s) != wrong.as_deref()).ok_or_else(|| LocalError::invalid("session", "required"))?;
            let row = app.store.instance(target)?.filter(|r| !core::is_brain(&app.store, &r.id)).ok_or_else(LocalError::not_found)?;
            let text = if d.kind == "send" { d.brief.clone() } else { core::compose_brief(None, &d.brief, false) };
            app.chat.input(&row.id, &core::compose_followup(Some(&d.said), &text), &[]).await?;
            row.id
        }
        _ => return Err(LocalError::invalid("mode", "invalid")),
    };
    // 发错的那边：新开错的停掉；送错话的告诉它别管（它可能已经动手了，去向条上提醒用户去看一眼）
    if let Some(w) = &wrong {
        if d.kind == "start" {
            let _ = app.chat.interrupt(w).await;
            core::watch_update(&app.store, |m| {
                m.remove(w);
            });
        } else {
            let _ = app.chat.input(w, "上一条消息是看看助理发错地方的，不是给这个会话的，请忽略它。如果你已经按它改了东西，停下来，说明改了什么。", &[]).await;
        }
    } else {
        // 还在排队：不开了
        let q: Vec<Queued> = queue(&app).into_iter().filter(|x| x.dispatch != d.id).collect();
        let _ = app.store.set(QUEUE_KEY, &q);
    }
    let fixed = Fix { at: crate::util::system_ms(), mode: req.mode.clone(), session: Some(to.clone()) };
    let d = core::dispatch_update(&app.paths, &id, |x| x.fixed = Some(fixed)).ok_or_else(LocalError::not_found)?;
    watch(&app, &to, &d.id);
    let to_title = title_by_id(&app, &to);
    let body = format!(
        "用户改了你的一次分派。他的原话是：「{}」。你{}，他改成了{}。看看已经把话送到对的地方了，不用你再送。记住这类话该去哪，下次照他的来；不用回复这条。",
        d.said.chars().take(200).collect::<String>(),
        if d.kind == "start" { "新开了一个会话".to_string() } else { format!("发给了「{}」", d.title) },
        if req.mode == "new" { "新开一个会话".to_string() } else { format!("发给「{to_title}」") },
    );
    tell_brain(&app, event("rerouted", &to, &to_title, &d.project, "", &body)).await;
    Ok(Json(json!({ "dispatch": d, "session": to })))
}

#[derive(Deserialize)]
pub struct AgreementReq {
    id: String,
}

/// 用户在界面上划掉一条约定
pub async fn agreement_remove(State(app): State<App>, Json(req): Json<AgreementReq>) -> R<StatusCode> {
    core::agreement_done(&app.paths, &req.id).map(|_| StatusCode::NO_CONTENT).ok_or_else(LocalError::not_found)
}

/// 不再盯着这个会话（“在干的活”里点掉）
pub async fn unwatch(State(app): State<App>, Path(session): Path<String>) -> StatusCode {
    core::watch_update(&app.store, |m| {
        m.remove(&session);
    });
    StatusCode::NO_CONTENT
}
