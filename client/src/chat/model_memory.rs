//! 记住用户在看看里上次选的模型、思考强度，下次新建对话时当默认（草稿页预先选好）。
//!
//! - **按什么记**：工具（`claude` / `codex`）+ 账号或配置（claude_profiles / codex_profiles 的 ID）。
//!   不同账号、第三方接口能用的模型不一样，A 账号选的不会套到 B 账号上；不按项目记（同一个账号换个项目还是想用同一个模型）。
//! - **什么算“选过”**：草稿页点过模型菜单、对话里换模型或强度（菜单或手打 `/model`、`/effort`），都记下最后一次。
//!   选“默认”就忘掉，回到工具自己的默认。
//! - **优先级**（新建对话）：草稿页这次选的 > 这里记住的 > Claude Code / Codex 自己的默认（settings.json 的 `model`、
//!   `effortLevel`，config.toml 的 `model`、`model_reasoning_effort`，看看配置里指定的模型、模型设置里的强度）。
//!   记的时候同时记下那时候配置里的默认值（`base`）；之后用户改了配置（编辑文件、在终端里用 `/model` 存成默认），
//!   就以配置为准、不再用记住的。
//! - **不可用了**：记住的模型不在这个账号现在的模型列表里（或列表取不到）就不用，强度不在那个模型支持的档位里也不用。
//! - 终端模式不加参数：Claude Code、Codex 在终端里用 `/model` 选的会自己存进配置文件，下次照样生效。

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::store::Store;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Last {
    /// 模型菜单里的值（`opus`、`gpt-5.5`…）；None = 默认
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// 记的时候配置里的默认值（见 [`claude_base`]、[`codex_base`]）；变了说明用户之后改过配置
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub at: i64,
}

fn key(agent: &str, profile: &str) -> String {
    format!("chat.last_model.{agent}.{profile}")
}

/// 记下一次选择（`kind` 是 `model` 或 `effort`；模型选 `default` 等于忘掉）
pub fn record(store: &Store, agent: &str, profile: &str, kind: &str, value: &str, base: String) {
    let k = key(agent, profile);
    let mut l: Last = store.get(&k).ok().flatten().unwrap_or_default();
    // 配置在那之后改过：以前记的作废，从这次重新记
    if l.base != base {
        l = Last::default();
    }
    let v = Some(value.to_string()).filter(|v| !v.is_empty() && v != "default");
    match kind {
        "model" => l.model = v,
        "effort" => l.effort = v,
        _ => return,
    }
    l.base = base;
    l.at = crate::util::system_ms();
    let r = if l.model.is_none() && l.effort.is_none() { store.remove(&k) } else { store.set(&k, &l) };
    if let Err(e) = r {
        tracing::warn!(error = %e, "记不下上次选的模型");
    }
}

/// 草稿页一次设好两样（None = 默认）
pub fn set(store: &Store, agent: &str, profile: &str, model: Option<&str>, effort: Option<&str>, base: String) {
    record(store, agent, profile, "model", model.unwrap_or(""), base.clone());
    record(store, agent, profile, "effort", effort.unwrap_or(""), base);
}

/// 记住的（配置之后改过时没有）
pub fn get(store: &Store, agent: &str, profile: &str, base: &str) -> Option<Last> {
    store.get::<Last>(&key(agent, profile)).ok().flatten().filter(|l| l.base == base)
}

