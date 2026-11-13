//! 飞书卡片（JSON 2.0，docs/TASK_BOARD.md §6.3、§6.4）。
//!
//! - 卡片都带 `update_multi: true`，之后能整张换掉（处理掉以后变灰、去掉按钮）；
//! - 会话里来的文字（命令、回复、问题）一律放 `plain_text` 或代码块，不当 markdown 解析，免得混进链接；
//! - 按钮回调的 `value` 见 [`Act`]，回到 `notify.rs` 处理。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 按钮上带的东西（回调原样带回来）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Act {
    /// allow / deny / answer / dialog / say / stop / test；机器人的 bot_stop / bot_lark（feishu_bot.rs）
    pub a: String,
    /// 会话所在的电脑
    #[serde(default)]
    pub dev: String,
    #[serde(default)]
    pub inst: String,
    /// 这张卡片对应的“等你处理”（`notify::attn_key`）
    #[serde(default)]
    pub p: String,
    /// 电脑发卡片时生成的随机数，处理时核对
    #[serde(default)]
    pub n: String,
    /// 选第几项
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub i: Option<usize>,
}

/// 卡片上共有的信息
pub struct Info<'a> {
    pub en: bool,
    /// Claude / Codex
    pub agent: &'a str,
    pub title: &'a str,
    pub project: &'a str,
    pub device: &'a str,
    /// “打开”的链接
    pub url: &'a str,
    /// 卡片里显示命令和回复内容（§6.5 内容开关）
    pub content: bool,
    /// 能在飞书里处理（关掉 = 只通知）
    pub actions: bool,
}

/// 中英文案
fn tr(en: bool, zh: &str, e: &str) -> String {
    if en { e.into() } else { zh.into() }
}

fn plain(s: &str) -> Value {
    json!({ "tag": "plain_text", "content": s })
}

fn div(s: &str) -> Value {
    json!({ "tag": "div", "text": plain(s) })
}

fn note(s: &str) -> Value {
    json!({ "tag": "div", "text": { "tag": "plain_text", "content": s, "text_size": "notation", "text_color": "grey" } })
}

/// 命令放代码块，最多 10 行
fn code(s: &str) -> Value {
    let lines: Vec<&str> = s.lines().take(10).collect();
    let mut body = lines.join("\n").replace("```", "'''");
    if s.lines().count() > 10 {
        body.push_str("\n…");
    }
    json!({ "tag": "markdown", "content": format!("```bash\n{body}\n```") })
}

fn button(text: &str, kind: &str, act: &Act, confirm: Option<(&str, &str)>) -> Value {
    let mut b = json!({ "tag": "button", "text": plain(text), "type": kind, "behaviors": [{ "type": "callback", "value": act }] });
    if let Some((title, body)) = confirm {
        b["confirm"] = json!({ "title": plain(title), "text": plain(body) });
    }
    b
}

fn link(text: &str, url: &str) -> Value {
    json!({ "tag": "button", "text": plain(text), "type": "default", "behaviors": [{ "type": "open_url", "default_url": url }] })
}

fn row(buttons: Vec<Value>) -> Value {
    let columns: Vec<Value> = buttons.into_iter().map(|b| json!({ "tag": "column", "width": "auto", "elements": [b] })).collect();
    json!({ "tag": "column_set", "flex_mode": "flow", "horizontal_spacing": "8px", "columns": columns })
}

/// 回复一句话的输入框（送进会话，§9 第 3 步）
fn say_form(i: &Info, act: &Act) -> Value {
    json!({
        "tag": "form",
        "name": "say",
        "elements": [
            { "tag": "input", "name": "text", "placeholder": plain(&tr(i.en, "回复一句话，发给 AI…", "Reply to the AI…")), "max_length": 1000, "width": "fill" },
            { "tag": "button", "name": "send", "text": plain(&tr(i.en, "发送", "Send")), "type": "primary", "form_action_type": "submit", "behaviors": [{ "type": "callback", "value": act }] },
        ],
    })
}

fn card(header: Value, summary: &str, elements: Vec<Value>) -> Value {
    json!({
        "schema": "2.0",
        "config": { "update_multi": true, "summary": { "content": summary } },
        "header": header,
        "body": { "elements": elements },
    })
}

