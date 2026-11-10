//! 本机装的 Claude Code 自带的模型价格表（它自己 `/cost`、`total_cost_usd` 用的那份）。
//!
//! Claude Code 的程序里有一段手写的模型目录（`"Hand-maintained baked-in model catalog …"`）：
//! `pricing_tiers`（每档的输入、输出、5 分钟 / 1 小时写缓存、读缓存、联网搜索，长提示词另一档 `long_prompt`）
//! 和 `models[]`（`id`、`provider_ids.first_party`、`display_name`、`pricing` 指向哪一档）。
//! 写法是 JS 对象（键不带引号、`!0`、`1e5`），这里用一个只认这些写法的小解析器转成 JSON。
//!
//! 新模型发布当天 Claude Code 就带着价格（models.dev 常晚几天）。认不出（Claude Code 改了写法、装的是别的东西）就当没有，
//! 退回 models.dev。只在 Claude Code 的文件换了（路径、大小、修改时间）时重新读。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

use crate::pricing::{Cost, Long, ModelInfo, Provider};

const MARKER: &[u8] = b"Hand-maintained baked-in model catalog";

/// 上次读的是哪个文件（路径、大小、修改时间）
static SEEN: Mutex<Option<(PathBuf, u64, i64)>> = Mutex::new(None);

/// 下次一定重新读（界面上点了“刷新价格”）
pub fn forget() {
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Claude Code 换了（或者第一次看）：`Some(读到的价格表)`，读不到时 `Some(None)`；没换：`None`
pub fn load_if_changed() -> Option<Option<Provider>> {
    let path = locate();
    let sig = path.as_ref().and_then(|p| {
        let m = std::fs::metadata(p).ok()?;
        let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
        Some((p.clone(), m.len(), mtime))
    });
    let mut g = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if g.is_some() && *g == sig {
        return None;
    }
    *g = sig.clone();
    drop(g);
    Some(sig.and_then(|(p, ..)| read(&p)))
}

/// 找 Claude Code 的程序：PATH 里的 `claude`（跟到链接指向的真文件：原生安装是 `versions/<版本>`，npm 装的是 `cli.js`），
/// 再找官方安装脚本的位置、桌面版自带的
fn locate() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe"] } else { &["claude"] };
    let on_path = std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file()));
    let p = on_path.or_else(|| crate::agents::claude_fallback().map(PathBuf::from))?;
    Some(std::fs::canonicalize(&p).unwrap_or(p))
}

/// 读出价格表：在文件里找到标记，取出它所在的那个对象
fn read(path: &Path) -> Option<Provider> {
    let text = find_object(path)?;
    let v = JsParser { s: text.as_bytes(), i: 0 }.value()?;
    let p = provider(&v);
    if p.is_none() {
        tracing::debug!(path = %path.display(), "Claude Code 的模型目录认不出来");
    }
    p
}