/// 记住的在这次的模型列表里能不能用：返回 `{ model, effort }`（菜单里的值；用不了的那样是 null），都用不了时 None。
/// `fallback`：模型没写支持哪些强度时界面给的通用档位
pub fn usable(last: &Last, options: &[Value], fallback: &[&str]) -> Option<Value> {
    if options.is_empty() {
        return None;
    }
    let base = |s: &str| super::models::base_id(s);
    let found = last.model.as_deref().and_then(|m| {
        options.iter().find(|o| o["value"].as_str() == Some(m)).or_else(|| {
            options.iter().find(|o| [&o["value"], &o["model"]].iter().any(|v| v.as_str().is_some_and(|v| !v.is_empty() && base(v) == base(m))))
        })
    });
    let model = found.and_then(|o| o["value"].as_str()).filter(|v| *v != "default").map(String::from);
    // 强度按实际会用的模型看：记住的模型，否则默认的那个
    let eff_opt = found.or_else(|| options.iter().find(|o| o["default"].as_bool() == Some(true) || o["value"] == "default")).or(options.first());
    let list: Vec<String> = eff_opt.and_then(|o| o["efforts"].as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
    let effort = last.effort.clone().filter(|e| if list.is_empty() { fallback.contains(&e.as_str()) } else { list.contains(e) });
    if model.is_none() && effort.is_none() {
        return None;
    }
    Some(json!({ "model": model, "effort": effort, "at": last.at }))
}

fn read_json(p: &Path) -> Value {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

/// Claude Code 用户级 settings.json 里和模型有关的默认值（`/model`、`/effort` 存的就是这些）
pub fn claude_base(config_dir: &Path) -> String {
    let s = read_json(&config_dir.join("settings.json"));
    let per_model: serde_json::Map<String, Value> =
        s["modelSettings"].as_object().map(|m| m.iter().filter(|(_, v)| !v["effortLevel"].is_null()).map(|(k, v)| (k.clone(), v["effortLevel"].clone())).collect()).unwrap_or_default();
    json!([s["model"], s["effortLevel"], per_model, s["env"]["ANTHROPIC_MODEL"]]).to_string()
}

/// Codex 的默认：config.toml 的 `model`、`model_reasoning_effort`、`profile`，加上看看配置里指定的模型、模型设置里的强度
pub fn codex_base(codex_home: &Path, p: &crate::codex_profiles::Profile) -> String {
    let doc = std::fs::read_to_string(codex_home.join("config.toml")).ok().and_then(|t| t.parse::<toml_edit::DocumentMut>().ok());
    let top = |k: &str| doc.as_ref().and_then(|d| d.get(k)).and_then(|v| v.as_str()).map(String::from);
    json!([top("model"), top("model_reasoning_effort"), top("profile"), p.model, p.tuning.effort]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("t.db")).unwrap();
        (dir, s)
    }

    fn opts() -> Vec<Value> {
        vec![
            json!({ "value": "default", "model": "claude-opus-5-5", "efforts": ["low", "medium", "high"] }),
            json!({ "value": "opus", "model": "claude-opus-5-5", "efforts": ["low", "medium", "high", "max"] }),
            json!({ "value": "haiku", "model": "claude-haiku-4-5-20251001", "efforts": [] }),
        ]
    }

    #[test]
    fn per_profile_and_default_forgets() {
        let (_d, s) = store();
        record(&s, "claude", "official", "model", "opus", "b".into());
        record(&s, "claude", "official", "effort", "max", "b".into());
        assert_eq!(get(&s, "claude", "official", "b").map(|l| (l.model, l.effort)), Some((Some("opus".into()), Some("max".into()))));
        // 别的账号、别的工具没有
        assert!(get(&s, "claude", "acc2", "b").is_none());
        assert!(get(&s, "codex", "official", "b").is_none());
        // 配置之后改过：不用
        assert!(get(&s, "claude", "official", "b2").is_none());
        // 选回默认：模型忘掉，强度还在
        record(&s, "claude", "official", "model", "default", "b".into());
        assert_eq!(get(&s, "claude", "official", "b").map(|l| (l.model, l.effort)), Some((None, Some("max".into()))));
        set(&s, "claude", "official", None, None, "b".into());
        assert!(s.get::<Last>(&key("claude", "official")).unwrap().is_none());
    }

    #[test]
    fn base_change_resets_old_choice() {
        let (_d, s) = store();
        record(&s, "codex", "official", "effort", "high", "b1".into());
        record(&s, "codex", "official", "model", "gpt-5.5", "b2".into());
        let l = get(&s, "codex", "official", "b2").unwrap();
        assert_eq!((l.model.as_deref(), l.effort), (Some("gpt-5.5"), None));
    }

    #[test]
    fn usable_falls_back() {
        let l = |m: Option<&str>, e: Option<&str>| Last { model: m.map(String::from), effort: e.map(String::from), ..Last::default() };
        let o = opts();
        assert_eq!(usable(&l(Some("opus"), Some("max")), &o, &[]), Some(json!({ "model": "opus", "effort": "max", "at": 0 })));
        // 手打的完整 ID 对上菜单里的值
        assert_eq!(usable(&l(Some("claude-haiku-4-5"), None), &o, &[]).unwrap()["model"], "haiku");
        // 模型不在列表里：不用；强度按默认模型的档位看（max 不在里面）
        assert_eq!(usable(&l(Some("gone-model"), Some("max")), &o, &[]), None);
        assert_eq!(usable(&l(Some("gone-model"), Some("high")), &o, &[]), Some(json!({ "model": null, "effort": "high", "at": 0 })));
        // 模型没写档位：用通用的
        assert_eq!(usable(&l(Some("haiku"), Some("xhigh")), &o, &["low", "xhigh"]).unwrap()["effort"], "xhigh");
        assert_eq!(usable(&l(Some("haiku"), Some("ultra")), &o, &["low"]).unwrap()["effort"], Value::Null);
        // 列表取不到：都不用
        assert_eq!(usable(&l(Some("opus"), None), &[], &[]), None);
    }

    #[test]
    fn bases_read_config() {
        let d = tempfile::tempdir().unwrap();
        let b0 = claude_base(d.path());
        std::fs::write(d.path().join("settings.json"), r#"{"model":"opus[1m]","theme":"x","modelSettings":{"claude-opus-5-5":{"effortLevel":"medium","autoCompactWindow":1}}}"#).unwrap();
        let b1 = claude_base(d.path());
        assert_ne!(b0, b1);
        // 别的设置变了不算
        std::fs::write(d.path().join("settings.json"), r#"{"model":"opus[1m]","theme":"y","modelSettings":{"claude-opus-5-5":{"effortLevel":"medium","autoCompactWindow":2}}}"#).unwrap();
        assert_eq!(claude_base(d.path()), b1);
        let p = crate::codex_profiles::list(&store().1).remove(0);
        let c0 = codex_base(d.path(), &p);
        std::fs::write(d.path().join("config.toml"), "model = \"gpt-5.5\"\n[x]\nmodel = \"y\"\n").unwrap();
        assert_ne!(codex_base(d.path(), &p), c0);
    }
}