fn header(title: &str, sub: &str, template: &str, tag: Option<(&str, &str)>) -> Value {
    let mut h = json!({ "title": plain(title), "subtitle": plain(sub), "template": template });
    if let Some((text, color)) = tag {
        h["text_tag_list"] = json!([{ "tag": "text_tag", "text": plain(text), "color": color }]);
    }
    h
}

fn subtitle(i: &Info) -> String {
    let mut s = String::new();
    if !i.title.is_empty() {
        s.push_str(i.title);
        s.push_str(" · ");
    }
    s.push_str(i.project);
    if !i.device.is_empty() {
        s.push_str(" · ");
        s.push_str(i.device);
    }
    s
}

fn open(i: &Info) -> Value {
    link(&tr(i.en, "打开", "Open"), i.url)
}

/// “等你处理”的卡片
pub struct Attn<'a> {
    /// permission / question / plan / limit / dialog
    pub kind: &'a str,
    pub tool: &'a str,
    pub detail: &'a str,
    pub risk: Option<&'a str>,
    /// 命中的命令规则（docs/PLAN_2026-10-27.md §3.2）
    pub guard: Option<&'a str>,
    pub options: &'a [String],
    /// 额度菜单里“等重置后自动继续”是第几项
    pub limit_wait: Option<usize>,
}

pub fn attn(i: &Info, a: &Attn, base: &Act) -> Value {
    let act = |name: &str, idx: Option<usize>| Act { a: name.into(), i: idx, ..base.clone() };
    let high = a.risk == Some("high") || a.guard.is_some();
    let (title, template) = match a.kind {
        "permission" => (tr(i.en, &format!("{} 在等你批准", i.agent), &format!("{} needs your approval", i.agent)), if high { "red" } else { "orange" }),
        "question" => (tr(i.en, &format!("{} 有个问题", i.agent), &format!("{} has a question", i.agent)), "orange"),
        "plan" => (tr(i.en, &format!("{} 写好了计划", i.agent), &format!("{} finished a plan", i.agent)), "orange"),
        "limit" => (tr(i.en, "额度用完了", "Usage limit reached"), "orange"),
        _ => (tr(i.en, &format!("{} 在等你选择", i.agent), &format!("{} is waiting for a choice", i.agent)), "orange"),
    };
    let tag = high.then_some((if i.en { "High risk" } else { "高风险" }, "red"));
    let mut els = Vec::new();
    let mut buttons = Vec::new();
    match a.kind {
        "permission" => {
            els.push(div(&tr(i.en, &format!("要用 {}：", a.tool), &format!("Wants to use {}:", a.tool))));
            if i.content && !a.detail.is_empty() {
                els.push(if a.tool == "Bash" { code(a.detail) } else { div(a.detail) });
            }
            if let Some(rule) = a.guard {
                els.push(note(&tr(i.en, &format!("命中了看看的命令规则：{rule}"), &format!("Matches a Looklook command rule: {rule}"))));
            }
            if i.actions {
                let confirm = high.then_some((if i.en { "Allow a high-risk action?" } else { "允许高风险操作？" }, if i.en { "This runs on your computer right away." } else { "允许后会马上在电脑上执行。" }));
                buttons.push(button(&tr(i.en, "允许", "Allow"), if high { "danger" } else { "primary" }, &act("allow", None), confirm));
                buttons.push(button(&tr(i.en, "拒绝", "Deny"), "default", &act("deny", None), None));
            }
        }
        "question" => {
            if i.content && !a.detail.is_empty() {
                els.push(div(a.detail));
            }
            if i.actions {
                for (n, o) in a.options.iter().enumerate().take(4) {
                    buttons.push(button(o, "default", &act("answer", Some(n)), None));
                }
            }
        }
        "plan" => {
            if i.content && !a.detail.is_empty() {
                els.push(div(a.detail));
            }
            els.push(note(&tr(i.en, "计划要打开看完再批准。", "Open it to read the whole plan before approving.")));
        }
        "limit" => {
            els.push(div(&tr(i.en, "可以等额度重置后自动继续，或者打开换一个账号。", "Wait for the reset and continue automatically, or open it to switch accounts.")));
            if let (true, Some(n)) = (i.actions, a.limit_wait) {
                buttons.push(button(&tr(i.en, "等重置后继续", "Wait and continue"), "primary", &act("dialog", Some(n)), None));
            }
        }
        _ => {
            if !a.detail.is_empty() {
                els.push(div(a.detail));
            }
            if i.actions && a.options.len() <= 4 {
                for (n, o) in a.options.iter().enumerate() {
                    buttons.push(button(o, "default", &act("dialog", Some(n)), None));
                }
            }
        }
    }
    buttons.push(open(i));
    els.push(row(buttons));
    if i.actions && a.kind == "question" && !a.options.is_empty() {
        els.push(say_form(i, &act("say", None)));
    }
    card(header(&title, &subtitle(i), template, tag), &title, els)
}

