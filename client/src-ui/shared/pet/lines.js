// 宠物台词（docs/PET_PACK.md §3.6）：按键挑一句、填上运行数据。
// 先找包里写的（pet.json 的 lines），再找按人设生成的那批（设置 → 宠物 → 生成台词），最后用内置的通用台词。
// 键和后端 src/pet_lines.rs 的 KEYS 一致。

const ZH = {
  hello: ['今天也一起加油吧~', '你来啦！', '嘿嘿，等你好久了', '新的一天，冲！'],
  morning: ['早上好！喝水了没？', '早呀，今天写点什么？', '早~ 先伸个懒腰'],
  evening: ['晚上好，今天辛苦啦', '晚上也要写代码吗？陪你', '晚饭吃了没~'],
  lateNight: ['这么晚了还不睡吗…', '熬夜会掉头发的哦', '再写一会儿就睡好不好', '夜深了，我陪着你'],
  longSession: ['已经连着干了 {min} 分钟啦，歇会儿？', '起来活动一下吧，{min} 分钟了', '眼睛累不累？看看远处~'],
  sent: ['收到！', '交给它啦', '好嘞~', '在看了在看了'],
  thinking: ['让我想想~', '嗯……', '思考中…'],
  longThink: ['想了 {min} 分钟了，是个难题呢', '它还在想…再等等', '烧脑中，{min} 分钟了'],
  tool: ['忙着呢', '干活干活', '叮叮当当~'],
  edit: ['在改 {file}', '动刀了：{file}', '{file} 要变样了'],
  editMany: ['已经改了 {n} 个文件了！', '大工程啊，{n} 个文件了', '改了 {n} 个，停不下来'],
  search: ['翻箱倒柜中…', '找找看在哪', '在代码里寻宝'],
  web: ['上网查查', '去网上找答案啦'],
  testRun: ['跑测试咯，祈祷中…', '测试开跑！', '紧张紧张…'],
  testPass: ['测试全过！', '绿了绿了！', '{n} 个测试都过啦', '漂亮！'],
  testFail: ['测试挂了…', '红了，别慌', '有测试没过，再来'],
  fixed: ['修好啦！刚才挂的过了', '起死回生！', '好耶，这下绿了'],
  install: ['装依赖中，喝口水吧', '下载下载…', '依赖装起来'],
  build: ['编译中…', '构建一下看看', '等它编完'],
  commit: ['提交啦！', '又一个提交，存档成功', 'commit 完成~'],
  push: ['推上去了！', '发射！', '代码已上天'],
  agentSpawn: ['叫了帮手来', '分身出动！', '派小弟去干活了'],
  errorStreak: ['已经连错 {n} 次了…', '别急，换个思路？', '呜，又失败了'],
  waiting: ['该你啦', '在等你点头哦', '要你确认一下~'],
  danger: ['这条命令有点危险，看仔细哦', '要删东西了，确定吗？', '小心！先看看是什么'],
  terminal: ['要去终端里操作一下', '终端那边在等你'],
  done: ['搞定！', '做完啦~', '完成！', '好了，看看吧'],
  recapQuick: ['{dur} 就搞定了，快吧', '嗖的一下就好了', '秒杀！'],
  recapLong: ['花了 {dur}，终于做完了', '{dur} 的大活儿，辛苦啦', '忙了 {dur}，歇口气'],
  recapBusy: ['改了 {files} 个文件、跑了 {cmds} 条命令', '这轮可真忙：{files} 个文件', '{cmds} 条命令跑完收工'],
  error: ['呜，出错了', '出问题了…', '这下尴尬了'],
  click: ['干嘛~', '嘿！', '戳我做什么', '在呢'],
  petted: ['嘿嘿~', '好舒服', '再摸摸', '喜欢~'],
  annoyed: ['别戳啦！', '好烦哦！', '哼，不理你了', '再点我要生气了'],
  dragged: ['哇，放我下来！', '飞起来了~', '去哪儿呀'],
  thrown: ['啊——', '呜哇！', '飞咯~'],
  sleepStart: ['困了，睡一会儿', '打个盹…', 'Zzz…'],
  sleepEnd: ['我醒啦', '睡饱了！', '伸个懒腰~'],
  wake: ['别吵我……', '唔…再睡五分钟', '谁呀…'],
  levelUp: ['和你更熟了！Lv.{level}', '好感升级 Lv.{level}！', '我们是 Lv.{level} 的好朋友啦'],
};

