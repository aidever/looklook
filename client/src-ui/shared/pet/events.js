// 宠物“看得懂在干什么”（docs/PET_PACK.md §3.4.1）：从对话条目和状态里认出值得说一句的事
// （跑测试、测试过了、装依赖、提交、连着出错、改了很多文件、想了很久……），交给宠物配台词和动作。
// 纯逻辑，不碰 DOM，`node --test` 直接测。

/** 命令是干什么的：test / install / build / commit / push / danger / null */
export function commandKind(cmd) {
  const c = String(cmd ?? '').trim();
  if (!c) return null;
  if (DANGER.some((r) => r.test(c))) return 'danger';
  if (/\bgit\s+(?:-C\s+\S+\s+)?push\b/.test(c)) return 'push';
  if (/\bgit\s+(?:-C\s+\S+\s+)?commit\b/.test(c)) return 'commit';
  if (TEST.some((r) => r.test(c))) return 'test';
  if (INSTALL.some((r) => r.test(c))) return 'install';
  if (BUILD.some((r) => r.test(c))) return 'build';
  return null;
}
const DANGER = [
  /\brm\s+(-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r)\b/,
  /\bgit\s+push\b.*(--force\b|\s-f\b)/,
  /\bgit\s+reset\s+--hard\b/,
  /\bgit\s+clean\s+-[a-zA-Z]*f/,
  /\bdrop\s+(table|database)\b/i,
  /\bmkfs\b|\bdd\s+if=/,
  /\bchmod\s+-R\s+777\b/,
];
const TEST = [
  /\b(npm|pnpm|yarn|bun)\s+(run\s+)?test\b/,
  /\bcargo\s+(test|nextest)\b/,
  /\bgo\s+test\b/,
  /\b(pytest|vitest|jest|mocha|rspec|phpunit)\b/,
  /\bpython3?\s+-m\s+(pytest|unittest)\b/,
  /\bnode\s+--test\b/,
  /\b(mvn|gradle|\.\/gradlew)\s+(\S+\s+)*test\b/,
  /\bdotnet\s+test\b/,
  /\bmake\s+(test|check)\b/,
];
const INSTALL = [/\b(npm|pnpm|yarn|bun)\s+(install|i|add|ci)\b/, /\bpip3?\s+install\b/, /\buv\s+(add|sync|pip\s+install)\b/, /\bcargo\s+(add|install|fetch)\b/, /\bgo\s+(get|mod\s+download)\b/, /\b(apt|apt-get|brew|dnf|yum)\s+install\b/];
const BUILD = [/\bcargo\s+(build|check|clippy)\b/, /\b(npm|pnpm|yarn|bun)\s+run\s+build\b/, /\bgo\s+build\b/, /\b(make|cmake|ninja|tsc|vite\s+build|webpack|mvn\s+package|gradle\s+build)\b/];

