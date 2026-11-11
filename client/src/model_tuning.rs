//! 模型设置：每个 Claude Code 账号 / Codex 配置（含本机默认）可以另设上下文窗口、自动压缩等常用项，
//! 新建、重新启动的会话生效。只用两边官方文档写明的开关，不改用户自己的配置文件：
//!
//! - **Claude Code**（code.claude.com/docs/en/env-vars、model-config，2.1.322 对照）：都是启动时的环境变量。
//!   - 上下文窗口 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`：只对 Claude Code 认不出的模型名生效（第三方接口的模型名，
//!     不以 `claude-` 开头、不带 `[1m]`），认得出的 Claude 模型照它自己知道的窗口算；所以只给第三方接口用。
//!   - 限制在 200K `CLAUDE_CODE_DISABLE_1M_CONTEXT=1`：原生 1M 的模型也按 200K 压缩（接口只收 200K 时用）。
//!   - 自动压缩开关 `DISABLE_AUTO_COMPACT=1`（手动 /compact 还能用，空闲压缩一起关）；
//!     压缩窗口 `CLAUDE_CODE_AUTO_COMPACT_WINDOW`（10 万到 100 万，Claude Code 再按模型的窗口封顶）。
//!   - 单次最多输出 `CLAUDE_CODE_MAX_OUTPUT_TOKENS`。
//!   - 第三方接口：思考强度 `CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=1`（认不出的模型名也发 effort）、请求超时 `API_TIMEOUT_MS`。
//!   - 不设 `CLAUDE_CODE_EFFORT_LEVEL`：它会盖过对话界面、/effort 里选的强度。
//! - **Codex**（learn.chatgpt.com config-reference，0.157 对照）：`config.toml` 的键，终端里用 `-c` 传，
//!   对话模式（app-server）放进 `thread/start`、`thread/resume`、`thread/fork` 的 `config`，都不改 `~/.codex/config.toml`。
//!   - `model_context_window`、`model_auto_compact_token_limit`、`model_reasoning_effort`、`model_reasoning_summary`、`model_verbosity`；
//!   - 第三方接口：`model_providers.<id>.stream_idle_timeout_ms`（流式回复多久没动静算断了）。
//!
//! 数值原样拼进启动命令，所以只收整数和固定的几个词。

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::LocalError;
use crate::model::LaunchEnv;

fn is_false(b: &bool) -> bool {
    !*b
}

fn range(field: &str, v: Option<u64>, min: u64, max: u64) -> Result<Option<u64>, LocalError> {
    match v {
        Some(n) if !(min..=max).contains(&n) => Err(LocalError::invalid(field, "out_of_range")),
        v => Ok(v),
    }
}

fn pick(field: &str, v: Option<String>, allowed: &[&str]) -> Result<Option<String>, LocalError> {
    match v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) if !allowed.contains(&s.as_str()) => Err(LocalError::invalid(field, "invalid")),
        v => Ok(v),
    }
}

