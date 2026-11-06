// 对话宠物的大脑（docs/PET_PACK.md §4）：现在该播哪段、朝哪边、要不要走、头顶说什么。
// 不碰 DOM、不看时间以外的东西，`node --test` 直接测。播放器（PetPlayer.jsx）每段播完调 `ended()`，
// agent 状态变了调 `setAgent()`，点、拖调 `click()` / `dragStart()` / `dragEnd()`；每个调用返回要播的 `Play` 或 null（继续播当前这段）。

/** 优先级（高的打断低的） */
export const LAYER = { idle: 0, sleep: 1, state: 2, once: 3, click: 4, drag: 5 };

/** 持续的状态：一段播完再抽一段，直到状态变了 */
const PERSIST = ['thinking', 'tool', 'waiting', 'terminal', 'offline'];
/** 没配时用哪个（§3.4） */
const FALLBACK = {
  idle: ['idle'],
  thinking: ['thinking', 'idle'],
  tool: ['tool', 'thinking', 'idle'],
  toolDone: ['toolDone'],
  waiting: ['waiting', 'idle'],
  terminal: ['terminal', 'waiting', 'idle'],
  done: ['done'],
  error: ['error'],
  offline: ['offline'],
  // 看得懂在干什么（events.js）：没配专门的动作就借 done / error 的
  testPass: ['testPass', 'done'],
  fixed: ['testPass', 'done'],
  testFail: ['testFail', 'error'],
  commit: ['commit', 'done'],
  hello: ['hello', 'done'],
  levelUp: ['levelUp', 'done'],
};

/**
 * 对话状态 → 宠物状态（§6.4）。`prev` 是上一次的 status，用来认出“刚跑完”“刚出错”“一个工具刚结束”。
 * 返回 `{ state, once }`：state 是持续状态，once 是这次要插播一次的（done / error / toolDone）或 null。
 */
export function agentState(status, prev) {
  const phase = status?.phase ?? 'detached';
  const state =
    phase === 'running' ? (status.tool ? 'tool' : 'thinking') : phase === 'approval' ? 'waiting' : phase === 'tui' ? 'terminal' : phase === 'idle' ? 'idle' : 'offline';
  let once = null;
  const was = prev?.phase;
  if (prev && (was === 'running' || was === 'approval') && phase === 'idle') once = status.error ? 'error' : 'done';
  else if (prev && was === 'running' && phase === 'running' && prev.tool && !status.tool) once = 'toolDone';
  return { state, once };
}

/** 带权重随机抽一项；`avoid` 是刚播过的（池子里不止一项时避开） */
export function pickWeighted(list, rng, avoid) {
  const items = list.length > 1 && avoid ? list.filter((x) => x.clip !== avoid) : list;
  if (!items.length) return null;
  const total = items.reduce((s, x) => s + (x.weight ?? 1), 0);
  let r = rng() * total;
  for (const x of items) {
    r -= x.weight ?? 1;
    if (r < 0) return x;
  }
  return items[items.length - 1];
}

/**
 * @param {object} m 换算后的清单（后端 `manifest_out`）
 * @param {{ rng?: () => number, now?: number, roam?: boolean }} opts
 * `roam`：更爱动——闲时多走动、走得远（桌面宠物满屏跑），待机少一些
 */