/** 测试输出里的数字：`12 passed` / `ok. 12 passed` / `Tests: 12 passed` / `# pass 12` */
export function testCount(out) {
  const s = String(out ?? '');
  const m = s.match(/(\d+)\s+(passed|passing)\b/) ?? s.match(/#\s*pass\s+(\d+)/) ?? s.match(/test result: ok\. (\d+) passed/);
  return m ? Number(m[1]) : null;
}

const EDIT_TOOLS = ['Edit', 'MultiEdit', 'Write', 'NotebookEdit', 'apply_patch'];
const SEARCH_TOOLS = ['Grep', 'Glob'];
const WEB_TOOLS = ['WebFetch', 'WebSearch'];
const AGENT_TOOLS = ['Task', 'Agent'];
const base = (p) => String(p ?? '').split(/[\\/]/).filter(Boolean).pop() ?? '';

/**
 * 跟着对话走的观察者：每次条目或状态变了调 `step(items, status, now)`，返回这次新出现的事件
 * `[{ key, vars, state? }]`（key 是台词键，state 是想插播的动作池，比如 testPass）。
 * 第一次调用只记住现有的条目，不对历史说话。
 */
export function createWatcher() {
  const w = {
    seen: new Set(),
    pending: new Map(), // tool_use id → { name, kind, input }
    ready: false,
    phase: null,
    turn: null, // 这一轮：{ start, files:Set, cmds, tools, failedTest }
    errStreak: 0,
    lastFailTest: false,
    thinkSince: 0,
    thinkSaid: 0,
    sessionStart: 0,
    lastActive: 0,
  };

  const newTurn = (now) => ({ start: now, files: new Set(), cmds: 0, tools: 0, tests: 0, said: new Set() });

  function step(items, status, now = Date.now()) {
    const out = [];
    const list = items ?? [];
    if (!w.ready) {
      for (const it of list) markSeen(it);
      w.ready = true;
      w.phase = status?.phase ?? null;
      if (w.phase === 'running') w.turn = newTurn(now);
      return out;
    }
    // 新条目
    for (const it of list) {
      const id = itemId(it);
      if (id == null || w.seen.has(id)) {
        // 同一条 assistant 回复会多次更新：里面新加的 tool_use 也要认
        if (it.k === 'assistant') scanUses(it, out);
        continue;
      }
      w.seen.add(id);
      if (it.k === 'user' && !it.mid) out.push({ key: 'sent' });
      if (it.k === 'assistant') scanUses(it, out);
      if (it.k === 'tool_result') onResult(it, out);
    }
    // 状态
    const phase = status?.phase ?? null;
    if (phase !== w.phase) {
      if (phase === 'running' && w.phase !== 'approval') {
        w.turn = newTurn(now);
        w.thinkSince = now;
        w.thinkSaid = 0;
        if (!w.sessionStart || now - w.lastActive > 30 * 60e3) w.sessionStart = now;
      }
      if (phase === 'approval') {
        const cmd = status?.pending?.input?.command;
        if (commandKind(cmd) === 'danger') out.push({ key: 'danger', vars: { cmd: short(cmd) } });
      }
      if ((w.phase === 'running' || w.phase === 'approval') && phase === 'idle' && w.turn) {
        if (!status?.error) out.push(recap(w.turn, now));
        w.turn = null;
      }
      w.phase = phase;
    }
    if (phase === 'running' || phase === 'approval') w.lastActive = now;
    // 想了很久：思考中（没在用工具）超过 2 分钟，每 3 分钟最多一次
    if (phase === 'running' && !status?.tool) {
      const min = Math.floor((now - w.thinkSince) / 60e3);
      if (min >= 2 && now - w.thinkSaid > 3 * 60e3) {
        w.thinkSaid = now;
        out.push({ key: 'longThink', vars: { min } });
      }
    } else if (phase === 'running') w.thinkSince = now;
    return out;
  }

  function markSeen(it) {
    const id = itemId(it);
    if (id != null) w.seen.add(id);
    if (it.k === 'assistant') for (const b of it.blocks ?? []) if (b.t === 'tool_use') w.seen.add(`u:${b.id}`);
  }

  function scanUses(it, out) {
    for (const b of it.blocks ?? []) {
      if (b.t !== 'tool_use' || w.seen.has(`u:${b.id}`)) continue;
      w.seen.add(`u:${b.id}`);
      const input = b.input ?? {};
      const kind = b.name === 'Bash' ? commandKind(input.command) : null;
      w.pending.set(b.id, { name: b.name, kind, input });
      if (w.turn) w.turn.tools += 1;
      if (b.name === 'Bash') {
        if (w.turn) w.turn.cmds += 1;
        if (kind === 'test') out.push({ key: 'testRun' });
        else if (kind === 'install') out.push({ key: 'install' });
        else if (kind === 'build') out.push({ key: 'build' });
      } else if (EDIT_TOOLS.includes(b.name)) {
        const files = b.name === 'apply_patch' ? Object.keys(input.changes ?? {}) : [input.file_path ?? input.notebook_path];
        const before = w.turn?.files.size ?? 0;
        for (const f of files) if (f) w.turn?.files.add(f);
        const n = w.turn?.files.size ?? 0;
        if (n >= 5 && before < n && n % 5 === 0) out.push({ key: 'editMany', vars: { n } });
        else out.push({ key: 'edit', vars: { file: base(files[0]) }, quiet: true });
      } else if (SEARCH_TOOLS.includes(b.name)) out.push({ key: 'search', quiet: true });
      else if (WEB_TOOLS.includes(b.name)) out.push({ key: 'web', quiet: true });
      else if (AGENT_TOOLS.includes(b.name)) out.push({ key: 'agentSpawn' });
    }
  }

  function onResult(it, out) {
    const use = w.pending.get(it.tool_use_id);
    w.pending.delete(it.tool_use_id);
    if (!use || use.name !== 'Bash') return;
    if (it.is_error) {
      w.errStreak += 1;
      if (use.kind === 'test') {
        w.lastFailTest = true;
        out.push({ key: 'testFail', state: 'testFail', count: 'errors' });
      } else if (w.errStreak >= 3) out.push({ key: 'errorStreak', vars: { n: w.errStreak } });
      return;
    }
    w.errStreak = 0;
    if (use.kind === 'test') {
      const n = testCount(it.content);
      if (w.turn) w.turn.tests += 1;
      out.push({ key: w.lastFailTest ? 'fixed' : 'testPass', state: 'testPass', vars: { n: n ?? '' }, count: w.lastFailTest ? 'fixes' : 'tests' });
      w.lastFailTest = false;
    } else if (use.kind === 'commit') out.push({ key: 'commit', state: 'commit', count: 'commits' });
    else if (use.kind === 'push') out.push({ key: 'push', state: 'commit' });
  }

  return {
    step,
    /** 连续干了多久（分钟）：中间歇超过 30 分钟重新算 */
    sessionMinutes: (now = Date.now()) => (w.sessionStart && now - w.lastActive < 30 * 60e3 ? Math.floor((now - w.sessionStart) / 60e3) : 0),
  };
}

const itemId = (it) => it?.key ?? it?.uuid ?? it?.id ?? null;
const short = (s) => {
  const one = String(s ?? '').replace(/\s+/g, ' ').trim();
  return one.length > 40 ? `${one.slice(0, 39)}…` : one;
};

/** 做完一轮：挑一种说法（很快、很久、很忙），没什么可说的就是普通的 done */
export function recap(turn, now) {
  const ms = Math.max(0, now - turn.start);
  const dur = fmtDur(ms);
  const files = turn.files.size;
  const vars = { dur, files, cmds: turn.cmds, min: Math.round(ms / 60e3) };
  if (ms >= 5 * 60e3) return { key: 'recapLong', vars, recap: true, count: 'turns' };
  if (files >= 3 || turn.cmds >= 5) return { key: 'recapBusy', vars, recap: true, count: 'turns' };
  if (ms < 20e3 && turn.tools > 0) return { key: 'recapQuick', vars, recap: true, count: 'turns' };
  return { key: 'done', vars, recap: true, count: 'turns' };
}

export function fmtDur(ms) {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m${s % 60 ? ` ${s % 60}s` : ''}`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}
