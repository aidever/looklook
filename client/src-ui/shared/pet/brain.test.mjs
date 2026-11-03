import test from 'node:test';
import assert from 'node:assert/strict';
import { agentState, createBrain, LAYER, pickLine, pickWeighted, textOf } from './brain.js';

const P = (...names) => names.map((clip) => ({ clip, weight: 1 }));
const M = {
  stage: { width: 640, height: 360, facing: 'left' },
  clips: {
    idle: { loop: true, mirror: true },
    think: { mirror: true },
    work: { mirror: true },
    call: { mirror: true },
    cheer: { mirror: true },
    sigh: { mirror: true },
    click: { mirror: true },
    held: { loop: true, mirror: true },
    walk: { mirror: true, move: { min: 50, max: 100, lead: 0.5, tail: 0.5 } },
    look: { turn: true, mirror: true },
    sign: { mirror: false },
    'sleep-start': { mirror: true },
    'sleep-loop': { loop: true, mirror: true },
    'sleep-end': { mirror: true },
  },
  states: { idle: P('idle'), thinking: P('think'), tool: P('work'), waiting: P('call'), done: P('cheer'), error: P('sigh') },
  actions: {
    click: P('click'),
    drag: P('held'),
    turn: P('look'),
    move: P('walk'),
    sleep: [{ start: P('sleep-start'), loop: P('sleep-loop'), end: P('sleep-end') }],
    random: [{ id: 'text', weight: 10, clips: P('sign') }],
  },
  behavior: { idle: 40, turn: 5, move: 10, sleepAfter: 60 },
};
const ROOM = { left: 500, right: 500 };
// 固定的“随机数”：依次取
const seq = (...xs) => {
  let i = 0;
  return () => xs[i++ % xs.length];
};

test('对话状态换算', () => {
  assert.deepEqual(agentState({ phase: 'running', tool: null }), { state: 'thinking', once: null });
  assert.deepEqual(agentState({ phase: 'running', tool: 'Bash' }), { state: 'tool', once: null });
  assert.deepEqual(agentState({ phase: 'approval' }), { state: 'waiting', once: null });
  assert.deepEqual(agentState({ phase: 'tui' }), { state: 'terminal', once: null });
  assert.deepEqual(agentState(null), { state: 'offline', once: null });
  assert.equal(agentState({ phase: 'idle' }, { phase: 'running' }).once, 'done');
  assert.equal(agentState({ phase: 'idle', error: 'x' }, { phase: 'running' }).once, 'error');
  assert.equal(agentState({ phase: 'running', tool: null }, { phase: 'running', tool: 'Bash' }).once, 'toolDone');
  // 刚打开（没有 prev）不补播
  assert.equal(agentState({ phase: 'idle' }, undefined).once, null);
  assert.equal(agentState({ phase: 'idle' }, { phase: 'idle' }).once, null);
});

test('带权重抽、避开刚播过的', () => {
  const l = [{ clip: 'a', weight: 1 }, { clip: 'b', weight: 3 }];
  assert.equal(pickWeighted(l, () => 0.1).clip, 'a');
  assert.equal(pickWeighted(l, () => 0.5).clip, 'b');
  assert.equal(pickWeighted(l, () => 0.1, 'a').clip, 'b');
  assert.equal(pickWeighted([{ clip: 'a' }], () => 0.9, 'a').clip, 'a', '只有一项时照播');
  assert.equal(pickWeighted([], () => 0), null);
});

test('思考立刻打断待机，换成调工具等这段播完', () => {
  const b = createBrain(M, { rng: () => 0 });
  assert.equal(b.start(0, ROOM).clip, 'idle');
  const p = b.setAgent('thinking', null, 100, ROOM);
  assert.equal(p.clip, 'think');
  assert.equal(p.layer, LAYER.state);
  assert.equal(p.line, 'thinking');
  assert.equal(b.setAgent('tool', null, 200, ROOM), null, '同一级：不打断');
  assert.equal(b.ended(300, ROOM).clip, 'work');
});

test('做完插播一次，然后回到闲时', () => {
  const b = createBrain(M, { rng: () => 0 });
  b.start(0, ROOM);
  b.setAgent('thinking', null, 1, ROOM);
  const p = b.setAgent('idle', 'done', 2, ROOM);
  assert.equal(p.clip, 'cheer');
  assert.equal(p.line, 'done');
  assert.equal(b.ended(3, ROOM).clip, 'idle');
});

