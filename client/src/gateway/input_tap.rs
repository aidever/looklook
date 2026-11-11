//! 终端 WebSocket 上看用户有没有在输入（不改数据，只看浏览器发给 ttyd 的那一半）。
//!
//! 会话列表按“用户最近一次输入”排（见 `Instances::note_input`）：本机、局域网、远程（WebRTC 也是接到同一个路由上）
//! 打开的终端都经过网关，这里一处就都能看到。浏览器发来的 WebSocket 帧是加了掩码的，按帧头解出负载的开头几十个字节：
//! ttyd 的输入消息以 `'0'` 开头；终端自己回的（焦点进出、鼠标、查询的应答）不算输入，滚轮看历史也不算。

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 看到用户输入时调用（调用方自己限频）
pub type OnInput = Arc<dyn Fn() + Send + Sync>;

/// 每条消息最多看负载的前这么多字节：更长的是粘贴，直接算输入
const PEEK: usize = 64;

/// 包一层读：读到的字节原样交出去，顺便喂给 [`WsTap`]
pub struct Tap<T> {
    inner: T,
    ws: WsTap,
    on: OnInput,
}

impl<T> Tap<T> {
    /// `deflate`：握手谈好了 permessage-deflate（浏览器默认会要，ttyd 会答应）时是 Some(每条消息各自压缩
    /// ——`client_no_context_takeover`)
    pub fn new(inner: T, on: OnInput, deflate: Option<bool>) -> Self {
        Self { inner, ws: WsTap::new(deflate), on }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Tap<T> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let r = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            if this.ws.feed(&buf.filled()[before..]) {
                (this.on)();
            }
        }
        r
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Tap<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(self: Pin<&mut Self>, cx: &mut Context<'_>, bufs: &[io::IoSlice<'_>]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// 压缩的消息最多攒这么多字节再解（再大的是大段粘贴，直接算输入；之后解压状态对不上，不再看压缩的消息）
const ZMAX: usize = 1 << 20;

/// permessage-deflate（RFC 7692）：浏览器发来的消息解压
struct Inflate {
    d: flate2::Decompress,
    /// 每条消息各自压缩（不沿用上一条的字典）
    reset: bool,
    /// 解坏了（或者跳过了一条）：之后的压缩消息看不懂了
    broken: bool,
}

impl Inflate {
    /// 解一条消息，返回 (开头 PEEK 个字节, 总长)
    fn message(&mut self, z: &[u8]) -> Option<(Vec<u8>, u64)> {
        if self.broken {
            return None;
        }
        let r = self.run(z);
        if r.is_none() {
            self.broken = true;
        } else if self.reset {
            self.d.reset(false);
        }
        r
    }

    fn run(&mut self, z: &[u8]) -> Option<(Vec<u8>, u64)> {
        let input = [z, &[0, 0, 0xff, 0xff]].concat();
        let mut out = Vec::new();
        let mut total = 0u64;
        let mut buf = [0u8; 8192];
        let mut off = 0;
        loop {
            let (i0, o0) = (self.d.total_in(), self.d.total_out());
            let st = self.d.decompress(&input[off..], &mut buf, flate2::FlushDecompress::Sync).ok()?;
            let used = (self.d.total_in() - i0) as usize;
            let made = (self.d.total_out() - o0) as usize;
            off += used;
            total += made as u64;
            let room = PEEK.saturating_sub(out.len());
            out.extend_from_slice(&buf[..made.min(room)]);
            if st == flate2::Status::StreamEnd || (off >= input.len() && made < buf.len()) || (used == 0 && made == 0) {
                break;
            }
        }
        Some((out, total))
    }
}

/// 一点一点喂进来的 WebSocket 帧（客户端发出的，带掩码）
#[derive(Default)]
pub struct WsTap {
    /// 还没收全的帧头
    head: Vec<u8>,
    /// 正在读负载
    body: bool,
    /// 这一帧的负载还剩多少字节、读到第几个（掩码按帧内位置）
    left: u64,
    pos: u64,
    mask: [u8; 4],
    /// 这一帧是文本 / 二进制帧或它们的续帧（控制帧不看）
    data: bool,
    /// 这一帧是一条消息的最后一帧
    fin: bool,
    /// 这条消息：压缩了、一共多长、负载开头（去掉掩码）、压缩的全部负载
    zipped: bool,
    len: u64,
    peek: Vec<u8>,
    z: Vec<u8>,
    inflate: Option<Inflate>,
}

impl WsTap {
    pub fn new(deflate: Option<bool>) -> Self {
        let inflate = deflate.map(|reset| Inflate { d: flate2::Decompress::new(false), reset, broken: false });
        Self { inflate, ..Default::default() }
    }