/// Claude Code 的模型设置（都不填 = 照 Claude Code 自己的默认）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClaudeTuning {
    /// 上下文窗口（token），只给第三方接口：`CLAUDE_CODE_MAX_CONTEXT_TOKENS`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// 限制在 200K：`CLAUDE_CODE_DISABLE_1M_CONTEXT=1`
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_1m: bool,
    /// 关掉自动压缩：`DISABLE_AUTO_COMPACT=1`
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_auto_compact: bool,
    /// 上下文到多少时自动压缩：`CLAUDE_CODE_AUTO_COMPACT_WINDOW`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_window: Option<u64>,
    /// 单次最多输出：`CLAUDE_CODE_MAX_OUTPUT_TOKENS`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    /// 第三方接口也发思考强度：`CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=1`
    #[serde(default, skip_serializing_if = "is_false")]
    pub always_effort: bool,
    /// 第三方接口的请求超时（秒）：`API_TIMEOUT_MS`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl ClaudeTuning {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn clean(self) -> Result<Self, LocalError> {
        Ok(ClaudeTuning {
            context_window: range("context_window", self.context_window, 8_000, 10_000_000)?,
            compact_window: range("compact_window", self.compact_window, 100_000, 1_000_000)?,
            max_output: range("max_output", self.max_output, 1_000, 1_000_000)?,
            timeout_secs: range("timeout_secs", self.timeout_secs, 10, 3_600)?,
            ..self
        })
    }

    /// 要设的环境变量（名字、值）。`api`：第三方接口（只对它有意义的几项只在这时加）
    pub fn vars(&self, api: bool) -> Vec<(&'static str, String)> {
        let mut v = vec![];
        if api {
            if let Some(n) = self.context_window {
                v.push(("CLAUDE_CODE_MAX_CONTEXT_TOKENS", n.to_string()));
            }
            if self.always_effort {
                v.push(("CLAUDE_CODE_ALWAYS_ENABLE_EFFORT", "1".into()));
            }
            if let Some(s) = self.timeout_secs {
                v.push(("API_TIMEOUT_MS", (s * 1000).to_string()));
            }
        }
        if self.no_1m {
            v.push(("CLAUDE_CODE_DISABLE_1M_CONTEXT", "1".into()));
        }
        if self.no_auto_compact {
            v.push(("DISABLE_AUTO_COMPACT", "1".into()));
        } else if let Some(n) = self.compact_window {
            v.push(("CLAUDE_CODE_AUTO_COMPACT_WINDOW", n.to_string()));
        }
        if let Some(n) = self.max_output {
            v.push(("CLAUDE_CODE_MAX_OUTPUT_TOKENS", n.to_string()));
        }
        v
    }

    /// 加到启动环境上
    pub fn env(&self, env: LaunchEnv, api: bool) -> LaunchEnv {
        self.vars(api).into_iter().fold(env, |env, (k, v)| env.lit(k, v))
    }
}

/// 0.157 的模型列表里出现过的（各模型支持的不一样，选了模型不支持的由 Codex 自己报错）
pub const CODEX_EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];
pub const CODEX_SUMMARIES: &[&str] = &["auto", "concise", "detailed", "none"];
pub const CODEX_VERBOSITY: &[&str] = &["low", "medium", "high"];

/// Codex 的模型设置（都不填 = 照 Codex 自己的配置）
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexTuning {
    /// `model_context_window`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// `model_auto_compact_token_limit`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_limit: Option<u64>,
    /// `model_reasoning_effort`（默认的；对话里另选的照样优先）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `model_reasoning_summary`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `model_verbosity`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verbosity: Option<String>,
    /// 第三方接口：`model_providers.<id>.stream_idle_timeout_ms`（秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_secs: Option<u64>,
}

impl CodexTuning {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn clean(self) -> Result<Self, LocalError> {
        let context_window = range("context_window", self.context_window, 8_000, 10_000_000)?;
        let compact_limit = range("compact_limit", self.compact_limit, 1_000, 10_000_000)?;
        if let (Some(w), Some(c)) = (context_window, compact_limit) {
            if c >= w {
                return Err(LocalError::invalid("compact_limit", "over_window"));
            }
        }
        Ok(CodexTuning {
            context_window,
            compact_limit,
            effort: pick("effort", self.effort, CODEX_EFFORTS)?,
            summary: pick("summary", self.summary, CODEX_SUMMARIES)?,
            verbosity: pick("verbosity", self.verbosity, CODEX_VERBOSITY)?,
            idle_timeout_secs: range("idle_timeout_secs", self.idle_timeout_secs, 10, 3_600)?,
        })
    }

    /// 顶层的 `config.toml` 键和值（不含服务商的）
    fn pairs(&self) -> Vec<(&'static str, Value)> {
        let mut v = vec![];
        if let Some(n) = self.context_window {
            v.push(("model_context_window", json!(n)));
        }
        if let Some(n) = self.compact_limit {
            v.push(("model_auto_compact_token_limit", json!(n)));
        }
        for (k, s) in [("model_reasoning_effort", &self.effort), ("model_reasoning_summary", &self.summary), ("model_verbosity", &self.verbosity)] {
            if let Some(s) = s {
                v.push((k, json!(s)));
            }
        }
        v
    }