/// 一轮结束：完成或出错
pub fn ended(i: &Info, failed: bool, text: &str, took_ms: i64, base: &Act) -> Value {
    let title = if failed { tr(i.en, "出错了", "Something went wrong") } else { tr(i.en, "做完了", "Done") };
    let mut els = Vec::new();
    if i.content && !text.is_empty() {
        let shown: String = if failed { text.lines().next().unwrap_or_default().into() } else { text.lines().take(5).collect::<Vec<_>>().join("\n") };
        els.push(div(&shown));
    }
    if !failed && took_ms > 0 {
        els.push(note(&tr(i.en, &format!("用时 {}", duration(took_ms, false)), &format!("Took {}", duration(took_ms, true)))));
    }
    els.push(row(vec![open(i)]));
    if i.actions {
        els.push(say_form(i, &Act { a: "say".into(), ..base.clone() }));
    }
    card(header(&title, &subtitle(i), if failed { "red" } else { "green" }, None), &format!("{title} · {}", i.title), els)
}

/// 处理掉了：灰色，去掉按钮，写明结果
pub fn resolved(i: &Info, result: &str) -> Value {
    card(header(result, &subtitle(i), "grey", None), result, vec![row(vec![open(i)])])
}

/// 回复已经送进会话
pub fn said(i: &Info, text: &str) -> Value {
    let title = tr(i.en, "已发给 AI", "Sent to the AI");
    let mut els = Vec::new();
    if i.content {
        els.push(div(text));
    }
    els.push(row(vec![open(i)]));
    card(header(&title, &subtitle(i), "grey", None), &title, els)
}

/// 设置页“发一条测试”：点按钮能收到回调才算配好（漏了 `card.action.trigger` 时会报 200340）
pub fn test(en: bool, act: &Act, ok: bool) -> Value {
    if ok {
        let title = tr(en, "看看：按钮回调已接通", "LookLook: button callbacks work");
        return card(header(&title, "", "green", None), &title, vec![div(&tr(en, "配置完成，之后的通知会发到这里。", "All set. Notifications will arrive here."))]);
    }
    let title = tr(en, "看看：测试通知", "LookLook: test notification");
    card(
        header(&title, "", "blue", None),
        &title,
        vec![
            div(&tr(en, "能看到这条，说明发消息没问题。再点一下下面的按钮，检查按钮回调。", "Sending works. Tap the button below to check button callbacks.")),
            note(&tr(en, "点了报错 200340：到开发者后台“事件与回调 → 回调配置”里加上“卡片回传交互”，再发布一个版本。", "Error 200340 when tapping: add “card.action.trigger” under Events & Callbacks → Callback configuration, then publish a new version.")),
            row(vec![button(&tr(en, "点一下", "Tap me"), "primary", act, None)]),
        ],
    )
}

/// 汇总卡片里的一行：一轮结束了（勿扰期间、或“做完的合并发”攒下的，docs/PLAN_2026-10-27.md §2）
pub struct Line {
    pub agent: String,
    pub title: String,
    pub project: String,
    pub device: String,
    pub failed: bool,
    pub text: String,
}

/// 汇总卡片最多列几行
const SUMMARY_MAX: usize = 10;

/// 勿扰时段结束、或合并的时间到了：一张卡片列出这段时间结束的每一轮
pub fn summary(en: bool, quiet: bool, lines: &[Line], content: bool, url: &str) -> Value {
    let failed = lines.iter().filter(|l| l.failed).count();
    let title = match (quiet, en) {
        (true, false) => format!("勿扰期间结束了 {} 个会话", lines.len()),
        (true, true) => format!("{} sessions finished during quiet hours", lines.len()),
        (false, false) => format!("{} 个会话做完了", lines.len()),
        (false, true) => format!("{} sessions finished", lines.len()),
    };
    let mut els = Vec::new();
    for l in lines.iter().take(SUMMARY_MAX) {
        let mark = if l.failed { "✗" } else { "✓" };
        let mut head = format!("{mark} {} · {}", l.agent, if l.title.is_empty() { &l.project } else { &l.title });
        if !l.title.is_empty() {
            head.push_str(&format!(" · {}", l.project));
        }
        if !l.device.is_empty() {
            head.push_str(&format!(" · {}", l.device));
        }
        els.push(div(&head));
        if content && !l.text.is_empty() {
            els.push(note(l.text.lines().next().unwrap_or_default()));
        }
    }
    if lines.len() > SUMMARY_MAX {
        let more = lines.len() - SUMMARY_MAX;
        els.push(note(&tr(en, &format!("还有 {more} 个"), &format!("and {more} more"))));
    }
    els.push(row(vec![link(&tr(en, "打开任务", "Open Tasks"), url)]));
    let template = if failed > 0 { "orange" } else { "green" };
    card(header(&title, "", template, None), &title, els)
}