export function createBrain(m, { rng = Math.random, now = 0, roam = false } = {}) {
  const clips = m.clips ?? {};
  const states = m.states ?? {};
  const actions = m.actions ?? {};
  const behavior = { idle: 40, turn: 5, move: 10, sleepAfter: 600, ...(m.behavior ?? {}) };
  // 更主动：走动的机会翻几倍、发呆少一半；单次走的距离放大（最远到屏幕那头）
  const reach = roam ? 4 : 1;
  if (roam) {
    behavior.move = Math.max(behavior.move * 3, 15);
    behavior.idle *= 0.5;
  }
  const natural = m.stage?.facing === 'right' ? 'right' : 'left';
  const s = {
    agent: 'idle',
    facing: natural,
    cur: null,
    last: null,
    idleSince: now,
    /** 睡觉：null 或 { set, phase: 'start' | 'loop' | 'end' } */
    sleep: null,
    dragging: false,
    /** 上一段闲时动作不是待机：下一段先待机一下，不连着耍 */
    busy: false,
  };

  const exists = (p) => p && clips[p.clip];
  const pool = (state) => {
    for (const k of FALLBACK[state] ?? [state]) {
      const l = (states[k] ?? []).filter(exists);
      if (l.length) return { key: k, list: l };
    }
    return null;
  };
  // 朝向：画面里人物朝 natural；要朝另一边就镜像。不能镜像的段只在不镜像时播
  const mirrored = () => s.facing !== natural;
  const playable = (list) => {
    const ok = list.filter((p) => exists(p) && (!mirrored() || clips[p.clip].mirror !== false));
    return ok.length ? ok : list.filter(exists);
  };

  /** 组装一段要播的 */
  const play = (p, layer, extra = {}) => {
    if (!p) return null;
    const c = clips[p.clip];
    const out = { clip: p.clip, mirror: mirrored() && c.mirror !== false, layer, move: null, line: null, fade: false, ...extra };
    s.cur = out;
    s.last = p.clip;
    return out;
  };

  const fromList = (list, layer, extra) => play(pickWeighted(playable(list), rng, s.last), layer, extra);

  /** 闲时：待机、转身、走、随机动作、到点睡觉 */
  const idleNext = (t, room) => {
    const sleepSets = (actions.sleep ?? []).filter((x) => x.start?.length && x.loop?.length && x.end?.length);
    if (behavior.sleepAfter > 0 && sleepSets.length && t - s.idleSince >= behavior.sleepAfter * 1000) return startSleep(sleepSets);
    const idle = pool('idle');
    // 更爱动：刚走完一段，多半接着往同一边走（一段视频走不远，连起来就能跑到屏幕那头）
    const went = s.walked;
    s.walked = null;
    if (roam && went && room && (room[went] ?? 0) > 80 && rng() < 0.65) {
      const again = walkDir(went, room);
      if (again) return again;
    }
    if (s.busy || !idle) {
      s.busy = false;
      return idle ? fromList(idle.list, LAYER.idle) : null;
    }
    const opts = [{ k: 'idle', w: behavior.idle }];
    if ((actions.turn ?? []).some(exists)) opts.push({ k: 'turn', w: behavior.turn });
    if ((actions.move ?? []).some(exists) && room) opts.push({ k: 'move', w: behavior.move });
    for (const g of actions.random ?? []) if ((g.clips ?? []).some(exists)) opts.push({ k: 'random', g, w: g.weight ?? 1 });
    const pick = pickWeighted(
      opts.filter((o) => o.w > 0).map((o) => ({ ...o, clip: o.k === 'random' ? `random:${o.g.id}` : o.k, weight: o.w })),
      rng,
    );
    if (!pick || pick.k === 'idle') return fromList(idle.list, LAYER.idle);
    s.busy = true;
    if (pick.k === 'turn') return fromList(actions.turn, LAYER.idle, { turn: true });
    if (pick.k === 'move') return walk(room) ?? (s.busy = false, fromList(idle.list, LAYER.idle));
    return fromList(pick.g.clips, LAYER.idle);
  };

  /** 走一段：方向看哪边有地方（两边都够就随机），距离在 [min, max] 里抽 */
  const walk = (room) => {
    const list = (actions.move ?? []).filter(exists);
    if (!list.length || !room) return null;
    const p = pickWeighted(list, rng, s.last);
    const c = clips[p.clip];
    const w = { min: 60, max: 240, lead: 0, tail: 0, pace: 120, ...(c.move ?? {}) };
    let dir = c.dir ?? null;
    if (!dir) {
      const can = ['left', 'right'].filter((d) => (room[d] ?? 0) >= w.min);
      if (!can.length) return null;
      dir = can.length === 2 ? (rng() < 0.5 ? 'left' : 'right') : can[0];
    } else if ((room[dir] ?? 0) < w.min) {
      // 这段只能往一边走：那边没地方就换一段反方向的
      const other = list.find((x) => clips[x.clip].dir && clips[x.clip].dir !== dir && (room[clips[x.clip].dir] ?? 0) >= w.min);
      if (!other) return null;
      return walkWith(other, clips[other.clip].dir, room);
    }
    return walkWith(p, dir, room);
  };

  /** 往指定方向走一段（接着走用） */
  const walkDir = (dir, room) => {
    const list = (actions.move ?? []).filter(exists).filter((p) => !clips[p.clip].dir || clips[p.clip].dir === dir);
    if (!list.length) return null;
    return walkWith(pickWeighted(list, rng, null), dir, room);
  };

  const walkWith = (p, dir, room) => {
    const c = clips[p.clip];
    const w = { min: 60, max: 240, lead: 0, tail: 0, pace: 120, ...(c.move ?? {}) };
    // 循环的走动段按步速走，走多远都自然；一段定长的视频走太远就成了滑行，靠下面“接着走”走远
    if (c.loop) w.max *= reach;
    const dist = Math.min(w.min + rng() * Math.max(0, w.max - w.min), room[dir] ?? 0);
    // 分了左右的段（精灵图）自己就朝那边，不镜像；否则按走的方向定朝向
    if (!c.dir) s.facing = dir;
    s.walked = dir;
    return play(p, LAYER.idle, { mirror: c.dir ? false : mirrored() && c.mirror !== false, move: { dx: dir === 'left' ? -dist : dist, lead: w.lead, tail: w.tail, pace: w.pace, looped: Boolean(c.loop) } });
  };

  const startSleep = (sets) => {
    const set = sets[Math.floor(rng() * sets.length)];
    s.sleep = { set, phase: 'start' };
    return fromList(set.start, LAYER.sleep, { line: 'sleepStart' });
  };

  /** 醒来：有睡醒段先播它 */
  const wake = (line = 'sleepEnd') => {
    if (!s.sleep) return null;
    if (s.sleep.phase === 'end') return null;
    s.sleep.phase = 'end';
    return fromList(s.sleep.set.end, LAYER.sleep, { line });
  };

  /** 按现在的状态接着播 */
  const resume = (t, room) => {
    if (s.dragging) return fromList(actions.drag ?? [], LAYER.drag) ?? null;
    if (PERSIST.includes(s.agent)) {
      const p = pool(s.agent);
      if (!p) return s.agent === 'offline' ? play(null) : idleNext(t, null);
      return fromList(p.list, LAYER.state);
    }
    return idleNext(t, room);
  };

  return {
    get state() {
      return { ...s };
    },
    get facing() {
      return s.facing;
    },
    get sleeping() {
      return Boolean(s.sleep);
    },
    /** 有没有离线时的动画（没有就由播放器淡化成封面） */
    hasOffline: () => Boolean(pool('offline')),

    /** 第一段 */
    start(t, room) {
      s.idleSince = t;
      return resume(t, room);
    },

    /** 当前这段播完了 */
    ended(t, room) {
      const cur = s.cur;
      if (cur?.turn) s.facing = s.facing === 'left' ? 'right' : 'left';
      if (s.sleep) {
        if (s.sleep.phase === 'start') {
          s.sleep.phase = 'loop';
          return fromList(s.sleep.set.loop, LAYER.sleep);
        }
        if (s.sleep.phase === 'loop') {
          // 睡着时 agent 开始干活：醒过来
          if (s.agent !== 'idle') return wake();
          return fromList(s.sleep.set.loop, LAYER.sleep);
        }
        s.sleep = null;
        s.idleSince = t;
      }
      return resume(t, room);
    },

    /** agent 状态变了。`once`：这次要插播的（done / error / toolDone） */
    setAgent(state, once, t, room) {
      const changed = state !== s.agent;
      s.agent = state;
      if (state === 'idle' && changed) s.idleSince = t;
      if (s.dragging) return null;
      if (s.sleep && state !== 'idle') return wake();
      const layer = s.cur?.layer ?? 0;
      if (once && layer <= LAYER.once) {
        const p = pool(once);
        if (p) return fromList(p.list, LAYER.once, { fade: true, line: once });
      }
      if (!changed) return null;
      // 闲时、睡觉时立刻切；同一级的状态等这段播完再换
      if (PERSIST.includes(state) && layer < LAYER.state) {
        const p = pool(state);
        if (p) return fromList(p.list, LAYER.state, { fade: true, line: state });
        if (state === 'offline') return play(null);
      }
      if (state === 'idle' && layer === LAYER.state) return resume(t, room);
      return null;
    },

    /** 插播一段事件动作（testPass、commit、levelUp……）：只在闲着、持续状态里插，不打断点、拖、睡觉 */
    event(key, t) {
      if (s.dragging || s.sleep) return null;
      if ((s.cur?.layer ?? 0) > LAYER.once) return null;
      const p = pool(key);
      if (!p) return null;
      if (key === 'hello') s.idleSince = t;
      return fromList(p.list, LAYER.once, { fade: true });
    },

    /** 被摸头：有专门的段用它，没有用点击的 */
    petted(t) {
      s.idleSince = t;
      if (s.dragging) return null;
      if (s.sleep) return wake('wake');
      const l = (actions.pet ?? []).filter(exists);
      const list = l.length ? l : (actions.click ?? []).filter(exists);
      return list.length ? fromList(list, LAYER.click, { fade: true }) : null;
    },

    /** 连着点太多下，烦了 */
    annoyed(t) {
      s.idleSince = t;
      if (s.dragging) return null;
      if (s.sleep) return wake('wake');
      const l = (actions.annoyed ?? []).filter(exists);
      const list = l.length ? l : (actions.click ?? []).filter(exists);
      return list.length ? fromList(list, LAYER.click, { fade: true }) : null;
    },

    /**
     * 看向一边（鼠标、输入框在那边）：只在闲着时。有转身段播转身段；没有就直接换朝向，
     * 返回 `{ flip: true }` 让播放器把当前这段翻过来（这段不能镜像就不翻）。
     */
    /** 往一边走一段（从另一块屏幕进来时往里走）；忙着、睡着、拎着时不走 */
    walkTo(dir, t, room) {
      if (s.dragging || s.sleep || PERSIST.includes(s.agent)) return null;
      s.idleSince = Math.min(s.idleSince, t);
      return walkDir(dir, room);
    },
    face(dir, t) {
      if (s.dragging || s.sleep || dir === s.facing) return null;
      if ((s.cur?.layer ?? 0) > LAYER.idle || s.cur?.move) return null;
      const turns = (actions.turn ?? []).filter(exists);
      if (turns.length) {
        s.busy = true;
        s.idleSince = Math.min(s.idleSince, t);
        return fromList(turns, LAYER.idle, { turn: true });
      }
      const c = s.cur && clips[s.cur.clip];
      if (c && c.mirror === false) return null;
      s.facing = dir;
      if (s.cur) s.cur.mirror = mirrored();
      return { flip: true, mirror: mirrored() };
    },

    /** 点了一下 */
    click(t) {
      s.idleSince = t;
      if (s.dragging) return null;
      if (s.sleep) return wake('wake');
      const l = (actions.click ?? []).filter(exists);
      if (!l.length) return null;
      return fromList(l, LAYER.click, { fade: true, line: 'click' });
    },

    dragStart(t) {
      s.idleSince = t;
      s.dragging = true;
      s.sleep = null;
      const l = (actions.drag ?? []).filter(exists);
      return l.length ? fromList(l, LAYER.drag, { fade: true }) : null;
    },

    /** 放下了（甩出去落地之后调） */
    dragEnd(t, room) {
      s.dragging = false;
      s.idleSince = t;
      const l = (actions.land ?? []).filter(exists);
      if (l.length) return fromList(l, LAYER.click, { fade: true });
      return resume(t, room);
    },

    /** 现在就睡（右键菜单、试播） */
    sleepNow(t) {
      const sets = (actions.sleep ?? []).filter((x) => x.start?.length && x.loop?.length && x.end?.length);
      if (!sets.length || s.sleep || s.dragging) return null;
      return startSleep(sets);
    },
    hasSleep: () => (actions.sleep ?? []).some((x) => x.start?.length && x.loop?.length && x.end?.length),

    /** 试播：直接播某一段（设置页） */
    force(clip) {
      if (!clips[clip]) return null;
      return play({ clip }, LAYER.drag, { fade: true });
    },
  };
}

/** 气泡台词：按界面语言取，取不到用 zh-CN */
export function pickLine(lines, lang, key, rng = Math.random) {
  if (!lines || !key) return null;
  const l = lines[lang]?.[key] ?? lines[lang?.split('-')[0]]?.[key] ?? lines['zh-CN']?.[key];
  if (!Array.isArray(l) || !l.length) return null;
  return l[Math.floor(rng() * l.length)];
}

/** 清单里的名字、说明：字符串或按语言分的对象 */
export function textOf(v, lang) {
  if (!v) return '';
  if (typeof v === 'string') return v;
  return v[lang] ?? v['zh-CN'] ?? v['en-US'] ?? Object.values(v)[0] ?? '';
}