test('没配的状态按缺省回退', () => {
  const m = { ...M, states: { idle: P('idle') } };
  const b = createBrain(m, { rng: () => 0 });
  b.start(0, ROOM);
  // tool → thinking → idle：最后是 idle 池
  assert.equal(b.setAgent('tool', null, 1, ROOM)?.clip ?? 'idle', 'idle');
  // done 没配：不插播，直接回到闲时
  assert.equal(b.setAgent('idle', 'done', 2, ROOM).line, null);
});

test('点击、拖拽优先，放下后接着播', () => {
  const b = createBrain(M, { rng: () => 0 });
  b.start(0, ROOM);
  b.setAgent('thinking', null, 1, ROOM);
  assert.equal(b.click(2).clip, 'click');
  assert.equal(b.dragStart(3).clip, 'held');
  assert.equal(b.setAgent('waiting', null, 4, ROOM), null, '拖着的时候不换');
  assert.equal(b.ended(5, ROOM).clip, 'held', '一直拎着');
  assert.equal(b.dragEnd(6, ROOM).clip, 'call', '放下：回到现在的状态');
});

test('闲久了睡觉，点一下醒，干活也会醒', () => {
  const b = createBrain(M, { rng: () => 0 });
  b.start(0, ROOM);
  const p = b.ended(61_000, ROOM);
  assert.equal(p.clip, 'sleep-start');
  assert.equal(p.line, 'sleepStart');
  assert.equal(b.ended(62_000, ROOM).clip, 'sleep-loop');
  assert.equal(b.ended(63_000, ROOM).clip, 'sleep-loop');
  assert.equal(b.click(64_000).clip, 'sleep-end');
  assert.equal(b.ended(65_000, ROOM).clip, 'idle');
  assert.equal(b.sleeping, false);
  // 再睡着，这次 agent 开始干活
  b.ended(200_000, ROOM);
  b.ended(201_000, ROOM);
  assert.equal(b.setAgent('thinking', null, 202_000, ROOM).clip, 'sleep-end');
  assert.equal(b.ended(203_000, ROOM).clip, 'think');
});

test('走动：按地方选方向，朝向跟着走', () => {
  // start：抽到待机（0）、待机段（0）；ended：抽到 move（权重区间 45~55 → 0.75）、走动段（0）、方向（0.9 → 右）、距离（0 → min）
  const b = createBrain(M, { rng: seq(0, 0, 0.75, 0, 0.9, 0) });
  b.start(0, ROOM);
  const p = b.ended(1, ROOM);
  assert.equal(p.clip, 'walk');
  assert.equal(p.move.dx, 50);
  assert.equal(p.mirror, true, '素材朝左，往右走要镜像');
  assert.equal(b.facing, 'right');
  // 右边没地方：只能往左（不用抽方向）
  const b2 = createBrain(M, { rng: seq(0, 0, 0.75, 0, 0) });
  b2.start(0, ROOM);
  const q = b2.ended(1, { left: 300, right: 10 });
  assert.equal(q.move.dx, -50);
  assert.equal(q.mirror, false);
  // 两边都没地方：不走，待机
  const b3 = createBrain(M, { rng: seq(0, 0, 0.75, 0, 0) });
  b3.start(0, ROOM);
  assert.equal(b3.ended(1, { left: 10, right: 10 }).clip, 'idle');
});

test('转身段播完朝向反过来；不能镜像的段朝右时不播', () => {
  const only = (b) => ({ ...M, behavior: { idle: 0, turn: 0, move: 0, sleepAfter: 0, ...b } });
  const b = createBrain({ ...only({ turn: 1 }), actions: { ...M.actions, random: [] } }, { rng: () => 0 });
  assert.equal(b.start(0, ROOM).clip, 'look');
  assert.equal(b.ended(1, ROOM).clip, 'idle', '耍完一段先待机');
  assert.equal(b.facing, 'right');
  // 朝右时只剩带字的段可选：照播但不镜像
  const p = b.ended(2, ROOM);
  assert.equal(p.clip, 'look');
  b.ended(3, ROOM);
  assert.equal(b.facing, 'left');
  const b2 = createBrain(only({}), { rng: () => 0 });
  assert.equal(b2.start(0, ROOM).clip, 'sign', '只有带字的随机动作：朝左时能播');
});

test('台词和名字按语言取', () => {
  const lines = { 'zh-CN': { done: ['搞定'] }, 'en-US': { done: ['Done'] } };
  assert.equal(pickLine(lines, 'en-US', 'done'), 'Done');
  assert.equal(pickLine(lines, 'ja', 'done'), '搞定');
  assert.equal(pickLine(lines, 'en-US', 'nope'), null);
  assert.equal(textOf({ 'zh-CN': '橙子', 'en-US': 'Orange' }, 'en-US'), 'Orange');
  assert.equal(textOf('x', 'en-US'), 'x');
});
