//! 对话宠物（docs/PET_PACK.md）：找宠物包、校验清单、提供文件、导入和删除。
//!
//! - 只认本格式（looklook-pet），只有一个来源：`local`（`{LOOKLOOK_HOME}/pets/`，导入、从 GitHub 装的都放这里）；
//! - 换算后的清单里文件都写成相对的键（`anim/idle.webm?v=…`），前端按场合拼前缀（设置页 `/pets/…`，对话页 `/instances/{id}/pet/…`）；
//! - 文件只认清单里登记过的键，其余一律 404。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::LocalError;

/// 整个包、单个文件、文件个数的上限（§2）
pub const MAX_PACK: u64 = 200 << 20;
pub const MAX_FILE: u64 = 8 << 20;
pub const MAX_FILES: usize = 400;
const MAX_MANIFEST: u64 = 1 << 20;
/// 状态（§3.4）
const STATES: &[&str] = &[
    "idle", "thinking", "tool", "toolDone", "waiting", "terminal", "done", "error", "offline",
    // 看得懂在干什么（§3.4.1）：没配时按 FALLBACK 用 done / error，都没有就只说话
    "testPass", "testFail", "commit", "hello", "levelUp",
];
const AGENTS: &[&str] = &["claude", "codex", "dsh", "opencode"];
const SETTINGS_KEY: &str = "pet";
/// 装进 `pets/<id>/` 时记下从哪来的（现在只有 GitHub：`{"github": "owner/repo", "ref", "path"}`）
pub(crate) const ORIGIN: &str = ".origin.json";

// ---------------- 清单 ----------------

/// 一段文字：一个字符串，或者按语言分的 `{ "zh-CN": …, "en-US": … }`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Text {
    One(String),
    Many(BTreeMap<String, String>),
}

