//! 对话宠物（docs/PET_PACK.md）：找宠物包、把 Codex 宠物和 dsh-pet 换算成同一种清单、提供文件、导入和删除。
//!
//! - 三个来源：`local`（`{LOOKLOOK_HOME}/pets/`，导入的放这里）、`codex`（`$CODEX_HOME/pets/`）、`dsh`（`$DSH_HOME/dsh-pet/`），后两个只读；
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
const STATES: &[&str] = &["idle", "thinking", "tool", "toolDone", "waiting", "terminal", "done", "error", "offline"];
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

fn picks(names: &[&str]) -> Vec<Pick> {
    names.iter().map(|n| Pick { clip: (*n).into(), weight: 1.0 }).collect()
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

/// 路径里每一段做百分号编码（dsh-pet 的片段名是中文）
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

/// dsh-pet 的名字（中文）：只要求能当一段路径
fn safe_segment(n: &str) -> bool {
    !n.is_empty() && n.len() <= 120 && n != "." && n != ".." && !n.chars().any(|c| matches!(c, '/' | '\\' | '\0' | ':' | '?' | '#' | '%')) && !n.starts_with('.')
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
    validate(&m, true)?;
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

/// 清单的字段和引用都对（§3）。`strict`：本格式要求的署名、许可、文件名规则（换算来的包不查这些）
pub fn validate(m: &Manifest, strict: bool) -> Result<(), PackError> {
    if strict {
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
        if strict && !valid_name(name) {
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
    for (list, what) in [(&a.click, "click"), (&a.drag, "drag"), (&a.land, "land"), (&a.turn, "turn"), (&a.walk, "move")] {
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

// ---------------- Codex 宠物（§5） ----------------

/// Codex 宠物：8 列 × 9 行（或 11 行的 v2），每格 192×208，行的含义固定
fn load_codex(source: &'static str, dir: &Path, v: Value) -> Result<Pack, PackError> {
    let image = v["spritesheetPath"].as_str().unwrap_or("spritesheet.webp").to_string();
    if !safe_segment(&image) {
        return Err(perr("bad_name", image));
    }
    let p = small_file(dir, &image)?.ok_or_else(|| perr("missing_file", &image))?;
    let raw_id = v["id"].as_str().map(str::to_string).or_else(|| dir.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
    let id: String = raw_id.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).take(40).collect();
    let id = id.trim_matches('-').to_string();
    if !valid_id(&id) {
        return Err(perr("field", "id"));
    }
    let name = v["displayName"].as_str().or(v["name"].as_str()).filter(|s| !s.trim().is_empty()).unwrap_or(&raw_id).to_string();
    let clip = |row: u32, frames: u32, fps: f64| Clip { row: Some(row), frames: Some(frames), fps: Some(fps), ..Clip::default() };
    let mut clips = BTreeMap::new();
    clips.insert("idle".to_string(), Clip { looped: true, ..clip(0, 6, 6.0) });
    for (name, row, dir) in [("run-right", 1, "right"), ("run-left", 2, "left")] {
        clips.insert(name.into(), Clip { looped: true, mirror: false, dir: Some(dir.into()), walk: Some(Walk::default()), ..clip(row, 8, 10.0) });
    }
    clips.insert("wave".into(), clip(3, 4, 6.0));
    clips.insert("jump".into(), clip(4, 5, 8.0));
    clips.insert("fail".into(), clip(5, 8, 8.0));
    clips.insert("wait".into(), Clip { looped: true, ..clip(6, 6, 6.0) });
    clips.insert("work".into(), Clip { looped: true, ..clip(7, 6, 8.0) });
    clips.insert("review".into(), Clip { looped: true, ..clip(8, 6, 6.0) });
    let mut states = BTreeMap::new();
    for (k, c) in [("idle", "idle"), ("thinking", "review"), ("tool", "work"), ("waiting", "wait"), ("done", "jump"), ("error", "fail")] {
        states.insert(k.to_string(), picks(&[c]));
    }
    let m = Manifest {
        format: "codex-pet".into(),
        version: 1,
        id: id.clone(),
        name: Some(Text::One(name)),
        description: v["description"].as_str().map(|s| Text::One(s.into())),
        renderer: "sprite".into(),
        stage: Stage { width: 192, height: 208, ground: 0.95, ..Stage::default() },
        sprite: Some(Sprite { image: image.clone(), cell: [192, 208], columns: 8 }),
        clips,
        states,
        actions: Actions { click: picks(&["wave", "jump"]), walk: picks(&["run-right", "run-left"]), ..Actions::default() },
        behavior: Behavior { idle: 70.0, turn: 0.0, walk: 15.0, sleep_after: 0 },
        ..Manifest::default()
    };
    validate(&m, false)?;
    check_sheet(&m, &p, &image)?;
    let mut files = BTreeMap::new();
    files.insert("sprite".to_string(), p);
    Ok(Pack { source, id, dir: dir.to_path_buf(), manifest: m, files, error: None })
}

/// 一个文件夹里的包（本格式，或 Codex 宠物）
pub fn load_dir(source: &'static str, dir: &Path) -> Result<Pack, PackError> {
    let v = read_json(&dir.join("pet.json"))?;
    if v["format"] == "looklook-pet" {
        load_ours(source, dir, v)
    } else if v.get("spritesheetPath").is_some() || dir.join("spritesheet.webp").is_file() {
        load_codex(source, dir, v)
    } else {
        Err(perr("field", "format"))
    }
}

// ---------------- dsh-pet（§5） ----------------

/// dsh-pet 的动画距离以宽 462 px 的宠物为准
const DSH_BASE: f64 = 462.0;

struct DshDirs {
    webm: Vec<PathBuf>,
    mov: Vec<PathBuf>,
}

fn str_list(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// 一份 dsh-pet 配置（已合并好）换算成清单
fn convert_dsh(source: &'static str, id: &str, dir: &Path, name: String, cfg: &Value, dirs: &DshDirs, license: Option<License>, credits: Vec<Credit>) -> Result<Pack, PackError> {
    let anim = &cfg["animations"];
    let mut clips: BTreeMap<String, Clip> = BTreeMap::new();
    let mut files = BTreeMap::new();
    // 登记一段：素材在才算
    let mut add = |n: &str, f: &dyn Fn(&mut Clip)| -> bool {
        if !safe_segment(n) {
            return false;
        }
        if !clips.contains_key(n) {
            let Some(webm) = dirs.webm.iter().map(|d| d.join(format!("{n}.webm"))).find(|p| p.is_file()) else { return false };
            files.insert(format!("anim/{n}.webm"), webm);
            if let Some(mov) = dirs.mov.iter().map(|d| d.join(format!("{n}.mov"))).find(|p| p.is_file()) {
                files.insert(format!("anim/{n}.mov"), mov);
            }
            clips.insert(n.to_string(), Clip::default());
        }
        f(clips.get_mut(n).expect("clip"));
        true
    };
    let none = |_: &mut Clip| {};
    let pool = |v: &Value, f: &dyn Fn(&mut Clip), add: &mut dyn FnMut(&str, &dyn Fn(&mut Clip)) -> bool| -> Vec<Pick> {
        str_list(v).into_iter().filter(|n| add(n, f)).map(|clip| Pick { clip, weight: 1.0 }).collect()
    };
    let idle = pool(&anim["idle"], &|c: &mut Clip| c.looped = true, &mut add);
    if idle.is_empty() {
        return Err(perr("field", "states.idle"));
    }
    let mut states = BTreeMap::new();
    states.insert("idle".to_string(), idle);
    let ws = anim["events"]["workStatus"].as_array().cloned().unwrap_or_default();
    for (i, k) in ["thinking", "tool", "toolDone", "waiting", "done", "error"].iter().enumerate() {
        let l = pool(ws.get(i).unwrap_or(&Value::Null), &none, &mut add);
        if !l.is_empty() {
            states.insert(k.to_string(), l);
        }
    }
    let mut actions = Actions {
        click: pool(&anim["clicks"], &none, &mut add),
        drag: pool(&anim["drag"], &|c: &mut Clip| c.looped = true, &mut add),
        turn: pool(&anim["turn"], &|c: &mut Clip| c.turn = true, &mut add),
        ..Actions::default()
    };
    let k = 640.0 / DSH_BASE;
    let def = &anim["moves"]["default"];
    for a in anim["moves"]["actions"].as_array().cloned().unwrap_or_default() {
        let Some(n) = a["name"].as_str() else { continue };
        let num = |key: &str, d: f64| a["params"][key].as_f64().or(def[key].as_f64()).unwrap_or(d);
        let w = Walk { min: num("minDist", 60.0) * k, max: num("maxDist", 240.0) * k, lead: num("leadSec", 2.0), tail: num("tailSec", 2.0), ..Walk::default() };
        if add(n, &|c: &mut Clip| c.walk = Some(w.clone())) {
            actions.walk.push(Pick { clip: n.into(), weight: 1.0 });
        }
    }
    for g in anim["categories"].as_array().cloned().unwrap_or_default() {
        let no_mirror = g["noMirror"].as_bool().unwrap_or(false);
        let l = pool(&g["actions"], &|c: &mut Clip| if no_mirror { c.mirror = false }, &mut add);
        let weight = g["weight"].as_f64().unwrap_or(10.0);
        if !l.is_empty() && weight > 0.0 {
            actions.random.push(Group { id: g["id"].as_str().unwrap_or("").to_string(), weight, clips: l });
        }
    }
    for s in anim["sleep"].as_array().cloned().unwrap_or_default() {
        let set = SleepSet {
            start: pool(&s["start"], &none, &mut add),
            looped: pool(&s["loop"], &|c: &mut Clip| c.looped = true, &mut add),
            end: pool(&s["end"], &none, &mut add),
        };
        if !set.start.is_empty() && !set.looped.is_empty() && !set.end.is_empty() {
            actions.sleep.push(set);
        }
    }
    let w = &cfg["animationWeights"];
    let behavior = Behavior {
        idle: w["idle"].as_f64().unwrap_or(10.0),
        turn: w["turn"].as_f64().unwrap_or(5.0),
        walk: w["move"].as_f64().unwrap_or(5.0),
        sleep_after: if actions.sleep.is_empty() { 0 } else { 600 },
    };
    let mut zh: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(t) = cfg["workStatusTexts"].as_array() {
        for (i, k) in ["thinking", "tool", "toolDone", "waiting", "done", "error"].iter().enumerate() {
            let l = t.get(i).map(str_list).unwrap_or_default();
            if !l.is_empty() {
                zh.insert(k.to_string(), l);
            }
        }
    }
    for (src, k) in [("sleepStartTexts", "sleepStart"), ("sleepEndTexts", "sleepEnd"), ("sleepRefuseTexts", "wake")] {
        let l = str_list(&cfg[src]);
        if !l.is_empty() {
            zh.insert(k.into(), l);
        }
    }
    let mut lines = BTreeMap::new();
    if !zh.is_empty() {
        lines.insert("zh-CN".to_string(), zh);
    }
    let m = Manifest {
        format: "dsh-pet".into(),
        version: 1,
        id: id.to_string(),
        name: Some(Text::One(name)),
        license,
        credits,
        agents: vec!["dsh".into()],
        renderer: "video".into(),
        stage: Stage { width: 640, height: 360, ..Stage::default() },
        clips,
        states,
        actions,
        behavior,
        lines,
        ..Manifest::default()
    };
    validate(&m, false)?;
    Ok(Pack { source, id: id.to_string(), dir: dir.to_path_buf(), manifest: m, files, error: None })
}

/// 装在 DSH profile 里的 dsh-pet 包（带默认素材和 `assets/config.jsonc`）：`$DSH_HOME` 下三层以内的 `node_modules/dsh-pet`
fn find_dsh_package(dsh: &Path) -> Option<PathBuf> {
    let mut level = vec![dsh.to_path_buf()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for d in &level {
            let cand = d.join("node_modules").join("dsh-pet");
            if cand.join("assets").join("config.jsonc").is_file() {
                return Some(cand);
            }
            let Ok(rd) = std::fs::read_dir(d) else { continue };
            for e in rd.flatten().take(64) {
                let n = e.file_name();
                let n = n.to_string_lossy();
                if n == "node_modules" || n.starts_with('.') || n == "electron" {
                    continue;
                }
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(e.path());
                }
            }
        }
        level = next;
    }
    None
}

fn dsh_credits() -> Vec<Credit> {
    vec![
        Credit { role: "原角色（溟月）".into(), name: "上善无形".into(), url: Some("https://space.bilibili.com/4456176".into()) },
        Credit { role: "DeepSeek 女仆设计".into(), name: "ZipZipPipe".into(), url: Some("https://space.bilibili.com/4168597".into()) },
        Credit { role: "动画".into(), name: "dsh-pet".into(), url: Some("https://github.com/PC2005-cloud/dsh-pet".into()) },
    ]
}

/// `$DSH_HOME/dsh-pet/` 里的宠物：默认宠物 `main` 和 `pet/` 下的各个种类
fn load_dsh(dsh: &Path) -> Vec<Pack> {
    let root = dsh.join("dsh-pet");
    let pkg = find_dsh_package(dsh);
    let mut out = Vec::new();
    // 默认宠物：包内默认配置，用户层 main-config.jsonc 的顶层字段整段替换
    let base = pkg.as_ref().and_then(|p| read_json(&p.join("assets").join("config.jsonc")).ok());
    let user = read_json(&root.join("main-config.jsonc")).ok();
    if base.is_some() || (user.is_some() && root.join("main-animation").is_dir()) {
        let mut cfg = base.unwrap_or_else(|| json!({}));
        if let (Some(Value::Object(u)), Value::Object(c)) = (user, &mut cfg) {
            for (k, v) in u {
                c.insert(k, v);
            }
        }
        let dirs = DshDirs {
            webm: [Some(root.join("main-animation").join("webm")), pkg.as_ref().map(|p| p.join("assets").join("webm"))].into_iter().flatten().collect(),
            mov: vec![root.join("main-animation").join("mov")],
        };
        let name = cfg["pets"][0]["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("dsh-pet").to_string();
        let license = License {
            spdx: "LicenseRef-dsh-pet".into(),
            commercial: Some(false),
            url: Some("https://github.com/PC2005-cloud/dsh-pet#许可".into()),
            note: Some("dsh-pet 素材允许开源使用、禁止商用；形象 CC BY-NC-SA 4.0".into()),
        };
        let dir = pkg.clone().unwrap_or_else(|| root.clone());
        out.push(convert_dsh("dsh", "main", &dir, name, &cfg, &dirs, Some(license), dsh_credits()).unwrap_or_else(|e| Pack::broken("dsh", &dir, e)));
    }
    let Ok(rd) = std::fs::read_dir(root.join("pet")) else { return out };
    let mut kinds: Vec<_> = rd.flatten().filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix("-config.json")).map(str::to_string)).collect();
    kinds.sort();
    for kind in kinds {
        let anim = root.join("pet").join(format!("{kind}-animation"));
        let cfg_path = root.join("pet").join(format!("{kind}-config.json"));
        let pack = (|| {
            if !safe_segment(&kind) {
                return Err(perr("bad_name", &kind));
            }
            let cfg = read_json(&cfg_path)?;
            if !anim.is_dir() {
                return Err(perr("missing_file", format!("{kind}-animation/")));
            }
            let name = cfg["pets"][0]["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(&kind).to_string();
            let dirs = DshDirs { webm: vec![anim.clone()], mov: vec![anim.join("mov")] };
            convert_dsh("dsh", &kind, &anim, name, &cfg, &dirs, None, Vec::new())
        })();
        out.push(pack.unwrap_or_else(|e| {
            let mut p = Pack::broken("dsh", &anim, e);
            p.id = kind.clone();
            p
        }));
    }
    out
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

/// 所有来源的宠物（同一来源里 id 重复的只留第一个）
pub fn scan(home: &Path) -> Vec<Pack> {
    let mut all = scan_dir("local", &local_dir(home));
    all.extend(scan_dir("codex", &crate::agents::codex_home().join("pets")));
    all.extend(load_dsh(&crate::agents::dsh_home()));
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
}

impl Default for PetSettings {
    fn default() -> Self {
        Self { enabled: true, by_agent: BTreeMap::new(), default: None, size: 1.0, lines: true }
    }
}

impl PetSettings {
    pub fn load(store: &crate::store::Store) -> Self {
        store.get(SETTINGS_KEY).ok().flatten().unwrap_or_default()
    }

    pub fn save(mut self, store: &crate::store::Store) -> Result<Self, LocalError> {
        self.size = if self.size.is_finite() { self.size.clamp(0.5, 2.0) } else { 1.0 };
        self.by_agent.retain(|k, _| AGENTS.contains(&k.as_str()));
        let ok = |k: &Option<String>| k.as_ref().is_none_or(|k| k.len() <= 200 && k.contains(':'));
        if !self.by_agent.values().all(ok) || !ok(&self.default) {
            return Err(LocalError::invalid("pet", "format"));
        }
        store.set(SETTINGS_KEY, &self)?;
        Ok(self)
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
    })
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
        if name.is_empty() {
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
    let base = local_dir(home);
    std::fs::create_dir_all(&base).map_err(|e| perr("io", e.to_string()))?;
    let tmp = tempfile::Builder::new().prefix(".import-").tempdir_in(&base).map_err(|e| perr("io", e.to_string()))?;
    unzip(zip_path, tmp.path())?;
    let root = pack_root(tmp.path()).ok_or_else(|| perr("missing_file", "pet.json"))?;
    install_root(home, &root, None)
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
            "codex": crate::agents::codex_home().join("pets").display().to_string(),
            "dsh": crate::agents::dsh_home().join("dsh-pet").display().to_string(),
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
        assert!(safe_segment("待机呼吸休闲"));
        assert!(!safe_segment("../x") && !safe_segment("a/b") && !safe_segment(".."));
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
    fn codex() {
        let t = tempfile::tempdir().unwrap();
        write(t.path(), "pet.json", br#"{"id":"Claude","displayName":"Claude","spritesheetPath":"spritesheet.webp"}"#);
        write(t.path(), "spritesheet.webp", &png(1536, 1872));
        let p = load_dir("codex", t.path()).unwrap();
        assert_eq!(p.id, "claude");
        assert_eq!(p.manifest.renderer, "sprite");
        assert_eq!(p.manifest.clips["review"].row, Some(8));
        assert_eq!(p.manifest.states["thinking"][0].clip, "review");
        assert!(p.manifest_out().sprite.unwrap().image.starts_with("sprite?v="));
        // 太矮，放不下 9 行
        write(t.path(), "spritesheet.webp", &png(1536, 1000));
        assert_eq!(load_dir("codex", t.path()).unwrap_err().code, "bad_clip");
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

    #[test]
    fn dsh() {
        let t = tempfile::tempdir().unwrap();
        let dsh = t.path();
        let pkg = dsh.join("profiles/web/node_modules/dsh-pet");
        write(&pkg, "assets/config.jsonc", r#"{
          // 包内默认
          "pets": [{ "name": "蓝毛小女仆", "id": "main" }],
          "workStatusTexts": [["想想"], ["干活"]],
          "sleepStartTexts": ["困了"],
          "animations": {
            "idle": ["待机"], "turn": ["东张西望"], "drag": ["悬空"], "clicks": ["点击-开心", "点击-缺文件"],
            "moves": { "default": { "minDist": 462, "maxDist": 924, "leadSec": 2, "tailSec": 2 }, "actions": [{ "name": "螃蟹走路", "params": { "maxDist": 462 } }] },
            "categories": [{ "id": "文字", "weight": 10, "noMirror": true, "actions": ["深度思考"] }],
            "events": { "workStatus": ["思考", ["忙碌", "清点"], "清点"] },
            "sleep": [{ "start": ["睡-开始"], "loop": ["睡-循环"], "end": ["睡-起来"] }]
          },
          "animationWeights": { "idle": 10, "turn": 5, "move": 5 },
        }"#.as_bytes());
        for n in ["待机", "东张西望", "悬空", "点击-开心", "螃蟹走路", "深度思考", "思考", "忙碌", "清点", "睡-开始", "睡-循环", "睡-起来"] {
            write(&pkg, &format!("assets/webm/{n}.webm"), b"x");
        }
        // 用户层覆盖名字；mov 在用户目录
        write(dsh, "dsh-pet/main-config.jsonc", r#"{ "pets": [{ "name": "小鲸", "id": "main" }] }"#.as_bytes());
        write(dsh, "dsh-pet/main-animation/mov/待机.mov", b"x");
        // 一个种类
        write(dsh, "dsh-pet/pet/pig-config.json", br#"{ "animations": { "idle": ["pig-idle"] } }"#);
        write(dsh, "dsh-pet/pet/pig-animation/pig-idle.webm", b"x");
        // 一个坏种类：没有素材目录
        write(dsh, "dsh-pet/pet/bad-config.json", br#"{ "animations": { "idle": ["x"] } }"#);
        let packs = load_dsh(dsh);
        assert_eq!(packs.len(), 3);
        let main = &packs[0];
        assert_eq!(main.id, "main");
        assert!(main.error.is_none(), "{:?}", main.error);
        let m = &main.manifest;
        assert!(matches!(&m.name, Some(Text::One(n)) if n == "小鲸"));
        assert_eq!(m.states["tool"].iter().map(|p| p.clip.as_str()).collect::<Vec<_>>(), ["忙碌", "清点"]);
        assert_eq!(m.states["toolDone"][0].clip, "清点");
        assert!(!m.states.contains_key("waiting"));
        assert_eq!(m.actions.click.len(), 1, "缺文件的片段不登记");
        let w = m.clips["螃蟹走路"].walk.as_ref().unwrap();
        assert_eq!((w.min, w.max, w.lead), (640.0, 640.0, 2.0));
        assert!(!m.clips["深度思考"].mirror && m.clips["东张西望"].turn);
        assert_eq!(m.actions.sleep.len(), 1);
        assert_eq!(m.lines["zh-CN"]["tool"], ["干活"]);
        assert_eq!(m.lines["zh-CN"]["sleepStart"], ["困了"]);
        assert_eq!(m.license.as_ref().unwrap().commercial, Some(false));
        assert!(main.files.contains_key("anim/待机.mov"));
        assert_eq!(packs[2].id, "pig");
        assert!(packs[2].error.is_none());
        assert_eq!(packs[1].id, "bad");
        assert_eq!(packs[1].error.as_ref().unwrap().code, "missing_file");
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
        s.by_agent.insert("dsh".into(), Some("dsh:main".into()));
        assert_eq!(s.pick("claude"), Some("local:a"));
        assert_eq!(s.pick("codex"), None);
        assert_eq!(s.pick("dsh"), Some("dsh:main"));
        s.enabled = false;
        assert_eq!(s.pick("dsh"), None);
    }
}
