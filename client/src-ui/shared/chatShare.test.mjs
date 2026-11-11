import assert from 'node:assert/strict';
import { test } from 'node:test';

import { shareHtml, shareMarkdown, turnsOf } from './chatShare.js';

const items = [
  { k: 'assistant', key: 'a0', blocks: [{ t: 'text', text: '接着上次的' }] },
  { k: 'user', key: 'u1', text: '修一下登录', images: 1 },
  { k: 'assistant', key: 'a1', blocks: [{ t: 'thinking', text: 'x' }, { t: 'tool_use', name: 'Bash', input: { command: 'ls  -la' } }, { t: 'text', text: '改好了 **两处**' }] },
  { k: 'tool_result', key: 'r1' },
  { k: 'assistant', key: 'a2', blocks: [{ t: 'tool_use', name: 'Edit', input: { file_path: '/a.js' } }] },
  { k: 'user', key: 'u2', text: '<script>alert(1)</script>' },
];

test('一条 = 你发的一句 + 这一轮 AI 做的', () => {
  const t = turnsOf(items);
  assert.equal(t.length, 3);
  assert.equal(t[0].user, null);
  assert.deepEqual(t[1].user, { text: '修一下登录', images: 1 });
  assert.equal(t[1].steps, 2);
  assert.deepEqual(t[1].parts.map((p) => p.t), ['tool', 'text', 'tool']);
  assert.equal(t[1].parts[0].arg, 'ls -la');
  assert.equal(t[2].parts.length, 0);
});

test('Markdown：可以不带工具', () => {
  const t = turnsOf(items).slice(1, 2);
  const with_ = shareMarkdown(t, { user: '我', agent: 'Claude', title: 'T' });
  assert.match(with_, /^# T\n\n## 我\n\n修一下登录\n\n\[1 image\(s\)\]\n\n## Claude\n\n- `Bash` ls -la\n\n改好了 \*\*两处\*\*\n\n- `Edit` \/a.js\n$/);
  assert.doesNotMatch(shareMarkdown(t, { tools: false }), /Bash/);
});

test('网页：你说的话转义，没有脚本，长的折起来', () => {
  const long = { k: 'user', key: 'u3', text: Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n') };
  const html = shareHtml(turnsOf([...items, long]), {
    md: (s) => `<p>${s}</p>`,
    labels: { title: 'T<x>', user: '我', agent: 'AI', readonly: 'ro', exported: (w) => w, steps: (n) => `${n} 步`, images: (n) => `${n} 图`, more: (n) => `全文 ${n} 行` },
  });
  assert.match(html, /&lt;script&gt;alert\(1\)&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script/);
  assert.match(html, /<title>T&lt;x&gt;<\/title>/);
  assert.match(html, /全文 20 行/);
  assert.match(html, /<summary>1 步<\/summary>/);
});