impl Text {
    fn is_blank(&self) -> bool {
        match self {
            Text::One(s) => s.trim().is_empty(),
            Text::Many(m) => m.values().all(|s| s.trim().is_empty()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct License {
    pub spdx: String,
    pub commercial: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Credit {
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Stage {
    pub width: u32,
    pub height: u32,
    pub ground: f64,
    #[serde(rename = "box", skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    pub facing: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
}

impl Default for Stage {
    fn default() -> Self {
        Self { width: 0, height: 0, ground: 0.85, bbox: None, facing: "left".into(), fps: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Sprite {
    pub image: String,
    pub cell: [u32; 2],
    pub columns: u32,
}

impl Default for Sprite {
    fn default() -> Self {
        Self { image: String::new(), cell: [192, 208], columns: 8 }
    }
}

/// 走动参数（§3.3）：距离以 `stage.width` 的像素为单位
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Walk {
    pub min: f64,
    pub max: f64,
    pub lead: f64,
    pub tail: f64,
    /// 循环的走动段（精灵图）每秒走多少（stage 像素）
    pub pace: f64,
}

impl Default for Walk {
    fn default() -> Self {
        Self { min: 60.0, max: 240.0, lead: 0.0, tail: 0.0, pace: 120.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Clip {
    #[serde(rename = "loop")]
    pub looped: bool,
    pub turn: bool,
    pub mirror: bool,
    #[serde(rename = "move", skip_serializing_if = "Option::is_none")]
    pub walk: Option<Walk>,
    /// 只往这个方向走（`left` / `right`，精灵图里分了左右的跑步行）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub speed: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub durations: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<Text>,
    /// 输出时填：文件的键（带版本），没有就是 None
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub webm: Option<String>,
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub mov: Option<String>,
}

impl Default for Clip {
    fn default() -> Self {
        Self {
            looped: false,
            turn: false,
            mirror: true,
            walk: None,
            dir: None,
            speed: 1.0,
            row: None,
            frames: None,
            fps: None,
            durations: None,
            title: None,
            webm: None,
            mov: None,
        }
    }
}

/// 动作池里的一项：写成名字，或 `{ "clip": …, "weight": … }`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(from = "PickIn")]
pub struct Pick {
    pub clip: String,
    pub weight: f64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PickIn {
    Name(String),
    Weighted {
        clip: String,
        #[serde(default = "one")]
        weight: f64,
    },
}

fn one() -> f64 {
    1.0
}

impl From<PickIn> for Pick {
    fn from(p: PickIn) -> Self {
        match p {
            PickIn::Name(clip) => Pick { clip, weight: 1.0 },
            PickIn::Weighted { clip, weight } => Pick { clip, weight },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SleepSet {
    pub start: Vec<Pick>,
    #[serde(rename = "loop")]
    pub looped: Vec<Pick>,
    pub end: Vec<Pick>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub weight: f64,
    pub clips: Vec<Pick>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Actions {
    pub click: Vec<Pick>,
    /// 被摸头（鼠标在身上来回蹭）；没配用 click
    pub pet: Vec<Pick>,
    /// 连着点太多下，烦了；没配用 click
    pub annoyed: Vec<Pick>,
    pub drag: Vec<Pick>,
    pub land: Vec<Pick>,
    pub turn: Vec<Pick>,
    #[serde(rename = "move")]
    pub walk: Vec<Pick>,
    pub sleep: Vec<SleepSet>,
    pub random: Vec<Group>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Behavior {
    pub idle: f64,
    pub turn: f64,
    #[serde(rename = "move")]
    pub walk: f64,
    #[serde(rename = "sleepAfter")]
    pub sleep_after: u32,
}

impl Default for Behavior {
    fn default() -> Self {
        Self { idle: 40.0, turn: 5.0, walk: 10.0, sleep_after: 600 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<Text>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Text>,
    #[serde(rename = "packVersion", skip_serializing_if = "Option::is_none")]
    pub pack_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<Person>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,
    pub credits: Vec<Credit>,
    pub agents: Vec<String>,
    pub renderer: String,
    pub stage: Stage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprite: Option<Sprite>,
    pub clips: BTreeMap<String, Clip>,
    pub states: BTreeMap<String, Vec<Pick>>,
    pub actions: Actions,
    pub behavior: Behavior,
    pub lines: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// 人设（§3.6）：生成碎碎念文案时交给模型；不写就按名字和介绍
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persona: Option<Text>,
    /// 内置的通用台词（包里没写的键用它补）；false = 只说包里写的
    #[serde(rename = "builtinLines", skip_serializing_if = "Option::is_none")]
    pub builtin_lines: Option<bool>,
    /// 输出时填：封面的键
    #[serde(skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

// ---------------- 包 ----------------

/// 包有问题：`code` 是界面上的文案键（`settings.pet_err_*`），`detail` 是哪个字段 / 文件 / 片段
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PackError {
    pub code: &'static str,
    pub detail: String,
}

pub(crate) fn perr(code: &'static str, detail: impl Into<String>) -> PackError {
    PackError { code, detail: detail.into() }
}

#[derive(Debug, Clone)]
pub struct Pack {
    pub source: &'static str,
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// 键（`anim/idle.webm`、`preview.webp`、`sprite.webp`）→ 磁盘上的文件
    pub files: BTreeMap<String, PathBuf>,
    pub error: Option<PackError>,
}

impl Pack {
    pub fn key(&self) -> String {
        format!("{}:{}", self.source, self.id)
    }

    fn broken(source: &'static str, dir: &Path, e: PackError) -> Self {
        let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Self { source, id, dir: dir.to_path_buf(), manifest: Manifest::default(), files: BTreeMap::new(), error: Some(e) }
    }

    /// 设置页列表里的一项
    pub fn summary(&self) -> Value {
        let m = &self.manifest;
        json!({
            "key": self.key(),
            "source": self.source,
            "id": self.id,
            "dir": self.dir.display().to_string(),
            "name": m.name,
            "description": m.description,
            "author": m.author,
            "license": m.license,
            "credits": m.credits,
            "agents": m.agents,
            "renderer": m.renderer,
            "packVersion": m.pack_version,
            "clips": m.clips.len(),
            "mov": m.renderer == "sprite" || (!m.clips.is_empty() && m.clips.keys().all(|c| self.files.contains_key(&format!("anim/{c}.mov")))),
            "preview": self.manifest_out().preview,
            "error": self.error,
            "origin": (self.source == "local").then(|| std::fs::read(self.dir.join(ORIGIN)).ok()).flatten().and_then(|b| serde_json::from_slice::<Value>(&b).ok()),
        })
    }

    /// 给前端的完整清单：文件换成带版本的键
    pub fn manifest_out(&self) -> Manifest {
        let mut m = self.manifest.clone();
        let key = |k: &str| self.files.get(k).map(|p| format!("{}?v={}", enc_path(k), version(p)));
        for (name, c) in m.clips.iter_mut() {
            c.webm = key(&format!("anim/{name}.webm"));
            c.mov = key(&format!("anim/{name}.mov"));
        }
        if let Some(s) = m.sprite.as_mut() {
            if let Some(k) = key("sprite") {
                s.image = k;
            }
        }
        m.preview = key("preview");
        m
    }

    pub fn file(&self, key: &str) -> Option<&PathBuf> {
        self.files.get(key)
    }
}

/// 文件的版本：修改时间 + 大小（换了文件 URL 就变，浏览器缓存可以一直留）
fn version(p: &Path) -> String {
    let m = std::fs::metadata(p).ok();
    let t = m.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    format!("{t:x}{:x}", m.map(|m| m.len()).unwrap_or(0))
}

/// 路径里每一段做百分号编码
fn enc_path(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !id.starts_with('-')
}

/// 本格式的文件名 / 片段名（§2）
fn valid_name(n: &str) -> bool {
    let b = n.as_bytes();
    !b.is_empty() && b.len() <= 64 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}

// ---------------- JSONC ----------------

/// 去掉 `//`、`/* */` 注释和对象、数组里多余的逗号，再按 JSON 解析
pub fn parse_jsonc(src: &str) -> Result<Value, String> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut out = String::with_capacity(src.len());
    let mut it = src.chars().peekable();
    let mut in_str = false;
    while let Some(c) = it.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = it.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if it.peek() == Some(&'/') => {
                while let Some(&n) = it.peek() {
                    if n == '\n' {
                        break;
                    }
                    it.next();
                }
            }
            '/' if it.peek() == Some(&'*') => {
                it.next();
                let mut prev = ' ';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    // 多余的逗号：`,` 后面（跳过空白）紧跟 `}` 或 `]`
    let chars: Vec<char> = out.chars().collect();
    let mut clean = String::with_capacity(chars.len());
    let mut in_str = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            clean.push(c);
            if c == '\\' && i + 1 < chars.len() {
                clean.push(chars[i + 1]);
                i += 1;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            clean.push(c);
        } else if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j >= chars.len() || !matches!(chars[j], '}' | ']') {
                clean.push(c);
            }
        } else {
            clean.push(c);
        }
        i += 1;
    }
    serde_json::from_str(&clean).map_err(|e| e.to_string())
}

fn read_json(p: &Path) -> Result<Value, PackError> {
    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let len = std::fs::metadata(p).map_err(|_| perr("missing_file", &name))?.len();
    if len > MAX_MANIFEST {
        return Err(perr("too_big", name));
    }
    let raw = std::fs::read_to_string(p).map_err(|_| perr("manifest", format!("{name}: not UTF-8")))?;
    parse_jsonc(&raw).map_err(|e| perr("manifest", format!("{name}: {e}")))
}

// ---------------- 图片尺寸 ----------------

/// PNG / WebP 的宽高（只读文件头）
pub fn image_size(p: &Path) -> Option<(u32, u32)> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = [0u8; 32];
    let n = f.read(&mut b).ok()?;
    let b = &b[..n];
    if b.len() >= 24 && b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)));
    }
    if b.len() >= 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        let le24 = |x: &[u8]| u32::from(x[0]) | u32::from(x[1]) << 8 | u32::from(x[2]) << 16;
        return match &b[12..16] {
            b"VP8X" => Some((1 + le24(&b[24..27]), 1 + le24(&b[27..30]))),
            b"VP8L" => {
                let (b0, b1, b2, b3) = (u32::from(b[21]), u32::from(b[22]), u32::from(b[23]), u32::from(b[24]));
                Some((1 + (b0 | (b1 & 0x3f) << 8), 1 + (b1 >> 6 | b2 << 2 | (b3 & 0xf) << 10)))
            }
            b"VP8 " => Some((u32::from(u16::from_le_bytes([b[26], b[27]]) & 0x3fff), u32::from(u16::from_le_bytes([b[28], b[29]]) & 0x3fff))),
            _ => None,
        };
    }
    None
}

// ---------------- 本格式 ----------------

fn small_file(dir: &Path, rel: &str) -> Result<Option<PathBuf>, PackError> {
    let p = dir.join(rel);
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.file_type().is_symlink() => Err(perr("bad_name", rel)),
        Ok(m) if m.is_file() => {
            if m.len() > MAX_FILE {
                Err(perr("too_big", rel))
            } else {
                Ok(Some(p))
            }
        }
        _ => Ok(None),
    }
}

/// 读一个本格式的包（§3），`v` 是已经解析好的 pet.json
fn load_ours(source: &'static str, dir: &Path, v: Value) -> Result<Pack, PackError> {
    let mut m: Manifest = serde_json::from_value(v).map_err(|e| perr("manifest", e.to_string()))?;
    if m.renderer == "sprite" {
        let s = m.sprite.get_or_insert_with(Sprite::default);
        if m.stage.width == 0 {
            m.stage.width = s.cell[0];
        }
        if m.stage.height == 0 {
            m.stage.height = s.cell[1];
        }
    }
    validate(&m)?;
    let mut files = BTreeMap::new();
    let preview = ["preview.webp", "preview.png"].iter().find_map(|n| small_file(dir, n).transpose()).transpose()?;
    files.insert("preview".into(), preview.ok_or_else(|| perr("missing_file", "preview.webp"))?);
    if m.renderer == "video" {
        for name in m.clips.keys() {
            let webm = format!("anim/{name}.webm");
            files.insert(webm.clone(), small_file(dir, &webm)?.ok_or_else(|| perr("missing_file", &webm))?);
            let mov = format!("anim/{name}.mov");
            if let Some(p) = small_file(dir, &mov)? {
                files.insert(mov, p);
            }
        }
    } else {
        let s = m.sprite.as_ref().expect("sprite");
        if !valid_name(&s.image) {
            return Err(perr("bad_name", &s.image));
        }
        let p = small_file(dir, &s.image)?.ok_or_else(|| perr("missing_file", &s.image))?;
        check_sheet(&m, &p, &s.image)?;
        files.insert("sprite".into(), p);
    }
    Ok(Pack { source, id: m.id.clone(), dir: dir.to_path_buf(), manifest: m, files, error: None })
}

/// 精灵图的尺寸够不够放下清单里写的行和帧
fn check_sheet(m: &Manifest, p: &Path, name: &str) -> Result<(), PackError> {
    let s = m.sprite.as_ref().expect("sprite");
    let (w, h) = image_size(p).ok_or_else(|| perr("bad_image", name))?;
    if s.cell[0] == 0 || s.cell[1] == 0 || s.columns == 0 || w < s.cell[0] * s.columns {
        return Err(perr("bad_image", name));
    }
    let rows = h / s.cell[1];
    for (cname, c) in &m.clips {
        if c.row.unwrap_or(0) >= rows || c.frames.unwrap_or(1) > s.columns {
            return Err(perr("bad_clip", cname));
        }
    }
    Ok(())
}

/// 清单的字段和引用都对（§3）
pub fn validate(m: &Manifest) -> Result<(), PackError> {
    if m.format != "looklook-pet" {
        return Err(perr("field", "format"));
    }
    if m.version != 1 {
        return Err(perr("version", m.version.to_string()));
    }
    if !valid_id(&m.id) {
        return Err(perr("field", "id"));
    }
    if m.author.as_ref().is_none_or(|a| a.name.trim().is_empty()) {
        return Err(perr("field", "author.name"));
    }
    let lic = m.license.as_ref().ok_or_else(|| perr("field", "license"))?;
    if lic.spdx.trim().is_empty() {
        return Err(perr("field", "license.spdx"));
    }
    if lic.commercial.is_none() {
        return Err(perr("field", "license.commercial"));
    }
    if let Some(a) = m.agents.iter().find(|a| !AGENTS.contains(&a.as_str())) {
        return Err(perr("field", format!("agents: {a}")));
    }
    if m.name.as_ref().is_none_or(Text::is_blank) {
        return Err(perr("field", "name"));
    }
    match m.renderer.as_str() {
        "video" => {}
        "sprite" if m.sprite.is_some() => {}
        "sprite" => return Err(perr("field", "sprite")),
        _ => return Err(perr("field", "renderer")),
    }
    let st = &m.stage;
    if st.width == 0 || st.height == 0 || st.width > 2048 || st.height > 2048 {
        return Err(perr("field", "stage"));
    }
    if !(0.0..=1.0).contains(&st.ground) || !matches!(st.facing.as_str(), "left" | "right") {
        return Err(perr("field", "stage"));
    }
    if m.clips.is_empty() {
        return Err(perr("field", "clips"));
    }
    for (name, c) in &m.clips {
        if !valid_name(name) {
            return Err(perr("bad_name", name));
        }
        if !(c.speed > 0.0 && c.speed <= 4.0) {
            return Err(perr("bad_clip", name));
        }
        if let Some(w) = &c.walk {
            if w.min < 0.0 || w.max < w.min || w.lead < 0.0 || w.tail < 0.0 || w.pace <= 0.0 {
                return Err(perr("bad_clip", name));
            }
        }
        if c.dir.as_deref().is_some_and(|d| !matches!(d, "left" | "right")) {
            return Err(perr("bad_clip", name));
        }
        if m.renderer == "sprite" && (c.row.is_none() || c.frames.is_none_or(|f| f == 0)) {
            return Err(perr("bad_clip", name));
        }
    }
    let check = |list: &[Pick], what: &str| -> Result<(), PackError> {
        for p in list {
            if !m.clips.contains_key(&p.clip) {
                return Err(perr("bad_ref", format!("{what}: {}", p.clip)));
            }
            if !(p.weight > 0.0) {
                return Err(perr("field", format!("{what}: {}", p.clip)));
            }
        }
        Ok(())
    };
    for (k, list) in &m.states {
        if !STATES.contains(&k.as_str()) {
            return Err(perr("field", format!("states.{k}")));
        }
        check(list, &format!("states.{k}"))?;
    }
    if m.states.get("idle").is_none_or(|l| l.is_empty()) {
        return Err(perr("field", "states.idle"));
    }
    let a = &m.actions;
    for (list, what) in [(&a.click, "click"), (&a.pet, "pet"), (&a.annoyed, "annoyed"), (&a.drag, "drag"), (&a.land, "land"), (&a.turn, "turn"), (&a.walk, "move")] {
        check(list, &format!("actions.{what}"))?;
    }
    if let Some(p) = a.turn.iter().find(|p| !m.clips[&p.clip].turn) {
        return Err(perr("bad_clip", &p.clip));
    }
    if let Some(p) = a.walk.iter().find(|p| m.clips[&p.clip].walk.is_none()) {
        return Err(perr("bad_clip", &p.clip));
    }
    for s in &a.sleep {
        if s.start.is_empty() || s.looped.is_empty() || s.end.is_empty() {
            return Err(perr("field", "actions.sleep"));
        }
        check(&s.start, "actions.sleep")?;
        check(&s.looped, "actions.sleep")?;
        check(&s.end, "actions.sleep")?;
    }
    for g in &a.random {
        if !(g.weight > 0.0) || g.clips.is_empty() {
            return Err(perr("field", format!("actions.random.{}", g.id)));
        }
        check(&g.clips, &format!("actions.random.{}", g.id))?;
    }
    let b = &m.behavior;
    if b.idle < 0.0 || b.turn < 0.0 || b.walk < 0.0 {
        return Err(perr("field", "behavior"));
    }
    Ok(())
}

/// 一个文件夹里的包：只认本格式（`format: "looklook-pet"`）
pub fn load_dir(source: &'static str, dir: &Path) -> Result<Pack, PackError> {
    let v = read_json(&dir.join("pet.json"))?;
    if v["format"] != "looklook-pet" {
        return Err(perr("format", v["format"].as_str().unwrap_or_default()));
    }
    load_ours(source, dir, v)
}

// ---------------- 扫描 ----------------

pub fn local_dir(home: &Path) -> PathBuf {
    home.join("pets")
}

fn scan_dir(source: &'static str, dir: &Path) -> Vec<Pack> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path()).collect();
    dirs.sort();
    dirs.iter().filter(|d| d.join("pet.json").is_file()).map(|d| load_dir(source, d).unwrap_or_else(|e| Pack::broken(source, d, e))).collect()
}

/// 装好的宠物（`{LOOKLOOK_HOME}/pets/` 下每个带 pet.json 的文件夹；id 重复的只留第一个）
pub fn scan(home: &Path) -> Vec<Pack> {
    let mut all = scan_dir("local", &local_dir(home));
    let mut seen = std::collections::HashSet::new();
    all.retain(|p| seen.insert(p.key()));
    all
}

static CACHE: Mutex<Option<(Instant, PathBuf, Vec<Pack>)>> = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(10);

/// 扫描结果缓存 10 秒；`fresh` 时重新扫
pub fn all(home: &Path, fresh: bool) -> Vec<Pack> {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, h, packs)) = c.as_ref() {
        if !fresh && h == home && at.elapsed() < CACHE_TTL {
            return packs.clone();
        }
    }
    let packs = scan(home);
    *c = Some((Instant::now(), home.to_path_buf(), packs.clone()));
    packs
}

pub(crate) fn forget() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn find(home: &Path, source: &str, id: &str) -> Option<Pack> {
    all(home, false).into_iter().find(|p| p.source == source && p.id == id && p.error.is_none())
}

// ---------------- 设置（§6.3） ----------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PetSettings {
    pub enabled: bool,
    /// 对话类型 → 宠物（`source:id`），None = 这种对话不显示
    #[serde(rename = "byAgent")]
    pub by_agent: BTreeMap<String, Option<String>>,
    /// 上面没写的对话类型用哪只
    pub default: Option<String>,
    pub size: f64,
    pub lines: bool,
    /// 碎碎念：按运行数据（做完一轮花了多久、改了几个文件、测试过没过……）套文案说一句，不花 token
    pub chatter: bool,
    /// 桌面应用里宠物住在桌面上（独立的透明窗口，应用最小化也在），不在页面里
    pub desktop: bool,
    /// 生成人设文案用的接口（OpenAI 兼容）；没配就用本机的 Claude Code / Codex
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<GenApi>,
}

impl Default for PetSettings {
    fn default() -> Self {
        Self { enabled: true, by_agent: BTreeMap::new(), default: None, size: 1.0, lines: true, chatter: true, desktop: true, api: None }
    }
}

/// OpenAI 兼容的接口：`{url}/chat/completions`
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GenApi {
    pub url: String,
    pub key: String,
    pub model: String,
    /// 只在给前端时用：存着密钥（密钥本身不发给页面）
    #[serde(rename = "keySet", skip_deserializing)]
    pub key_set: bool,
}

impl PetSettings {
    pub fn load(store: &crate::store::Store) -> Self {
        store.get(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    /// 给页面看的：密钥换成 `keySet`
    pub fn masked(mut self) -> Self {
        if let Some(a) = self.api.as_mut() {
            a.key_set = !a.key.is_empty();
            a.key.clear();
        }
        self
    }

    pub fn save(mut self, store: &crate::store::Store) -> Result<Self, LocalError> {
        // 页面不知道密钥：没填新的就留着原来的
        if let Some(a) = self.api.as_mut() {
            a.url = a.url.trim().trim_end_matches('/').to_string();
            a.model = a.model.trim().to_string();
            a.key = a.key.trim().to_string();
            if a.key.is_empty() {
                a.key = Self::load(store).api.map(|o| o.key).unwrap_or_default();
            }
            if !(a.url.starts_with("http://") || a.url.starts_with("https://")) || a.url.len() > 500 || a.model.len() > 200 || a.key.len() > 1000 {
                return Err(LocalError::invalid("pet", "api"));
            }
        }
        if self.api.as_ref().is_some_and(|a| a.url.is_empty() && a.model.is_empty()) {
            self.api = None;
        }
        self.size = if self.size.is_finite() { self.size.clamp(0.5, 2.0) } else { 1.0 };
        self.by_agent.retain(|k, _| AGENTS.contains(&k.as_str()));
        let ok = |k: &Option<String>| k.as_ref().is_none_or(|k| k.len() <= 200 && k.contains(':'));
        if !self.by_agent.values().all(ok) || !ok(&self.default) {
            return Err(LocalError::invalid("pet", "format"));
        }
        store.set(SETTINGS_KEY, &self)?;
        crate::desk_pet::set_wanted(self.enabled && self.desktop);
        Ok(self.masked())
    }

    /// 这种对话用哪只（`source:id`）
    pub fn pick(&self, agent: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        match self.by_agent.get(agent) {
            Some(v) => v.as_deref(),
            None => self.default.as_deref(),
        }
    }
}

/// 对话页要的：这个对话用哪只、它的清单、大小和气泡开关
pub fn for_agent(home: &Path, store: &crate::store::Store, agent: &str) -> Value {
    let s = PetSettings::load(store);
    let pack = s.pick(agent).and_then(|k| k.split_once(':')).and_then(|(src, id)| find(home, src, id));
    json!({
        "pet": pack.as_ref().map(|p| json!({ "key": p.key(), "source": p.source, "id": p.id, "manifest": p.manifest_out() })),
        "size": s.size,
        "lines": s.lines,
        "chatter": s.chatter,
        "desktop": s.desktop,
        "gen": pack.as_ref().map(|p| gen_lines(store, &p.key())),
    })
}

// ---------------- 好感度、碎碎念文案 ----------------

const BOND_KEY: &str = "pet.bond";
static BOND_LOCK: Mutex<()> = Mutex::new(());

/// 和一只宠物处了多久、一起干了多少事（存在这台电脑上，手机电脑打开都算）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Bond {
    /// 好感（经验）：摸头、点、陪着做完一轮都加
    pub xp: u64,
    pub clicks: u64,
    pub pets: u64,
    pub turns: u64,
    pub tests: u64,
    pub fixes: u64,
    pub commits: u64,
    pub errors: u64,
    /// 第一次见面（毫秒）
    pub met: u64,
    /// 一起过了几天（有打开过的日子）、最后一天（浏览器本地日期 `YYYY-MM-DD`）
    pub days: u32,
    #[serde(rename = "lastDay")]
    pub last_day: String,
}

/// 加一些计数。`day` 是页面的本地日期：换了一天 `days` + 1，返回里 `newDay` = true（页面据此打招呼）
pub fn bond_add(store: &crate::store::Store, key: &str, add: &BTreeMap<String, u64>, day: Option<&str>) -> Result<Value, LocalError> {
    let _g = BOND_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all: BTreeMap<String, Bond> = store.get(BOND_KEY).ok().flatten().unwrap_or_default();
    let b = all.entry(key.to_string()).or_default();
    if b.met == 0 && day.is_some() {
        b.met = crate::util::system_ms().max(0) as u64;
    }
    // 一次最多加这么多，防止页面出错刷爆
    let n = |k: &str| add.get(k).copied().unwrap_or(0).min(50);
    b.xp += n("xp");
    b.clicks += n("clicks");
    b.pets += n("pets");
    b.turns += n("turns");
    b.tests += n("tests");
    b.fixes += n("fixes");
    b.commits += n("commits");
    b.errors += n("errors");
    let mut new_day = false;
    if let Some(d) = day.filter(|d| d.len() == 10 && d.bytes().all(|c| c.is_ascii_digit() || c == b'-')) {
        if b.last_day.as_str() < d {
            b.last_day = d.to_string();
            b.days += 1;
            new_day = true;
        }
    }
    let out = b.clone();
    if all.len() > 200 {
        // 删掉的宠物留下的：只留最近的
        let mut keys: Vec<_> = all.iter().map(|(k, v)| (v.last_day.clone(), k.clone())).collect();
        keys.sort();
        for (_, k) in keys.into_iter().take(all.len() - 200) {
            all.remove(&k);
        }
    }
    store.set(BOND_KEY, &all)?;
    let mut v = serde_json::to_value(out).unwrap_or_default();
    v["newDay"] = json!(new_day);
    Ok(v)
}

fn gen_key(pet: &str) -> String {
    format!("pet.lines.{pet}")
}

/// 按人设生成的一批文案：`{ lang: { 键: [句子] } , at, via }`
pub fn gen_lines(store: &crate::store::Store, pet: &str) -> Value {
    store.get::<Value>(&gen_key(pet)).ok().flatten().unwrap_or(Value::Null)
}

pub fn set_gen_lines(store: &crate::store::Store, pet: &str, v: Option<&Value>) -> Result<(), LocalError> {
    match v {
        Some(v) => store.set(&gen_key(pet), v)?,
        None => store.remove(&gen_key(pet))?,
    }
    Ok(())
}

// ---------------- 文件 ----------------

/// 片段、封面、精灵图：整个文件（片段不大，前端取成 blob 再播，不需要 Range）
pub async fn serve(home: PathBuf, source: String, id: String, key: String) -> Result<Response, LocalError> {
    let pack = tokio::task::spawn_blocking({
        let home = home.clone();
        move || find(&home, &source, &id)
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(LocalError::not_found)?;
    let path = pack.file(&key).cloned().ok_or_else(LocalError::not_found)?;
    let bytes = tokio::fs::read(&path).await.map_err(|_| LocalError::not_found())?;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let ctype = match ext.as_str() {
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "webp" => "image/webp",
        "png" => "image/png",
        _ => "application/octet-stream",
    };
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ctype.to_string()),
            (header::CONTENT_LENGTH, bytes.len().to_string()),
            (header::CACHE_CONTROL, "private, max-age=31536000, immutable".into()),
            (header::ETAG, format!("\"{}\"", version(&path))),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
            (header::CONTENT_SECURITY_POLICY, "sandbox; default-src 'none'".into()),
        ],
        Body::from(bytes),
    )
        .into_response())
}

// ---------------- 导入、删除 ----------------

/// zip 里的条目名：只许相对路径，不许 `..`、绝对路径、反斜杠
fn entry_path(name: &str) -> Option<PathBuf> {
    if name.contains('\\') || name.starts_with('/') || name.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for seg in name.split('/') {
        match seg {
            "" | "." => continue,
            ".." => return None,
            s if s.contains(':') => return None,
            s => out.push(s),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// 解压到 `dest`：检查个数、单个大小、总大小（按实际解出的字节数算，不信 zip 里写的大小）
fn unzip(zip_path: &Path, dest: &Path) -> Result<(), PackError> {
    let f = std::fs::File::open(zip_path).map_err(|_| perr("bad_zip", ""))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| perr("bad_zip", e.to_string()))?;
    if z.len() > MAX_FILES * 2 {
        return Err(perr("too_many", z.len().to_string()));
    }
    extract(&mut z, "", dest)
}

/// 只解出 `prefix`（空 = 全部，否则以 `/` 结尾）下面的条目，去掉前缀放进 `dest`；限制同 [`unzip`]
pub(crate) fn extract<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, prefix: &str, dest: &Path) -> Result<(), PackError> {
    extract_some(z, prefix, dest, &|_| true)
}

/// 同 [`extract`]，只解 `keep`（去掉前缀后的名字）认的条目
pub(crate) fn extract_some<R: Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, prefix: &str, dest: &Path, keep: &dyn Fn(&str) -> bool) -> Result<(), PackError> {
    let mut total = 0u64;
    let mut files = 0usize;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| perr("bad_zip", e.to_string()))?;
        let full = e.name().to_string();
        let Some(name) = full.strip_prefix(prefix) else { continue };
        let name = name.to_string();
        // macOS 压缩时带的 __MACOSX/、.DS_Store
        if name.starts_with("__MACOSX/") || name.ends_with(".DS_Store") || full.contains("/__MACOSX/") {
            continue;
        }
        if name.is_empty() || !keep(&name) {
            continue;
        }
        let rel = entry_path(&name).ok_or_else(|| perr("bad_name", &name))?;
        if e.is_dir() {
            continue;
        }
        if e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(perr("bad_name", &name));
        }
        files += 1;
        if files > MAX_FILES {
            return Err(perr("too_many", files.to_string()));
        }
        let out = dest.join(&rel);
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| perr("io", e.to_string()))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| perr("io", e.to_string()))?;
        let mut buf = [0u8; 64 * 1024];
        let mut size = 0u64;
        loop {
            let n = e.read(&mut buf).map_err(|e| perr("bad_zip", e.to_string()))?;
            if n == 0 {
                break;
            }
            size += n as u64;
            total += n as u64;
            if size > MAX_FILE {
                return Err(perr("too_big", &name));
            }
            if total > MAX_PACK {
                return Err(perr("too_big", ""));
            }
            w.write_all(&buf[..n]).map_err(|e| perr("io", e.to_string()))?;
        }
    }
    Ok(())
}

/// 解出来的包的根：`pet.json` 在最外层，或者在唯一的一层子目录里
pub(crate) fn pack_root(dir: &Path) -> Option<PathBuf> {
    if dir.join("pet.json").is_file() {
        return Some(dir.to_path_buf());
    }
    let subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name() != "__MACOSX").map(|e| e.path()).collect();
    match subs.as_slice() {
        [one] if one.join("pet.json").is_file() => Some(one.clone()),
        _ => None,
    }
}

/// 导入一个 `.llpet` / `.zip`：解压到临时目录 → 校验 → 换掉 `pets/<id>/`（同 id 覆盖）
pub fn import(home: &Path, zip_path: &Path) -> Result<Pack, PackError> {
    import_with_origin(home, zip_path, None)
}

/// 同 [`import`]，`origin` 写进 `.origin.json`（从哪装的，比如另一台电脑）
pub fn import_with_origin(home: &Path, zip_path: &Path, origin: Option<&Value>) -> Result<Pack, PackError> {
    let base = local_dir(home);
    std::fs::create_dir_all(&base).map_err(|e| perr("io", e.to_string()))?;
    let tmp = tempfile::Builder::new().prefix(".import-").tempdir_in(&base).map_err(|e| perr("io", e.to_string()))?;
    unzip(zip_path, tmp.path())?;
    let root = pack_root(tmp.path()).ok_or_else(|| perr("missing_file", "pet.json"))?;
    install_root(home, &root, origin)
}

/// 从这台电脑上的路径装：`.llpet` / `.zip` 文件，或者解开的宠物文件夹（选文件夹本身或里面的 `pet.json` 都行）。
/// 文件夹是复制进来的，原来的不动。
pub fn import_path(home: &Path, path: &Path) -> Result<Pack, PackError> {
    let meta = std::fs::metadata(path).map_err(|_| perr("missing_file", path.display().to_string()))?;
    let dir = if meta.is_dir() {
        path
    } else if path.file_name().is_some_and(|n| n == "pet.json") {
        path.parent().ok_or_else(|| perr("missing_file", "pet.json"))?
    } else {
        if meta.len() > MAX_PACK {
            return Err(perr("too_big", String::new()));
        }
        return import(home, path);
    };
    let src = pack_root(dir).ok_or_else(|| perr("missing_file", "pet.json"))?;
    let base = local_dir(home);
    std::fs::create_dir_all(&base).map_err(|e| perr("io", e.to_string()))?;
    let tmp = tempfile::Builder::new().prefix(".import-").tempdir_in(&base).map_err(|e| perr("io", e.to_string()))?;
    let root = tmp.path().join("pack");
    let (mut n, mut size) = (0usize, 0u64);
    copy_pack(&src, &root, &mut n, &mut size)?;
    install_root(home, &root, None)
}

/// 复制解开的包：只收普通文件和文件夹（不跟符号链接），跳过隐藏文件；限制同 zip（文件数、总大小）
fn copy_pack(from: &Path, to: &Path, n: &mut usize, size: &mut u64) -> Result<(), PackError> {
    std::fs::create_dir_all(to).map_err(|e| perr("io", e.to_string()))?;
    for e in std::fs::read_dir(from).map_err(|e| perr("io", e.to_string()))?.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') || name == "__MACOSX" {
            continue;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() {
            copy_pack(&e.path(), &to.join(&name), n, size)?;
        } else if t.is_file() {
            *n += 1;
            *size += e.metadata().map(|m| m.len()).unwrap_or(0);
            if *n > MAX_FILES * 2 {
                return Err(perr("too_many", n.to_string()));
            }
            if *size > MAX_PACK {
                return Err(perr("too_big", String::new()));
            }
            std::fs::copy(e.path(), to.join(&name)).map_err(|e| perr("io", e.to_string()))?;
        }
    }
    Ok(())
}

/// 把解好、放在 `pets/` 下临时目录里的包 `root` 换进 `pets/<id>/`（同 id 覆盖）；`origin` 写进 `.origin.json`（从哪装的）
pub(crate) fn install_root(home: &Path, root: &Path, origin: Option<&Value>) -> Result<Pack, PackError> {
    let base = local_dir(home);
    std::fs::create_dir_all(&base).map_err(|e| perr("io", e.to_string()))?;
    let pack = load_dir("local", root)?;
    let _ = std::fs::remove_file(root.join(ORIGIN));
    if let Some(o) = origin {
        std::fs::write(root.join(ORIGIN), o.to_string()).map_err(|e| perr("io", e.to_string()))?;
    }
    let dest = base.join(&pack.id);
    let old = base.join(format!(".old-{}", pack.id));
    let _ = std::fs::remove_dir_all(&old);
    if dest.exists() {
        std::fs::rename(&dest, &old).map_err(|e| perr("io", e.to_string()))?;
    }
    if let Err(e) = std::fs::rename(root, &dest) {
        let _ = std::fs::rename(&old, &dest);
        return Err(perr("io", e.to_string()));
    }
    let _ = std::fs::remove_dir_all(&old);
    forget();
    load_dir("local", &dest)
}

/// 删除一个导入的包（只能删 `local` 的）
pub fn remove(home: &Path, id: &str) -> Result<(), LocalError> {
    if !valid_id(id) {
        return Err(LocalError::invalid("id", "format"));
    }
    let dir = local_dir(home).join(id);
    if !dir.is_dir() {
        return Err(LocalError::not_found());
    }
    std::fs::remove_dir_all(&dir)?;
    forget();
    Ok(())
}

pub fn list_json(home: &Path, fresh: bool) -> Value {
    let packs = all(home, fresh);
    json!({
        "items": packs.iter().map(Pack::summary).collect::<Vec<_>>(),
        "dirs": {
            "local": local_dir(home).display().to_string(),
        },
    })
}

pub fn pack_error(e: PackError) -> LocalError {
    LocalError::new("PET_INVALID").with("reason", format!("pet_{}", e.code)).with("detail", e.detail)
}

// ---------------- 命令行：looklook pet check ----------------

/// 制作时自查（教程第 5 节）：清单、文件，装了 ffmpeg 时再看每段的尺寸、时长、透明通道、首尾帧
pub fn check(dir: &Path) -> i32 {
    let pack = match load_dir("local", dir) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ {}: {}", e.code, e.detail);
            return 1;
        }
    };
    let m = &pack.manifest;
    let name = match &m.name {
        Some(Text::One(s)) => s.clone(),
        Some(Text::Many(mm)) => mm.values().next().cloned().unwrap_or_default(),
        None => String::new(),
    };
    println!("✓ pet.json：{} ({})，{} 段，{}", name, m.id, m.clips.len(), m.renderer);
    let mut warn = 0;
    for s in STATES.iter().filter(|s| !m.states.contains_key(**s) && !matches!(**s, "terminal" | "offline" | "toolDone")) {
        println!("· 没配状态 {s}（会用缺省的代替）");
    }
    if m.actions.click.is_empty() {
        println!("· 没配 actions.click：点它不会有动作");
    }
    if m.renderer != "video" {
        return 0;
    }
    let total: u64 = pack.files.values().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    println!("  合计 {:.1} MB", total as f64 / 1048576.0);
    let no_mov: Vec<&String> = m.clips.keys().filter(|c| !pack.files.contains_key(&format!("anim/{c}.mov"))).collect();
    if !no_mov.is_empty() {
        warn += 1;
        println!("! {} 段没有 .mov，苹果设备上只显示封面：{}", no_mov.len(), no_mov.iter().take(8).map(|s| s.as_str()).collect::<Vec<_>>().join("、"));
    }
    let ffprobe = which("ffprobe");
    let ffmpeg = which("ffmpeg");
    let Some(ffprobe) = ffprobe else {
        println!("· 没找到 ffprobe / ffmpeg，跳过尺寸、透明通道和首尾帧检查");
        return 0;
    };
    let tmp = tempfile::tempdir().ok();
    let idle = m.states.get("idle").and_then(|l| l.first()).map(|p| p.clip.clone()).unwrap_or_default();
    let idle_first = match (&ffmpeg, &tmp) {
        (Some(ff), Some(t)) => frame(ff, &pack.files[&format!("anim/{idle}.webm")], false, &t.path().join("idle0.png")),
        _ => None,
    };
    for (cname, c) in &m.clips {
        let p = &pack.files[&format!("anim/{cname}.webm")];
        let out = std::process::Command::new(&ffprobe)
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_name,width,height:stream_tags=alpha_mode:format=duration", "-of", "json"])
            .arg(p)
            .output();
        let Ok(out) = out else { continue };
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
        let s = &v["streams"][0];
        let (w, h) = (s["width"].as_u64().unwrap_or(0), s["height"].as_u64().unwrap_or(0));
        let alpha = s["tags"]["alpha_mode"].as_str() == Some("1") || s["tags"]["ALPHA_MODE"].as_str() == Some("1");
        let dur: f64 = v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0);
        let mut notes = Vec::new();
        if s["codec_name"].as_str() != Some("vp9") {
            notes.push(format!("编码是 {}，要 vp9", s["codec_name"].as_str().unwrap_or("?")));
        }
        if !alpha {
            notes.push("没有透明通道".to_string());
        }
        if (w, h) != (u64::from(m.stage.width), u64::from(m.stage.height)) {
            notes.push(format!("尺寸 {w}×{h}，清单写的是 {}×{}", m.stage.width, m.stage.height));
        }
        if !(0.5..=20.0).contains(&dur) {
            notes.push(format!("时长 {dur:.1} 秒"));
        }
        let mut ssim = String::new();
        if let (Some(ff), Some(t), Some(idle0)) = (&ffmpeg, &tmp, &idle_first) {
            let a = frame(ff, p, false, &t.path().join("a.png"));
            let b = frame(ff, p, true, &t.path().join("b.png"));
            // 首帧、尾帧都要是标准站姿（转身段的尾帧、睡觉段例外，见 PET_PACK.md §3.2）
            let sleepy = m.actions.sleep.iter().any(|s| [&s.start, &s.looped, &s.end].iter().any(|l| l.iter().any(|p| &p.clip == cname)));
            if let Some(a) = &a {
                // 透明通道写进去了但全是 0（例如 drawbox 画在透明底上）：整段看不见
                if alpha_mean(ff, a).is_some_and(|v| v < 1.0) {
                    notes.push("画面几乎全透明，人物看不见（透明通道是空的）".into());
                }
                let x = similarity(ff, a, idle0);
                ssim.push_str(&format!(" 首帧 {}", x.map(|x| format!("{x:.3}")).unwrap_or_else(|| "?".into())));
                if x.is_some_and(|x| x < 0.95) && !sleepy {
                    notes.push("首帧不像标准站姿".into());
                }
            }
            if let (Some(a), Some(b)) = (&a, &b) {
                if c.looped || (!c.turn && !sleepy) {
                    let x = similarity(ff, b, if c.looped { a } else { idle0 });
                    ssim.push_str(&format!(" 尾帧 {}", x.map(|x| format!("{x:.3}")).unwrap_or_else(|| "?".into())));
                    if x.is_some_and(|x| x < 0.95) {
                        notes.push(if c.looped { "首尾帧不同，循环会跳".into() } else { "尾帧没回到标准站姿".into() });
                    }
                }
            }
        }
        let mark = if notes.is_empty() { "✓" } else { "!" };
        if !notes.is_empty() {
            warn += 1;
        }
        println!("{mark} {cname}：{w}×{h} {dur:.1}s{ssim}{}", if notes.is_empty() { String::new() } else { format!(" — {}", notes.join("；")) });
    }
    if warn > 0 {
        println!("\n{warn} 处要看一下（不影响装上，只是播起来可能不顺）");
    }
    0
}