/// 正在干活、但很久没有新进展了（卡住提醒）
pub fn stalled(i: &Info, doing: &str, idle_ms: i64, base: &Act) -> Value {
    let title = tr(i.en, "看起来没有动静了", "No progress for a while");
    let mut els = vec![div(&tr(
        i.en,
        &format!("已经 {}没有新的进展。可能卡住了，也可能在跑很久的命令。", duration(idle_ms, false)),
        &format!("Nothing new for {}. It may be stuck, or running a long command.", duration(idle_ms, true)),
    ))];
    if i.content && !doing.is_empty() {
        els.push(note(&tr(i.en, &format!("正在：{doing}"), &format!("Doing: {doing}"))));
    }
    let mut buttons = vec![open(i)];
    if i.actions {
        let stop = Act { a: "stop".into(), ..base.clone() };
        buttons.push(button(&tr(i.en, "停止", "Stop"), "danger", &stop, Some((&tr(i.en, "停止这一轮？", "Stop this turn?"), &tr(i.en, "相当于在会话里按 Esc。", "Same as pressing Esc in the session.")))));
    }
    els.push(row(buttons));
    card(header(&title, &subtitle(i), "orange", None), &format!("{title} · {}", i.title), els)
}

/// 没登录的电脑：卡片末尾加一句开通远程访问（“打开”指向这台电脑，离开它就打不开）
pub fn remote_hint(mut c: Value, en: bool, url: &str) -> Value {
    if let Some(els) = c["body"]["elements"].as_array_mut() {
        els.push(note(&tr(
            en,
            "“打开”只在这台电脑上能用。开通远程访问后，出门在外用手机一点就能接着处理（新用户注册送一年）。",
            "\"Open\" only works on this computer. Turn on remote access to pick things up from your phone anywhere, free for your first year.",
        )));
        els.push(row(vec![link(&tr(en, "开通远程访问", "Turn on remote access"), url)]));
    }
    c
}

/// 超出每分钟上限时合并成一条
pub fn digest(en: bool, n: usize, url: &str) -> Value {
    let title = tr(en, &format!("还有 {n} 条通知没发"), &format!("{n} more notifications"));
    card(
        header(&title, "", "orange", None),
        &title,
        vec![div(&tr(en, "一分钟里通知太多了，打开任务页看全部。", "Too many notifications in a minute. Open the Tasks page to see them all.")), row(vec![link(&tr(en, "打开任务", "Open Tasks"), url)])],
    )
}

// ---------------- 飞书机器人的回答卡片（docs/FEISHU_BOT.md） ----------------

/// 流式卡片里正文、状态行的组件 id
pub const STREAM_BODY: &str = "body";
pub const STREAM_STATUS: &str = "status";
pub const STREAM_TOOLS: &str = "tools";

/// AI 的正文：markdown（代码块、列表要能看），只去掉 `<at`，不让 AI 输出 @ 人
pub fn body_markdown(text: &str) -> String {
    text.replace("<at", "‹at")
}

/// 回答卡片：没有卡片头，看起来像一条消息，不像一条通知
fn answer(summary: &str, elements: Vec<Value>, streaming: bool) -> Value {
    let mut config = json!({ "update_multi": true, "summary": { "content": summary } });
    if streaming {
        config["streaming_mode"] = json!(true);
    }
    json!({ "schema": "2.0", "config": config, "body": { "elements": elements } })
}

