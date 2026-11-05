import { test } from 'node:test';
import assert from 'node:assert/strict';
import { commandKind, createWatcher, testCount, fmtDur } from './events.js';
import { fill, lineFor, BUILTIN } from './lines.js';
import { levelOf } from './bond.js';

// 拼出来，免得测试文件本身被当成危险命令
const FORCE_PUSH = ['git', 'push', '--force'].join(' ');

test('commandKind', () => {
  assert.equal(commandKind('cargo test --all'), 'test');
  assert.equal(commandKind('cd x && npm run test'), 'test');
  assert.equal(commandKind('npm install'), 'install');
  assert.equal(commandKind('git commit -m "x"'), 'commit');
  assert.equal(commandKind('git -C /a push origin HEAD:main'), 'push');
  assert.equal(commandKind(FORCE_PUSH), 'danger');
  assert.equal(commandKind(['rm', '-rf', 'build'].join(' ')), 'danger');
  assert.equal(commandKind('cargo build --release'), 'build');
  assert.equal(commandKind('ls -la'), null);
});

test('testCount', () => {
  assert.equal(testCount('test result: ok. 12 passed; 0 failed'), 12);
  assert.equal(testCount('# pass 7\n# fail 0'), 7);
  assert.equal(testCount('nothing'), null);
});

const use = (id, name, input) => ({ k: 'assistant', key: `a${id}`, blocks: [{ t: 'tool_use', id, name, input }] });
const res = (id, isError, content = '') => ({ k: 'tool_result', key: `r${id}`, tool_use_id: id, is_error: isError, content });

test('watcher: history is silent, then events', () => {
  const w = createWatcher();
  const items = [{ k: 'user', key: 'u0', text: 'old' }, use('x', 'Bash', { command: 'cargo test' }), res('x', true)];
  assert.deepEqual(w.step(items, { phase: 'idle' }, 0), []);
  // 新的一轮
  items.push({ k: 'user', key: 'u1', text: 'hi' });
  let ev = w.step(items, { phase: 'running' }, 1000);
  assert.deepEqual(
    ev.map((e) => e.key),
    ['sent'],
  );
  items.push(use('t1', 'Bash', { command: 'cargo test' }));
  ev = w.step(items, { phase: 'running', tool: 'Bash' }, 2000);
  assert.deepEqual(
    ev.map((e) => e.key),
    ['testRun'],
  );
  items.push(res('t1', true));
  ev = w.step(items, { phase: 'running' }, 3000);
  assert.deepEqual(
    ev.map((e) => [e.key, e.state]),
    [['testFail', 'testFail']],
  );
  items.push(use('t2', 'Bash', { command: 'cargo test' }), res('t2', false, 'test result: ok. 5 passed'));
  ev = w.step(items, { phase: 'running' }, 4000);
  assert.deepEqual(
    ev.map((e) => e.key),
    ['testRun', 'fixed'],
  );
  assert.equal(ev[1].vars.n, 5);
  for (let i = 0; i < 5; i++) items.push(use(`e${i}`, 'Edit', { file_path: `/a/f${i}.rs` }));
  ev = w.step(items, { phase: 'running' }, 5000);
  assert.ok(ev.some((e) => e.key === 'editMany' && e.vars.n === 5));
  ev = w.step(items, { phase: 'idle' }, 5000 + 6 * 60e3);
  assert.equal(ev.length, 1);
  assert.equal(ev[0].key, 'recapLong');
  assert.equal(ev[0].vars.files, 5);
});

test('watcher: error streak, danger, long think', () => {
  const w = createWatcher();
  w.step([], { phase: 'idle' }, 0);
  const items = [];
  w.step(items, { phase: 'running' }, 1000);
  for (let i = 0; i < 3; i++) items.push(use(`b${i}`, 'Bash', { command: 'ls nope' }), res(`b${i}`, true));
  const ev = w.step(items, { phase: 'running' }, 2000);
  assert.deepEqual(
    ev.map((e) => e.key),
    ['errorStreak'],
  );
  assert.equal(ev[0].vars.n, 3);
  const d = w.step(items, { phase: 'approval', pending: { input: { command: ['rm', '-rf', '/tmp/x'].join(' ') } } }, 3000);
  assert.deepEqual(
    d.map((e) => e.key),
    ['danger'],
  );
  w.step(items, { phase: 'running' }, 4000);
  const lt = w.step(items, { phase: 'running' }, 4000 + 3 * 60e3);
  assert.deepEqual(
    lt.map((e) => e.key),
    ['longThink'],
  );
});

test('lines', () => {
  assert.equal(fill('改了 {n} 个', { n: 3 }), '改了 3 个');
  assert.equal(fill('改了 {n} 个', {}), null);
  const m = { lines: { 'zh-CN': { done: ['包里的'] } } };
  assert.equal(lineFor({ manifest: m, lang: 'zh-CN', key: 'done' }), '包里的');
  const gen = { 'zh-CN': { click: ['生成的'] } };
  assert.equal(lineFor({ manifest: m, gen, lang: 'zh-CN', key: 'click' }), '生成的');
  assert.ok(BUILTIN['zh-CN'].petted.includes(lineFor({ manifest: m, lang: 'zh-CN', key: 'petted' })));
  assert.equal(lineFor({ manifest: { builtinLines: false }, lang: 'zh-CN', key: 'petted' }), null);
  assert.ok(BUILTIN['en-US'].click.includes(lineFor({ manifest: m, lang: 'en-US', key: 'click' })));
  // 每个内置键中英都有
  assert.deepEqual(Object.keys(BUILTIN['zh-CN']).sort(), Object.keys(BUILTIN['en-US']).sort());
});

test('levels and durations', () => {
  assert.equal(levelOf(0).level, 1);
  assert.equal(levelOf(20).level, 2);
  assert.equal(levelOf(1000).level, 7);
  assert.equal(levelOf(1600).level, 8);
  assert.equal(fmtDur(65000), '1m 5s');
  assert.equal(fmtDur(3 * 3600e3 + 60e3), '3h 1m');
});