/// 分块读（原生安装的程序有几百 MB，不整个读进内存），找到标记后往回找对象开头 `={`、往后按括号配对找到结尾
fn find_object(path: &Path) -> Option<String> {
    const CHUNK: usize = 8 << 20;
    const BACK: usize = 4096;
    const MAX_OBJ: usize = 512 << 10;
    let mut f = std::fs::File::open(path).ok()?;
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + BACK);
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = find(&buf, MARKER) {
            // 标记在对象的第一个键（"//"）里：往回找 `={`
            let start = buf[..at].windows(2).rposition(|w| w == b"={")? + 1;
            let mut obj = buf[start..].to_vec();
            // 对象可能跨块：再读一些
            while obj.len() < MAX_OBJ {
                if let Some(end) = object_end(&obj) {
                    obj.truncate(end);
                    return String::from_utf8(obj).ok();
                }
                let n = f.read(&mut chunk).ok()?;
                if n == 0 {
                    break;
                }
                obj.extend_from_slice(&chunk[..n]);
            }
            return object_end(&obj).and_then(|end| String::from_utf8(obj[..end].to_vec()).ok());
        }
        // 只留末尾一段（标记可能被块切开），往回找开头也够用
        if buf.len() > BACK + MARKER.len() {
            buf.drain(..buf.len() - BACK - MARKER.len());
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `{` 开头的对象在哪里结束（不含）：括号配对，跳过字符串
fn object_end(b: &[u8]) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// 只认对象、数组、字符串、数字、`!0` / `!1`、`null`、`void 0`、`true` / `false` 的 JS 字面量解析器
struct JsParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn value(&mut self) -> Option<Value> {
        self.ws();
        match *self.s.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = Map::new();
                loop {
                    if self.eat(b'}') {
                        return Some(Value::Object(m));
                    }
                    self.ws();
                    let k = match *self.s.get(self.i)? {
                        q @ (b'"' | b'\'') => self.string(q)?,
                        _ => self.ident()?,
                    };
                    if !self.eat(b':') {
                        return None;
                    }
                    let v = self.value()?;
                    m.insert(k, v);
                    if !self.eat(b',') {
                        return self.eat(b'}').then_some(Value::Object(m));
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = vec![];
                loop {
                    if self.eat(b']') {
                        return Some(Value::Array(a));
                    }
                    a.push(self.value()?);
                    if !self.eat(b',') {
                        return self.eat(b']').then_some(Value::Array(a));
                    }
                }
            }
            q @ (b'"' | b'\'' | b'`') => self.string(q).map(Value::String),
            b'!' => {
                self.i += 1;
                let n = self.number()?;
                Some(Value::Bool(n.as_f64() == Some(0.0)))
            }
            b'-' | b'.' | b'0'..=b'9' => self.number(),
            _ => {
                let id = self.ident()?;
                match id.as_str() {
                    "null" => Some(Value::Null),
                    "true" => Some(Value::Bool(true)),
                    "false" => Some(Value::Bool(false)),
                    "void" => {
                        self.number()?;
                        Some(Value::Null)
                    }
                    // 引用别的变量：不认识，当没有
                    _ => Some(Value::Null),
                }
            }
        }
    }
    fn ident(&mut self) -> Option<String> {
        self.ws();
        let st = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_' || self.s[self.i] == b'$') {
            self.i += 1;
        }
        (self.i > st).then(|| String::from_utf8_lossy(&self.s[st..self.i]).into_owned())
    }
    fn string(&mut self, q: u8) -> Option<String> {
        self.i += 1;
        let mut out = Vec::new();
        while *self.s.get(self.i)? != q {
            let c = self.s[self.i];
            if c == b'\\' {
                self.i += 1;
                match *self.s.get(self.i)? {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let hex = std::str::from_utf8(self.s.get(self.i + 1..self.i + 5)?).ok()?;
                        let ch = char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('?');
                        out.extend_from_slice(ch.to_string().as_bytes());
                        self.i += 4;
                    }
                    other => out.push(other),
                }
            } else {
                out.push(c);
            }
            self.i += 1;
        }
        self.i += 1;
        Some(String::from_utf8_lossy(&out).into_owned())
    }
    fn number(&mut self) -> Option<Value> {
        self.ws();
        let st = self.i;
        while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'-' | b'+') {
            self.i += 1;
        }
        let t = std::str::from_utf8(&self.s[st..self.i]).ok()?;
        let n: f64 = t.parse().ok()?;
        serde_json::Number::from_f64(n).map(Value::Number)
    }
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v[k].as_f64()
}

fn tier_cost(t: &Value) -> Option<Cost> {
    let long = t.get("long_prompt").and_then(|l| {
        Some(Long {
            above: l["above_prompt_tokens"].as_f64()? as u64,
            input: f(l, "input")?,
            output: f(l, "output")?,
            cache_read: f(l, "cache_read"),
            cache_write: f(l, "cache_write_5m"),
            cache_write_1h: f(l, "cache_write_1h"),
        })
    });
    Some(Cost {
        input: f(t, "input")?,
        output: f(t, "output")?,
        cache_read: f(t, "cache_read"),
        cache_write: f(t, "cache_write_5m"),
        cache_write_1h: f(t, "cache_write_1h"),
        web_search: f(t, "web_search"),
        long,
    })
}

