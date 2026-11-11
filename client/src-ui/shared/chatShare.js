// 对话的只读分享（ChatShare.jsx 的界面）：按“一次你发的 + AI 这一轮做的”分成一条条，挑着导出成
// 一个独立的网页（.html，谁都能打开、只能看）或 Markdown。不依赖 React，单元测试见 chatShare.test.mjs。

/** 一次工具调用写成一行：工具名 + 最能说明它的参数 */
function toolLine(b) {
  const i = b.input ?? {};
  const arg = i.command ?? i.file_path ?? i.path ?? i.pattern ?? i.url ?? i.query ?? i.description ?? '';
  const one = String(arg).replace(/\s+/g, ' ').trim();
  return { name: String(b.name ?? ''), arg: one.length > 160 ? `${one.slice(0, 159)}…` : one };
}

/**
 * 对话条目分成一条条：每条从你发的一句开始，带上到下一句之前 AI 的回复和工具调用。
 * 第一句之前的（接着以前的对话时开头的总结等）单独算一条、没有 user。
 * 返回 [{ key, user: { text, images } | null, parts: [{ t: 'text', text } | { t: 'tool', name, arg }], chars, steps }]
 */
export function turnsOf(items) {
  const out = [];
  let cur = null;
  const start = (key, user) => {
    cur = { key, user, parts: [], chars: user?.text?.length ?? 0, steps: 0 };
    out.push(cur);
  };
  for (const it of items ?? []) {
    if (it.k === 'user') {
      start(it.key, { text: String(it.text ?? ''), images: it.images ?? 0 });
      continue;
    }
    if (it.k !== 'assistant') continue;
    for (const b of it.blocks ?? []) {
      if (b.t === 'text' && b.text?.trim()) {
        if (!cur) start(`pre:${it.key}`, null);
        cur.parts.push({ t: 'text', text: b.text.trim() });
        cur.chars += b.text.length;
      } else if (b.t === 'tool_use') {
        if (!cur) start(`pre:${it.key}`, null);
        cur.parts.push({ t: 'tool', ...toolLine(b) });
        cur.steps += 1;
      }
    }
  }
  return out.filter((x) => (x.user && (x.user.text.trim() || x.user.images)) || x.parts.length);
}

/** AI 这一轮说的话（不含工具），拼起来 */
export const replyText = (turn) => turn.parts.filter((p) => p.t === 'text').map((p) => p.text).join('\n\n');