fn which(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(&exe)).find(|p| p.is_file()))
}

/// 取第一帧 / 最后一帧（用 libvpx 解码才有透明通道）
fn frame(ff: &Path, video: &Path, last: bool, out: &Path) -> Option<PathBuf> {
    let mut cmd = std::process::Command::new(ff);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    if last {
        cmd.args(["-sseof", "-0.2"]);
    }
    cmd.args(["-c:v", "libvpx-vp9", "-i"]).arg(video);
    if last {
        cmd.args(["-update", "1"]);
    } else {
        cmd.args(["-frames:v", "1"]);
    }
    let ok = cmd.arg(out).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().ok()?.success();
    (ok && out.is_file()).then(|| out.to_path_buf())
}

/// 一帧透明通道的平均值（0~255）
fn alpha_mean(ff: &Path, img: &Path) -> Option<f64> {
    let out = std::process::Command::new(ff)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(img)
        .args(["-vf", "alphaextract,signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=-", "-f", "null", "-"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().find_map(|l| l.strip_prefix("lavfi.signalstats.YAVG=")).and_then(|v| v.trim().parse().ok())
}

fn similarity(ff: &Path, a: &Path, b: &Path) -> Option<f64> {
    let out = std::process::Command::new(ff).args(["-hide_banner", "-i"]).arg(a).arg("-i").arg(b).args(["-lavfi", "ssim", "-f", "null", "-"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stderr);
    let i = s.find("All:")?;
    s[i + 4..].split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn jsonc() {
        let v = parse_jsonc("\u{feff}{\n // c\n \"a\": \"x//y\", /* b */ \"b\": [1, 2,],\n \"c\": \"q\\\"/*\",\n}").unwrap();
        assert_eq!(v, json!({ "a": "x//y", "b": [1, 2], "c": "q\"/*" }));
        assert!(parse_jsonc("{\"a\": }").is_err());
    }

    #[test]
    fn names() {
        assert!(valid_id("claude-girl"));
        assert!(!valid_id("Claude"));
        assert!(!valid_id("-x"));
        assert!(valid_name("idle.webm"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("中文"));
        assert_eq!(enc_path("anim/待机.webm"), "anim/%E5%BE%85%E6%9C%BA.webm");
        assert_eq!(entry_path("a/./b"), Some(PathBuf::from("a/b")));
        assert_eq!(entry_path("../x"), None);
        assert_eq!(entry_path("/x"), None);
        assert_eq!(entry_path("a\\b"), None);
    }

    pub(crate) fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// 1×1 的 VP8L WebP 头不好手写，测试用 PNG（只读文件头）
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 6, 0, 0, 0]);
        b
    }

    pub(crate) const MIN: &str = r#"{
      "format": "looklook-pet", "version": 1, "id": "t", "name": "测试",
      "author": { "name": "me" }, "license": { "spdx": "CC0-1.0", "commercial": true },
      "renderer": "video", "stage": { "width": 640, "height": 360 },
      "clips": { "idle": { "loop": true }, "walk": { "move": { "min": 10, "max": 20 } }, "look": { "turn": true } },
      "states": { "idle": ["idle"], "done": [{ "clip": "look", "weight": 2 }] },
      "actions": { "move": ["walk"], "turn": ["look"], "click": ["idle"] },
    }"#;

    pub(crate) fn min_pack(dir: &Path) {
        write(dir, "pet.json", MIN.as_bytes());
        write(dir, "preview.png", &png(10, 10));
        for c in ["idle", "walk", "look"] {
            write(dir, &format!("anim/{c}.webm"), b"\x1a\x45\xdf\xa3");
        }
        write(dir, "anim/idle.mov", b"x");
    }

    #[test]
    fn ours() {
        let t = tempfile::tempdir().unwrap();
        min_pack(t.path());
        let p = load_dir("local", t.path()).unwrap();
        assert_eq!(p.id, "t");
        assert_eq!(p.manifest.states["done"], vec![Pick { clip: "look".into(), weight: 2.0 }]);
        let out = p.manifest_out();
        assert!(out.clips["idle"].webm.as_deref().unwrap().starts_with("anim/idle.webm?v="));
        assert!(out.clips["idle"].mov.is_some() && out.clips["walk"].mov.is_none());
        assert!(out.preview.as_deref().unwrap().starts_with("preview?v="));
        assert_eq!(p.summary()["mov"], json!(false));
        // 引用了不存在的片段
        write(t.path(), "pet.json", MIN.replace("\"done\": [{ \"clip\": \"look\"", "\"done\": [{ \"clip\": \"nope\"").as_bytes());
        assert_eq!(load_dir("local", t.path()).unwrap_err().code, "bad_ref");
        // 转身池里的片段没标 turn
        write(t.path(), "pet.json", MIN.replace("\"look\": { \"turn\": true }", "\"look\": {}").as_bytes());
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("bad_clip", "look"));
        // 没写许可
        write(t.path(), "pet.json", MIN.replace("\"commercial\": true", "\"x\": 1").as_bytes());
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("field", "license.commercial"));
        // 拼错的状态名
        write(t.path(), "pet.json", MIN.replace("\"done\":", "\"dnoe\":").as_bytes());
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("field", "states.dnoe"));
        // 缺片段文件
        write(t.path(), "pet.json", MIN.as_bytes());
        std::fs::remove_file(t.path().join("anim/walk.webm")).unwrap();
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("missing_file", "anim/walk.webm"));
    }

    #[test]
    fn sprite() {
        let t = tempfile::tempdir().unwrap();
        let guide = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/PET_PACK_GUIDE.md")).unwrap();
        // 教程第 8 节的精灵图示例原样能装
        let i = guide.find("```jsonc\n{\n  \"format\": \"looklook-pet\", \"version\": 1, \"id\": \"my-sprite\"").expect("guide sprite example");
        let body = &guide[i + 9..];
        let body = &body[..body.find("```").unwrap()];
        write(t.path(), "pet.json", body.as_bytes());
        write(t.path(), "preview.png", &png(10, 10));
        write(t.path(), "spritesheet.webp", &png(1536, 1872));
        let p = load_dir("local", t.path()).unwrap();
        assert_eq!(p.manifest.renderer, "sprite");
        assert_eq!((p.manifest.stage.width, p.manifest.stage.height), (192, 208));
        assert_eq!(p.manifest.clips["review"].row, Some(8));
        assert!(p.manifest_out().sprite.unwrap().image.starts_with("sprite?v="));
        // 太矮，放不下 9 行
        write(t.path(), "spritesheet.webp", &png(1536, 1000));
        assert_eq!(load_dir("local", t.path()).unwrap_err().code, "bad_clip");
        // 别家格式不认
        write(t.path(), "pet.json", br#"{"id":"x","displayName":"X","spritesheetPath":"spritesheet.webp"}"#);
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("format", ""));
        write(t.path(), "pet.json", br#"{"format":"dsh-pet","id":"x"}"#);
        assert_eq!(load_dir("local", t.path()).unwrap_err(), perr("format", "dsh-pet"));
    }

    #[test]
    fn webp_size() {
        let t = tempfile::tempdir().unwrap();
        // VP8X：宽高各减 1 存 24 位
        let mut b = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\x10\0\0\0".to_vec();
        b.extend([0xff, 0x05, 0x00, 0x4f, 0x07, 0x00]);
        write(t.path(), "a.webp", &b);
        assert_eq!(image_size(&t.path().join("a.webp")), Some((1536, 1872)));
    }

    pub(crate) fn zip_dir(src: &Path, out: &Path, prefix: &str) {
        let f = std::fs::File::create(out).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for e in walk(src) {
            let rel = e.strip_prefix(src).unwrap().to_string_lossy().replace('\\', "/");
            z.start_file(format!("{prefix}{rel}"), zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(&std::fs::read(&e).unwrap()).unwrap();
        }
        z.finish().unwrap();
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                out.extend(walk(&e.path()));
            } else {
                out.push(e.path());
            }
        }
        out
    }

    #[test]
    fn import_and_remove() {
        let home = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        min_pack(src.path());
        let z = home.path().join("t.llpet");
        // 包在一层子目录里也行
        zip_dir(src.path(), &z, "my-pet/");
        let p = import(home.path(), &z).unwrap();
        assert_eq!(p.dir, home.path().join("pets/t"));
        assert!(home.path().join("pets/t/anim/idle.webm").is_file());
        // 再导入一次 = 覆盖
        import(home.path(), &z).unwrap();
        let names: Vec<String> = std::fs::read_dir(home.path().join("pets")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["t"], "临时目录都清掉了");
        // 带 .. 的条目
        let f = std::fs::File::create(&z).unwrap();
        let mut w = zip::ZipWriter::new(f);
        w.start_file("../evil", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        assert_eq!(import(home.path(), &z).unwrap_err().code, "bad_name");
        assert!(!home.path().join("evil").exists());
        remove(home.path(), "t").unwrap();
        assert!(!home.path().join("pets/t").exists());
        assert!(remove(home.path(), "../x").is_err());
    }

    #[test]
    fn settings_pick() {
        let mut s = PetSettings { default: Some("local:a".into()), ..PetSettings::default() };
        s.by_agent.insert("codex".into(), None);
        s.by_agent.insert("dsh".into(), Some("local:b".into()));
        assert_eq!(s.pick("claude"), Some("local:a"));
        assert_eq!(s.pick("codex"), None);
        assert_eq!(s.pick("dsh"), Some("local:b"));
        s.enabled = false;
        assert_eq!(s.pick("dsh"), None);
    }
}