/// 解析出的目录 → 服务商 `claude-code`：每个模型按 `id` 和 `first_party`（带日期的那个）各放一份
fn provider(v: &Value) -> Option<Provider> {
    let tiers = v["pricing_tiers"].as_object()?;
    let mut models = BTreeMap::new();
    for m in v["models"].as_array()? {
        let Some(id) = m["id"].as_str() else { continue };
        let Some(cost) = m["pricing"].as_str().and_then(|t| tiers.get(t)).and_then(tier_cost) else { continue };
        let info = ModelInfo { name: m["display_name"].as_str().unwrap_or(id).to_string(), cost: Some(cost), release_date: String::new(), context: m["context"]["window"].as_u64() };
        if let Some(fp) = m["provider_ids"]["first_party"].as_str().filter(|fp| *fp != id) {
            models.insert(fp.to_string(), info.clone());
        }
        models.insert(id.to_string(), info);
    }
    (!models.is_empty()).then(|| Provider { name: "Claude Code".into(), api: String::new(), models })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"//":"Hand-maintained baked-in model catalog — x",schema_version:1,pricing_tiers:{tier_4_20:{input:4,output:20,cache_write_5m:5,cache_write_1h:8,cache_read:0.2,web_search:0.01},haiku_55:{input:0.1,output:0.5,cache_write_5m:0.125,cache_write_1h:0.2,cache_read:0.01,web_search:0.01,long_prompt:{above_prompt_tokens:1e5,input:0.5,output:2.5,cache_write_5m:0.625,cache_write_1h:1,cache_read:0.05}}},models:[{id:"claude-haiku-5-5",family:"haiku",display_name:"Haiku 5.5",provider_ids:{first_party:"claude-haiku-5-5",anthropic_google_cloud:null},eager_input_streaming:{vertex:!0},context:{window:200000,supports_1m_suffix:!0},pricing:"haiku_55",capabilities:["a","b"]},{id:"claude-opus-9",display_name:"Opus 9",provider_ids:{first_party:"claude-opus-9-20270101"},pricing:"tier_4_20",x:void 0,y:Ab}]}"#;

    #[test]
    fn parses_the_baked_catalog() {
        let dir = std::env::temp_dir().join(format!("ll-cprices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude");
        // 前后有别的东西（程序的其它部分）
        let mut bytes = vec![0u8; 9 << 20];
        bytes.extend_from_slice(b"var SOo=");
        bytes.extend_from_slice(SAMPLE.as_bytes());
        bytes.extend_from_slice(b";function z(){return 1}");
        std::fs::write(&file, bytes).unwrap();
        let p = read(&file).unwrap();
        let h = p.models["claude-haiku-5-5"].cost.unwrap();
        assert_eq!((h.input, h.cache_write_1h, h.long.unwrap().above, h.long.unwrap().input), (0.1, Some(0.2), 100_000, 0.5));
        assert_eq!(h.at(150_000).input, 0.5);
        assert_eq!(h.at(50_000).input, 0.1);
        assert_eq!(p.models["claude-opus-9-20270101"].cost.unwrap().output, 20.0);
        assert_eq!(p.models["claude-opus-9"].name, "Opus 9");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本机真的装着 Claude Code 时读一下（CI 上没有就跳过）
    #[test]
    fn real_claude_code() {
        let Some(path) = locate() else { return };
        let Some(p) = read(&path) else { return };
        assert!(p.models.keys().any(|k| k.starts_with("claude-")), "{:?}", p.models.keys().collect::<Vec<_>>());
    }
}