    /// 喂一段字节，返回这段里有没有收全一条用户输入
    pub fn feed(&mut self, mut bytes: &[u8]) -> bool {
        let mut hit = false;
        while !bytes.is_empty() {
            if !self.body {
                self.head.push(bytes[0]);
                bytes = &bytes[1..];
                if let Some(n) = head_len(&self.head) {
                    if self.head.len() == n {
                        self.start();
                        if self.left == 0 {
                            hit |= self.end();
                        }
                    }
                }
                continue;
            }
            let n = (self.left.min(bytes.len() as u64)) as usize;
            if self.data {
                let zipped = self.zipped;
                for &b in &bytes[..n] {
                    let v = b ^ self.mask[(self.pos % 4) as usize];
                    self.pos += 1;
                    if zipped {
                        if self.z.len() <= ZMAX {
                            self.z.push(v);
                        }
                    } else if self.peek.len() < PEEK {
                        self.peek.push(v);
                    } else {
                        break;
                    }
                }
            }
            self.left -= n as u64;
            bytes = &bytes[n..];
            if self.left == 0 {
                hit |= self.end();
            }
        }
        hit
    }

    fn start(&mut self) {
        let h = &self.head;
        let op = h[0] & 0x0f;
        let masked = h[1] & 0x80 != 0;
        let (len, at) = match h[1] & 0x7f {
            126 => (u16::from_be_bytes([h[2], h[3]]) as u64, 4),
            127 => (u64::from_be_bytes(h[2..10].try_into().unwrap_or_default()), 10),
            l => (l as u64, 2),
        };
        self.mask = if masked { [h[at], h[at + 1], h[at + 2], h[at + 3]] } else { [0; 4] };
        self.left = len;
        self.pos = 0;
        self.fin = h[0] & 0x80 != 0;
        self.data = op <= 2;
        if op == 1 || op == 2 {
            // 一条新消息：RSV1 表示压缩了
            self.zipped = self.inflate.is_some() && h[0] & 0x40 != 0;
            self.len = 0;
            self.peek.clear();
            self.z.clear();
        }
        if self.data {
            self.len += len;
        }
        self.head.clear();
        self.body = true;
    }