const EN = {
  hello: ["Let's do great today!", "You're here!", 'Missed you~', 'New day, let’s go!'],
  morning: ['Good morning! Had water?', 'Morning~ what are we building?', 'Morning! Stretch first'],
  evening: ['Good evening, long day?', 'Coding tonight? I’m in', 'Had dinner yet?'],
  lateNight: ['Still up this late…', 'Sleep soon, okay?', 'Night owl mode', 'It’s late, I’m here with you'],
  longSession: ['{min} minutes straight — take a break?', 'Stand up and stretch, {min} min!', 'Rest your eyes a bit~'],
  sent: ['Got it!', 'Passing it on', 'Okay~', 'Watching!'],
  thinking: ['Hmm, let me think~', 'Thinking…', 'Hmm…'],
  longThink: ['{min} minutes thinking — tough one', 'Still thinking… hang on', 'Brain at work, {min} min'],
  tool: ['Busy busy', 'On it', 'Clink clank~'],
  edit: ['Editing {file}', 'Touching {file}', '{file} is changing'],
  editMany: ['{n} files changed already!', 'Big job: {n} files', '{n} files and counting'],
  search: ['Digging around…', 'Where is it…', 'Treasure hunt in the code'],
  web: ['Checking the web', 'Looking it up online'],
  testRun: ['Running tests, fingers crossed…', 'Tests go!', 'So nervous…'],
  testPass: ['All green!', 'Tests passed!', '{n} tests passed', 'Nice!'],
  testFail: ['Tests failed…', 'Red. Don’t panic', 'Something broke, again'],
  fixed: ['Fixed! It passes now', 'Back from the dead!', 'Yay, green again'],
  install: ['Installing deps, grab a drink', 'Downloading…', 'Deps incoming'],
  build: ['Building…', 'Compiling, wait for it', 'Let’s see if it builds'],
  commit: ['Committed!', 'Saved the game', 'Another commit~'],
  push: ['Pushed!', 'Launch!', 'Code is in the cloud'],
  agentSpawn: ['Called for backup', 'Clones deployed!', 'Sent a helper'],
  errorStreak: ['{n} failures in a row…', 'Try another way?', 'Oops, again'],
  waiting: ['Your turn', 'Waiting for your OK', 'Needs your answer~'],
  danger: ['Risky command — check it!', 'It wants to delete stuff. Sure?', 'Careful! Read it first'],
  terminal: ['Needs you in the terminal', 'The terminal is waiting'],
  done: ['Done!', 'All finished~', 'Ta-da!', 'Take a look'],
  recapQuick: ['Done in {dur}, fast!', 'Zoom, done', 'Speedrun!'],
  recapLong: ['Took {dur}, finally done', '{dur} of work, well done', 'Busy {dur}, take a breath'],
  recapBusy: ['{files} files, {cmds} commands', 'Busy round: {files} files', '{cmds} commands later, done'],
  error: ['Oops, an error', 'Something went wrong…', 'Well, that’s awkward'],
  click: ['Hm?', 'Hey!', 'Why the poke?', 'I’m here'],
  petted: ['Hehe~', 'That feels nice', 'More please', 'Love it~'],
  annoyed: ['Stop poking!', 'So annoying!', 'Hmph, ignoring you', 'I’ll get mad!'],
  dragged: ['Put me down!', 'Flying~', 'Where to?'],
  thrown: ['Aaah—', 'Whoa!', 'Wheee~'],
  sleepStart: ['Sleepy… nap time', 'Just a short nap…', 'Zzz…'],
  sleepEnd: ['I’m awake', 'Well rested!', 'Big stretch~'],
  wake: ['Let me sleep……', 'Five more minutes…', 'Who’s there…'],
  levelUp: ['We’re closer now! Lv.{level}', 'Bond up: Lv.{level}!', 'Lv.{level} besties'],
};

export const BUILTIN = { 'zh-CN': ZH, 'en-US': EN };

const pickIn = (src, lang, key) => {
  if (!src || !key) return null;
  const l = src[lang]?.[key] ?? src[lang?.split('-')[0]]?.[key] ?? (lang?.startsWith('en') ? null : src['zh-CN']?.[key]);
  return Array.isArray(l) && l.length ? l : null;
};

/** 把 `{min}` 这样的占位符换成数据；缺数据的句子不要 */
export function fill(text, vars = {}) {
  let missing = false;
  const out = String(text).replace(/\{(\w+)\}/g, (_, k) => {
    const v = vars[k];
    if (v == null || v === '') {
      missing = true;
      return '';
    }
    return String(v);
  });
  return missing ? null : out;
}

/**
 * 挑一句：包里的 → 生成的 → 内置的（包写了 `builtinLines: false` 就不用内置的）。
 * 每一层都先去掉填不上数据的句子，避开刚说过的那句。
 */
export function lineFor({ manifest, gen, lang = 'zh-CN', key, vars, rng = Math.random, last }) {
  const sources = [manifest?.lines, gen];
  if (manifest?.builtinLines !== false) sources.push(BUILTIN);
  const english = lang?.startsWith('en');
  for (const src of sources) {
    const l = pickIn(src, english ? 'en-US' : lang, key) ?? (src === BUILTIN ? null : pickIn(src, lang, key));
    if (!l) continue;
    const ok = l.map((x) => fill(x, vars)).filter(Boolean);
    if (!ok.length) continue;
    const pool = ok.length > 1 ? ok.filter((x) => x !== last) : ok;
    return pool[Math.floor(rng() * pool.length)];
  }
  return null;
}

/** 这个时间点该怎么打招呼 */
export function greetKey(date = new Date()) {
  const h = date.getHours();
  if (h < 5) return 'lateNight';
  if (h < 11) return 'morning';
  if (h >= 18) return 'evening';
  return 'hello';
}