/// 折叠面板（默认收起）
fn panel(title: &str, elements: Vec<Value>) -> Value {
    json!({
        "tag": "collapsible_panel",
        "expanded": false,
        "header": {
            "title": { "tag": "markdown", "content": title, "text_size": "notation" },
            "icon": { "tag": "standard_icon", "token": "down-small-ccm_outlined", "size": "16px 16px" },
            "icon_position": "right",
            "icon_expanded_angle": -180,
        },
        "border": { "color": "grey", "corner_radius": "5px" },
        "vertical_spacing": "4px",
        "padding": "4px 8px 4px 8px",
        "elements": elements,
    })
}

/// 小字的一行（页脚、状态行）
fn small(id: Option<&str>, s: &str) -> Value {
    let mut v = json!({ "tag": "markdown", "content": s, "text_size": "notation" });
    if let Some(id) = id {
        v["element_id"] = json!(id);
    }
    v
}

/// “Claude · 项目 · 电脑”（没有项目时“助理 · 电脑”）
pub fn whose(i: &Info) -> String {
    let mut s = i.agent.to_string();
    if !i.project.is_empty() {
        s.push_str(" · ");
        s.push_str(i.project);
    }
    if !i.device.is_empty() {
        s.push_str(" · ");
        s.push_str(i.device);
    }
    s
}

/// 一轮里 AI 用的一个工具
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub name: String,
    /// 命令、文件名、搜的词…（一行，已截短）
    pub detail: String,
    pub done: bool,
    pub failed: bool,
}

/// 工具步骤：每步一行；太多只留最后的
pub fn steps_markdown(steps: &[Step], en: bool) -> String {
    const MAX: usize = 30;
    let mut lines = Vec::new();
    if steps.len() > MAX {
        lines.push(tr(en, &format!("…前面还有 {} 步", steps.len() - MAX), &format!("…{} earlier steps", steps.len() - MAX)));
    }
    for s in steps.iter().skip(steps.len().saturating_sub(MAX)) {
        let mark = if s.failed { "✗" } else if s.done { "✓" } else { "…" };
        let name = body_markdown(&s.name.replace(['*', '`'], ""));
        if s.detail.is_empty() {
            lines.push(format!("{mark} **{name}**"));
        } else {
            lines.push(format!("{mark} **{name}** `{}`", body_markdown(&s.detail.replace('`', "'"))));
        }
    }
    lines.join("\n")
}

fn steps_title(n: usize, en: bool) -> String {
    tr(en, &format!("过程 · {n} 步"), &format!("Steps · {n}"))
}

/// 一轮的流式卡片：上面是工具步骤（折叠），正文边写边出，下面一行小字写正在做什么
pub fn stream(i: &Info, stop: &Act) -> Value {
    let mut buttons = vec![];
    if i.actions {
        buttons.push(button(&tr(i.en, "停止", "Stop"), "default", stop, None));
    }
    buttons.push(open(i));
    // 流式接口只能改 markdown / plain_text 组件的文字：面板标题定死，里面的步骤边跑边推
    answer(
        &tr(i.en, "处理中…", "Working…"),
        vec![
            panel(&tr(i.en, "过程", "Steps"), vec![small(Some(STREAM_TOOLS), "…")]),
            json!({ "tag": "markdown", "element_id": STREAM_BODY, "content": "…" }),
            small(Some(STREAM_STATUS), &tr(i.en, "正在思考…", "Thinking…")),
            row(buttons),
        ],
        true,
    )
}

/// 一轮结束：整卡换成结果。`outcome`：done / failed / stopped
pub fn stream_done(i: &Info, outcome: &str, text: &str, steps: &[Step], thinking: &str, took_ms: i64) -> Value {
    let status = match outcome {
        "done" => tr(i.en, "已完成", "Done"),
        "failed" => tr(i.en, "<font color='red'>出错了</font>", "<font color='red'>Failed</font>"),
        _ => tr(i.en, "已停止", "Stopped"),
    };
    let mut els = vec![];
    if !steps.is_empty() {
        els.push(panel(&steps_title(steps.len(), i.en), vec![small(None, &steps_markdown(steps, i.en))]));
    }
    if !thinking.is_empty() {
        els.push(panel(&tr(i.en, "💭 思考", "💭 Thinking"), vec![small(None, &body_markdown(thinking))]));
    }
    els.push(json!({ "tag": "markdown", "element_id": STREAM_BODY, "content": if text.is_empty() { "…" } else { text } }));
    let mut foot = vec![status];
    if took_ms > 0 {
        foot.push(tr(i.en, &format!("用时 {}", duration(took_ms, false)), &format!("took {}", duration(took_ms, true))));
    }
    foot.push(body_markdown(&whose(i)));
    els.push(small(None, &foot.join(" · ")));
    els.push(row(vec![open(i)]));
    // 会话列表里的预览：回答的开头
    let preview: String = text.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '#' | '>' | '[' | ']')).take(100).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
    let summary = if preview.is_empty() { whose(i) } else { preview };
    answer(&summary, els, false)
}