    fn end(&mut self) -> bool {
        self.body = false;
        if !self.data || !self.fin {
            return false;
        }
        let hit = if self.zipped {
            if self.z.len() > ZMAX {
                // 太大：是粘贴；解压状态从此对不上
                if let Some(i) = &mut self.inflate {
                    i.broken = true;
                }
                true
            } else {
                match self.inflate.as_mut().and_then(|i| i.message(&self.z)) {
                    Some((peek, len)) => is_user_input(&peek, len),
                    None => false,
                }
            }
        } else {
            is_user_input(&self.peek, self.len)
        };
        self.peek.clear();
        self.z.clear();
        hit
    }
}

/// 帧头一共多长（还不够判断时 None）
fn head_len(h: &[u8]) -> Option<usize> {
    if h.len() < 2 {
        return None;
    }
    let ext = match h[1] & 0x7f {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    let mask = if h[1] & 0x80 != 0 { 4 } else { 0 };
    Some(2 + ext + mask)
}

/// ttyd 的一条消息是不是用户输入：`'0'` 开头（INPUT），内容不全是终端自己回的
fn is_user_input(peek: &[u8], len: u64) -> bool {
    let Some((&b'0', data)) = peek.split_first() else { return false };
    if len as usize > PEEK {
        return true;
    }
    !data.is_empty() && !only_auto(data)
}

/// 整段都是终端自动发的：焦点进出（`ESC [ I` / `ESC [ O`）、鼠标（SGR `ESC [ < … M/m`、X10 `ESC [ M` + 3 字节）、
/// 设备属性等应答（`ESC [ ? … c`、`ESC [ > … c`、`ESC [ ? … $ y`）、OSC / DCS 应答
fn only_auto(mut d: &[u8]) -> bool {
    while !d.is_empty() {
        let Some(n) = auto_seq(d) else { return false };
        d = &d[n..];
    }
    true
}

/// 开头这一段是不是一个自动发的序列，是的话返回它的长度
fn auto_seq(d: &[u8]) -> Option<usize> {
    if d.len() < 3 || d[0] != 0x1b {
        return None;
    }
    match d[1] {
        b'[' => {
            match d[2] {
                b'I' | b'O' => return Some(3),
                b'M' => return (d.len() >= 6).then_some(6),
                _ => {}
            }
            // CSI：参数和中间字节，到结尾字节为止
            let lead = d[2];
            let mut i = 2;
            while i < d.len() && (0x20..=0x3f).contains(&d[i]) {
                i += 1;
            }
            let fin = *d.get(i)?;
            match (lead, fin) {
                (b'<', b'M' | b'm') => Some(i + 1),
                (b'?' | b'>', b'c') => Some(i + 1),
                (b'?', b'y') | (b'?', b'n') => Some(i + 1),
                _ => None,
            }
        }
        // OSC / DCS 应答：到 BEL 或 ST（ESC \）为止
        b']' | b'P' => {
            let mut i = 2;
            while i < d.len() {
                if d[i] == 0x07 {
                    return Some(i + 1);
                }
                if d[i] == 0x1b && d.get(i + 1) == Some(&b'\\') {
                    return Some(i + 2);
                }
                i += 1;
            }
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 浏览器那样发一帧：带掩码
    fn frame(op: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x12, 0x34, 0x56, 0x78];
        let mut f = vec![0x80 | op];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n if n < 65536 => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn typed_keys_count() {
        assert!(WsTap::new(None).feed(&frame(2, b"0l")));
        assert!(WsTap::new(None).feed(&frame(2, b"0\r")));
        assert!(WsTap::new(None).feed(&frame(2, b"0\x1b[A")));
        assert!(WsTap::new(None).feed(&frame(2, b"0\x1b")));
        assert!(WsTap::new(None).feed(&frame(2, "0你好".as_bytes())));
        // 长的粘贴
        let paste = [b"0".as_slice(), &[b'x'; 70000]].concat();
        assert!(WsTap::new(None).feed(&frame(2, &paste)));
    }

    #[test]
    fn auto_replies_dont_count() {
        for p in [
            b"0\x1b[I".as_slice(),
            b"0\x1b[O",
            b"0\x1b[<64;10;5M\x1b[<64;10;5M",
            b"0\x1b[<0;3;4m",
            b"0\x1b[M`!!",
            b"0\x1b[?1;2c",
            b"0\x1b[>0;276;0c",
            b"0\x1b]11;rgb:0000/0000/0000\x1b\\",
            b"0\x1b]10;rgb:ffff/ffff/ffff\x07",
            b"0\x1bP>|xterm.js(5.5.0)\x1b\\",
            b"0\x1b[?2026;2$y",
            b"1{\"columns\":80,\"rows\":24}",
            b"{\"AuthToken\":\"\",\"columns\":80}",
            b"2",
            b"3",
        ] {
            assert!(!WsTap::new(None).feed(&frame(2, p)), "{:?}", String::from_utf8_lossy(p));
        }
        // ping 不看
        assert!(!WsTap::new(None).feed(&frame(9, b"0abc")));
    }

    #[test]
    fn split_reads_and_many_frames() {
        let mut all = frame(2, b"0\x1b[I");
        all.extend(frame(2, b"1{\"columns\":80,\"rows\":24}"));
        all.extend(frame(2, b"0ls\r"));
        let mut t = WsTap::new(None);
        // 一个字节一个字节地喂：只有最后一帧收全那一下算
        let hits: Vec<bool> = all.iter().map(|b| t.feed(std::slice::from_ref(b))).collect();
        assert_eq!(hits.iter().filter(|h| **h).count(), 1);
        assert!(*hits.last().unwrap());
        // 一次喂完
        assert!(WsTap::new(None).feed(&all));
        // 中等长度（16 位长度）的帧之后接着能对上
        let mut t = WsTap::new(None);
        let mid = [b"0".as_slice(), &[0x1b, b'[', b'I'].repeat(100)].concat();
        assert!(t.feed(&frame(2, &mid)), "超过 PEEK 的算粘贴");
        assert!(!t.feed(&frame(2, b"0\x1b[O")));
        assert!(t.feed(&frame(2, b"0q")));
    }

    /// 压缩一条消息（沿用同一个字典），去掉末尾的 00 00 ff ff，按 RSV1 发
    fn zframe(c: &mut flate2::Compress, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(payload.len() + 64);
        c.compress_vec(payload, &mut out, flate2::FlushCompress::Sync).unwrap();
        assert!(out.ends_with(&[0, 0, 0xff, 0xff]));
        out.truncate(out.len() - 4);
        let mut f = frame(2, &out);
        f[0] |= 0x40;
        f
    }

    #[test]
    fn permessage_deflate() {
        let mut c = flate2::Compress::new(flate2::Compression::default(), false);
        let mut t = WsTap::new(Some(false));
        assert!(!t.feed(&zframe(&mut c, b"{\"AuthToken\":\"\",\"columns\":80,\"rows\":24}")));
        assert!(!t.feed(&zframe(&mut c, b"0\x1b[I")));
        assert!(!t.feed(&zframe(&mut c, b"1{\"columns\":80,\"rows\":24}")));
        assert!(t.feed(&zframe(&mut c, b"0l")));
        assert!(!t.feed(&zframe(&mut c, b"0\x1b[O")));
        // 不压缩的消息夹在中间也行
        assert!(t.feed(&frame(2, b"0s")));
        assert!(!t.feed(&zframe(&mut c, b"0\x1b[I")));
        assert!(t.feed(&zframe(&mut c, b"0\r")));
    }
}