/** 导出文件名：标题里不能放进文件名的字换成短横线 */
export function shareName(title, ext, date = new Date()) {
  const base = (title || 'chat').replace(/[\\/:*?"<>|\s]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 60) || 'chat';
  return `${base}-${date.toISOString().slice(0, 10)}.${ext}`;
}

/**
 * 导出成 Markdown。`names`：{ user, agent, title, images(n) }；`tools`：工具调用每次一行。
 */
export function shareMarkdown(turns, { user = 'You', agent = 'Assistant', title = '', tools = true, images = (n) => `[${n} image(s)]` } = {}) {
  const out = [];
  if (title) out.push(`# ${title}`, '');
  for (const turn of turns) {
    if (turn.user) {
      out.push(`## ${user}`, '');
      if (turn.user.text.trim()) out.push(turn.user.text.trim(), '');
      if (turn.user.images) out.push(images(turn.user.images), '');
    }
    const parts = turn.parts.filter((p) => tools || p.t === 'text');
    if (!parts.length) continue;
    out.push(`## ${agent}`, '');
    let list = [];
    const flush = () => {
      if (list.length) out.push(...list, '');
      list = [];
    };
    for (const p of parts) {
      if (p.t === 'tool') list.push(`- \`${p.name}\`${p.arg ? ` ${p.arg}` : ''}`);
      else {
        flush();
        out.push(p.text, '');
      }
    }
    flush();
  }
  return `${out.join('\n').replace(/\n{3,}/g, '\n\n').trim()}\n`;
}

const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** 你发的一句超过这么多行就先折起来（网页里点开看全文） */
const FOLD_LINES = 14;

/**
 * 导出成一个独立的网页：样式都在里面，不加载任何外部东西，没有脚本；跟系统深浅色。
 * `md(text)` 把 AI 的 Markdown 转成（已清洗过的）HTML。`labels`：{ user, agent, title, readonly, exported, steps(n), images(n), more }
 */
export function shareHtml(turns, { md, labels, tools = true, lang = 'zh-CN', date = new Date() }) {
  const L = labels;
  const body = turns
    .map((turn) => {
      const rows = [];
      if (turn.user) {
        const text = turn.user.text.trim();
        const lines = text.split('\n');
        const shown = lines.length > FOLD_LINES
          ? `<details><summary><span class="pre">${esc(lines.slice(0, 6).join('\n'))}</span><span class="more">${esc(L.more(lines.length))}</span></summary><div class="pre">${esc(lines.slice(6).join('\n'))}</div></details>`
          : `<div class="pre">${esc(text)}</div>`;
        rows.push(`<section class="me"><div class="who">${esc(L.user)}</div><div class="bubble">${text ? shown : ''}${turn.user.images ? `<div class="note">${esc(L.images(turn.user.images))}</div>` : ''}</div></section>`);
      }
      const parts = turn.parts.filter((p) => tools || p.t === 'text');
      if (parts.length) {
        const html = [];
        let list = [];
        const flush = () => {
          if (list.length) html.push(`<details class="tools"><summary>${esc(L.steps(list.length))}</summary><ul>${list.join('')}</ul></details>`);
          list = [];
        };
        for (const p of parts) {
          if (p.t === 'tool') list.push(`<li><code>${esc(p.name)}</code> ${esc(p.arg)}</li>`);
          else {
            flush();
            html.push(`<div class="md">${md(p.text)}</div>`);
          }
        }
        flush();
        rows.push(`<section class="ai"><div class="who">${esc(L.agent)}</div>${html.join('')}</section>`);
      }
      return `<article>${rows.join('')}</article>`;
    })
    .join('\n');
  const when = date.toLocaleString(lang);
  return `<!doctype html>
<html lang="${esc(lang)}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="robots" content="noindex"><title>${esc(L.title)}</title>
<style>${SHARE_CSS}</style></head>
<body><main>
<header><h1>${esc(L.title)}</h1><p class="note">${esc(L.readonly)} · ${esc(L.exported(when))}</p></header>
${body}
</main></body></html>
`;
}

const SHARE_CSS = `
:root{--bg:#f7f7f5;--fg:#1f2328;--muted:#6b7280;--line:#e5e7eb;--me:#fff1ea;--me-line:#f6d3c2;--code:#f1f1ef;--accent:#c96442}
@media (prefers-color-scheme:dark){:root{--bg:#18181b;--fg:#e8e8ea;--muted:#9a9aa3;--line:#2e2e33;--me:#2b211d;--me-line:#4a342a;--code:#232327;--accent:#e08a66}}
*{box-sizing:border-box}html,body{margin:0;background:var(--bg);color:var(--fg)}
body{font:15px/1.7 -apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Hiragino Sans GB","Microsoft YaHei",sans-serif;-webkit-text-size-adjust:100%}
main{max-width:820px;margin:0 auto;padding:28px 16px 64px}
header{margin-bottom:28px;padding-bottom:16px;border-bottom:1px solid var(--line)}
h1{font-size:22px;line-height:1.35;margin:0 0 6px;overflow-wrap:anywhere}
.note{color:var(--muted);font-size:13px;margin:4px 0 0}
article{margin:0 0 28px}
.who{font-size:12px;font-weight:600;color:var(--muted);margin:0 0 6px;letter-spacing:.02em}
.me{display:flex;flex-direction:column;align-items:flex-end;margin-bottom:18px}
.me .who{align-self:flex-end}
.bubble{max-width:min(640px,100%);background:var(--me);border:1px solid var(--me-line);border-radius:16px;padding:10px 14px}
.pre{white-space:pre-wrap;overflow-wrap:anywhere}
details>summary{cursor:pointer;list-style:none}details>summary::-webkit-details-marker{display:none}
.bubble summary .more{display:block;color:var(--accent);font-size:13px;margin-top:4px}
.bubble details[open] summary .more{display:none}
.ai .md{overflow-wrap:anywhere}
.md p{margin:.5em 0}.md pre{background:var(--code);border-radius:10px;padding:12px 14px;overflow-x:auto;font-size:13px;line-height:1.55}
.md code{font:13px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;background:var(--code);border-radius:5px;padding:1px 5px}
.md pre code{background:none;padding:0}
.md table{border-collapse:collapse;display:block;overflow-x:auto;max-width:100%}.md th,.md td{border:1px solid var(--line);padding:6px 10px}
.md blockquote{margin:.6em 0;padding:0 0 0 12px;border-left:3px solid var(--line);color:var(--muted)}
.md img{max-width:100%}.md a{color:var(--accent)}
.tools{margin:8px 0;font-size:13px;color:var(--muted)}
.tools summary{display:inline-block;padding:2px 10px;border:1px solid var(--line);border-radius:999px}
.tools ul{margin:8px 0;padding-left:18px}.tools li{margin:2px 0;overflow-wrap:anywhere}
`;