/// 缺权限：在飞书里点链接确认就补上（`register::begin` 带着 App ID），补好后换成 [`granted`]
pub fn grant(en: bool, why: &str, url: &str) -> Value {
    let title = tr(en, "看看需要再开通一点权限", "LookLook needs a few more permissions");
    card(
        header(&title, "", "orange", None),
        &title,
        vec![div(why), note(&tr(en, "点下面的按钮，在飞书里确认就开通了（1 小时内有效）。", "Tap the button and confirm in Feishu (valid for 1 hour).")), row(vec![link(&tr(en, "去开通", "Grant"), url)])],
    )
}

pub fn granted(en: bool) -> Value {
    let title = tr(en, "权限已开通", "Permissions granted");
    card(header(&title, "", "green", None), &title, vec![note(&tr(en, "可以接着用了。", "You can continue now."))])
}

/// 机器人的帮助卡片（`welcome`：第一次说话时附的欢迎卡片）：怎么用 + 飞书 CLI 现在到哪一步、下一步的按钮或链接
pub fn bot_help(en: bool, lark: &crate::lark_cli::Status, act: &Act, page: &str, welcome: bool) -> Value {
    use crate::lark_cli::State;
    let title = if welcome { tr(en, "你好，我是看看助理", "Hi, I'm the LookLook assistant") } else { tr(en, "怎么用", "How to use") };
    let how = tr(
        en,
        "**私聊直接说**：跟看看助理说，和在看看的助理页说一样。它会自己找项目、把活派给 Claude Code / Codex，有结果再告诉你\n**在群里**：把我拉进群，@ 我说话。别人的问题：引用那条消息再 @ 我，我回答时会 @ 那个人（说“别 @”就不 @）。只有你能叫我，别人 @ 我我不理\n**停止**：停下助理正在说的这一轮\n**帮助**：再看这张卡片",
        "**Message me directly**: talk to the LookLook assistant, same as on its page. It finds the project, hands work to Claude Code / Codex and reports back\n**In a group**: add me and @ me. For someone else's question, quote their message and @ me; my answer will @ them (say “don't @” to skip). Only you can call me; I ignore others\n**stop**: stop the assistant's current turn\n**help**: show this card again",
    );
    let mut els = vec![json!({ "tag": "markdown", "content": how }), json!({ "tag": "hr" })];
    let state = match lark.state {
        State::Ready => tr(en, "✅ **飞书 CLI 已装好**：助理会照飞书官方的技能以你的身份读写飞书（文档、日历、多维表格、聊天记录）；第一次用时它会发链接让你在飞书里授权，发消息前会先问你。", "✅ **Feishu CLI is installed**: the assistant follows Feishu's official skills to read and write Feishu as you (docs, calendar, Base, chats). The first time, it sends you a link to authorize in Feishu, and it asks before sending anything."),
        State::Missing => tr(
            en,
            &format!("**飞书 CLI**：机器人只能以机器人的身份收发消息。装上飞书官方的命令行工具和配套技能，助理就能以你的身份读写文档、日历、多维表格和聊天记录。\n官方安装命令：`{}`", lark.command),
            &format!("**Feishu CLI**: the bot can only talk as a bot. Install Feishu's official CLI and its skills so the assistant can read and write your docs, calendar, Base and chats as you.\nOfficial install command: `{}`", lark.command),
        ),
    };
    els.push(json!({ "tag": "markdown", "content": state }));
    if let Some(e) = &lark.error {
        els.push(note(&tr(en, &format!("上次没装上：{e}"), &format!("The last install failed: {e}"))));
    }
    if lark.running.is_some() {
        els.push(note(&tr(en, "正在装（1–3 分钟），装好这张卡片会自己更新。", "Installing (1–3 minutes). This card updates when it's done.")));
    } else if lark.state == State::Missing {
        els.push(row(vec![button(&tr(en, "装到这台电脑", "Install on this computer"), "primary", act, None), link(&tr(en, "在看看里看", "Open in LookLook"), page)]));
    }
    card(header(&title, "", "blue", None), &title, els)
}