    /// 终端里的 Codex：每项一个 `-c 键=值`（字符串不加引号：Codex 把解析不了的 TOML 值当字符串，这几个词都不是 TOML）
    pub fn configs(&self, mut env: LaunchEnv) -> LaunchEnv {
        for (k, v) in self.pairs() {
            let v = v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
            env = env.config(format!("{k}={v}"));
        }
        env
    }

    /// 定义第三方服务商时一起加的 `-c`
    pub fn provider_configs(&self, env: LaunchEnv, provider: &str) -> LaunchEnv {
        match self.idle_timeout_secs {
            Some(s) => env.config(format!("model_providers.{provider}.stream_idle_timeout_ms={}", s * 1000)),
            None => env,
        }
    }

    /// app-server 的 `thread/start` 等的 `config`；没设时 None
    pub fn thread_config(&self) -> Option<Value> {
        let m: Map<String, Value> = self.pairs().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        (!m.is_empty()).then(|| Value::Object(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shells::Family;

    #[test]
    fn claude_env() {
        let t = ClaudeTuning { context_window: Some(256_000), compact_window: Some(200_000), max_output: Some(64_000), always_effort: true, timeout_secs: Some(900), ..Default::default() }.clean().unwrap();
        let api = t.env(LaunchEnv::new(), true).wrap("claude", Family::Posix);
        assert_eq!(
            api,
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS='256000' CLAUDE_CODE_ALWAYS_ENABLE_EFFORT='1' API_TIMEOUT_MS='900000' CLAUDE_CODE_AUTO_COMPACT_WINDOW='200000' CLAUDE_CODE_MAX_OUTPUT_TOKENS='64000' claude"
        );
        let official = t.env(LaunchEnv::new(), false).wrap("claude", Family::Posix);
        assert_eq!(official, "CLAUDE_CODE_AUTO_COMPACT_WINDOW='200000' CLAUDE_CODE_MAX_OUTPUT_TOKENS='64000' claude");
        let off = ClaudeTuning { no_auto_compact: true, compact_window: Some(300_000), no_1m: true, ..Default::default() };
        assert_eq!(off.env(LaunchEnv::new(), false).wrap("c", Family::Posix), "CLAUDE_CODE_DISABLE_1M_CONTEXT='1' DISABLE_AUTO_COMPACT='1' c", "关了自动压缩就不设窗口");
        assert!(ClaudeTuning { compact_window: Some(50_000), ..Default::default() }.clean().is_err());
        assert!(ClaudeTuning::default().is_empty());
        // 旧数据 / 空对象能读
        assert_eq!(serde_json::from_str::<ClaudeTuning>("{}").unwrap(), ClaudeTuning::default());
    }

    #[test]
    fn codex_config() {
        let t = CodexTuning { context_window: Some(400_000), compact_limit: Some(300_000), effort: Some("high".into()), summary: Some(" none ".into()), idle_timeout_secs: Some(600), ..Default::default() }.clean().unwrap();
        assert_eq!(t.summary.as_deref(), Some("none"));
        let cmd = t.provider_configs(t.configs(LaunchEnv::new()), "looklook-x").wrap("codex", Family::Posix);
        assert_eq!(
            cmd,
            " codex -c model_context_window=400000 -c model_auto_compact_token_limit=300000 -c model_reasoning_effort=high -c model_reasoning_summary=none -c model_providers.looklook-x.stream_idle_timeout_ms=600000"
        );
        assert_eq!(t.thread_config().unwrap(), json!({ "model_context_window": 400000, "model_auto_compact_token_limit": 300000, "model_reasoning_effort": "high", "model_reasoning_summary": "none" }));
        assert!(CodexTuning::default().thread_config().is_none());
        assert!(CodexTuning { effort: Some("huge".into()), ..Default::default() }.clean().is_err());
        assert!(CodexTuning { context_window: Some(200_000), compact_limit: Some(200_000), ..Default::default() }.clean().is_err());
    }
}