/// 用时：12 秒、3 分、1 小时 5 分
pub fn duration(ms: i64, en: bool) -> String {
    let s = ms / 1000;
    match (s, en) {
        (s, false) if s < 60 => format!("{s} 秒"),
        (s, true) if s < 60 => format!("{s}s"),
        (s, false) if s < 3600 => format!("{} 分", s / 60),
        (s, true) if s < 3600 => format!("{}m", s / 60),
        (s, false) => format!("{} 小时 {} 分", s / 3600, s % 3600 / 60),
        (s, true) => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(actions: bool) -> Info<'static> {
        Info { en: false, agent: "Claude", title: "发布看板", project: "looklook", device: "工作站", url: "https://a.example/s/x", content: true, actions }
    }

    fn buttons(c: &Value) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(v: &Value, out: &mut Vec<String>) {
            if v["tag"] == "button" {
                out.push(v["text"]["content"].as_str().unwrap_or_default().to_string());
            }
            match v {
                Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                Value::Object(m) => m.values().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        walk(c, &mut out);
        out
    }

    #[test]
    fn high_risk_permission() {
        let base = Act { dev: "d".into(), inst: "i".into(), p: "7".into(), n: "abc".into(), ..Default::default() };
        let a = Attn { kind: "permission", tool: "Bash", detail: "git push --force", risk: Some("high"), guard: None, options: &[], limit_wait: None };
        let c = attn(&info(true), &a, &base);
        assert_eq!(c["schema"], "2.0");
        assert_eq!(c["config"]["update_multi"], true);
        assert_eq!(c["header"]["template"], "red");
        assert_eq!(c["header"]["text_tag_list"][0]["text"]["content"], "高风险");
        assert_eq!(buttons(&c), ["允许", "拒绝", "打开"]);
        let allow = &c["body"]["elements"][2]["columns"][0]["elements"][0];
        assert_eq!(allow["behaviors"][0]["value"]["a"], "allow");
        assert_eq!(allow["behaviors"][0]["value"]["n"], "abc");
        assert!(allow["confirm"].is_object());
        assert!(c["body"]["elements"][1]["content"].as_str().unwrap().contains("```bash\ngit push --force\n```"));
        // 只通知：只剩“打开”
        assert_eq!(buttons(&attn(&info(false), &a, &base)), ["打开"]);
    }

    #[test]
    fn question_and_content_switch() {
        let opts = vec!["SQLite".to_string(), "Postgres".to_string()];
        let a = Attn { kind: "question", tool: "AskUserQuestion", detail: "用哪个？", risk: None, guard: None, options: &opts, limit_wait: None };
        let c = attn(&info(true), &a, &Act::default());
        assert_eq!(buttons(&c), ["SQLite", "Postgres", "打开", "发送"]);
        let hidden = Info { content: false, ..info(true) };
        assert!(!attn(&hidden, &a, &Act::default()).to_string().contains("用哪个"));
    }

    #[test]
    fn ended_and_duration() {
        let c = ended(&info(true), false, "已修好。\n第二行", 754_000, &Act::default());
        assert_eq!(c["header"]["template"], "green");
        assert!(c.to_string().contains("用时 12 分"));
        assert_eq!(duration(3_900_000, true), "1h 5m");
        assert_eq!(resolved(&info(true), "已允许 · 在飞书上")["header"]["template"], "grey");
    }

    #[test]
    fn summary_and_stalled() {
        let line = |failed: bool, n: usize| Line { agent: "Claude".into(), title: format!("任务{n}"), project: "looklook".into(), device: "工作站".into(), failed, text: "改好了。\n第二行".into() };
        let lines: Vec<Line> = (0..12).map(|n| line(n == 3, n)).collect();
        let c = summary(false, true, &lines, true, "https://a.example/tasks");
        assert_eq!(c["header"]["template"], "orange");
        let s = c.to_string();
        assert!(s.contains("勿扰期间结束了 12 个会话"));
        assert!(s.contains("✗ Claude · 任务3 · looklook · 工作站"));
        assert!(s.contains("还有 2 个"));
        assert!(!s.contains("第二行"));
        assert!(!summary(true, false, &lines[..1], false, "u").to_string().contains("改好了"));
        let c = stalled(&info(true), "运行 cargo build", 1_260_000, &Act { n: "x".into(), ..Default::default() });
        assert_eq!(buttons(&c), ["打开", "停止"]);
        assert!(c.to_string().contains("已经 21 分没有新的进展"));
        assert_eq!(buttons(&stalled(&info(false), "", 60_000, &Act::default())), ["打开"]);
    }

    #[test]
    fn bot_cards() {
        let c = stream(&info(true), &Act { a: "bot_stop".into(), n: "n".into(), ..Default::default() });
        assert_eq!(c["config"]["streaming_mode"], true);
        assert_eq!(c["config"]["update_multi"], true);
        assert!(c.get("header").is_none(), "回答卡片没有卡片头");
        assert_eq!(c["body"]["elements"][0]["elements"][0]["element_id"], STREAM_TOOLS);
        assert_eq!(c["body"]["elements"][1]["element_id"], STREAM_BODY);
        assert_eq!(c["body"]["elements"][2]["element_id"], STREAM_STATUS);
        assert_eq!(buttons(&c), ["停止", "打开"]);
        assert_eq!(buttons(&stream(&info(false), &Act::default())), ["打开"]);

        let steps = [Step { name: "Bash".into(), detail: "cargo `test`".into(), done: true, failed: false }, Step { name: "Edit".into(), detail: String::new(), done: true, failed: true }];
        let c = stream_done(&info(true), "done", "**改好了**，跑过测试。", &steps, "先看看", 65_000);
        assert!(c.get("header").is_none());
        assert!(c["config"].get("streaming_mode").is_none());
        assert_eq!(c["config"]["summary"]["content"], "改好了，跑过测试。");
        let s = c.to_string();
        assert!(s.contains("过程 · 2 步") && s.contains("✓ **Bash** `cargo 'test'`") && s.contains("✗ **Edit**"), "{s}");
        assert!(s.contains("💭 思考"));
        assert!(s.contains("已完成 · 用时 1 分 · Claude · looklook · 工作站"), "{s}");
        assert_eq!(buttons(&c), ["打开"]);
        let stopped = stream_done(&info(true), "stopped", "", &[], "", 0).to_string();
        assert!(stopped.contains("已停止") && !stopped.contains("过程") && !stopped.contains("思考"));
        assert!(stream_done(&info(true), "failed", "x", &[], "", 0).to_string().contains("出错了"));
        let asst = Info { project: "", agent: "看看助理", ..info(true) };
        assert_eq!(whose(&asst), "看看助理 · 工作站");
        let many: Vec<Step> = (0..35).map(|n| Step { name: format!("T{n}"), detail: String::new(), done: false, failed: false }).collect();
        let md = steps_markdown(&many, false);
        assert!(md.starts_with("…前面还有 5 步") && md.contains("… **T34**") && !md.contains("**T4**"));
        assert!(grant(false, "缺表情权限", "https://open.feishu.cn/page/launcher?x").to_string().contains("去开通"));
        assert_eq!(body_markdown("<at id=all></at>"), "‹at id=all></at>");

        // 帮助 / 欢迎卡片：飞书 CLI 每一步
        use crate::lark_cli::{State, Status};
        let st = |state, running| Status { state, cli: false, skills: false, running, error: None, npm: true, command: "npx @larksuite/cli@latest install --lang zh".into() };
        let act = Act { a: "bot_lark".into(), n: "n".into(), ..Default::default() };
        let c = bot_help(false, &st(State::Missing, None), &act, "u", true);
        assert!(c.to_string().contains("你好，我是看看助理") && c.to_string().contains("引用那条消息再 @ 我") && c.to_string().contains("npx @larksuite/cli@latest install"));
        assert_eq!(buttons(&c), ["装到这台电脑", "在看看里看"]);
        let busy = bot_help(false, &st(State::Missing, Some("install")), &act, "u", false);
        assert!(buttons(&busy).is_empty() && busy.to_string().contains("正在装"));
        let ready = bot_help(true, &st(State::Ready, None), &act, "u", false);
        assert!(ready.to_string().contains("Feishu CLI is installed") && buttons(&ready).is_empty());
    }

    #[test]
    fn guest_hint_keeps_buttons() {
        let act = Act { a: "test".into(), n: "n".into(), ..Default::default() };
        let c = remote_hint(test(true, &act, false), true, "https://looklook.test/register");
        assert!(c.to_string().contains("Turn on remote access") && c.to_string().contains("https://looklook.test/register"));
        assert!(buttons(&c).len() >= 2);
    }
}
