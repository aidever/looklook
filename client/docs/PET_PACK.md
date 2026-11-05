# 对话宠物与宠物包格式（设计）

2026-11-03 · 状态：一期、二期已开发（二期 2026-11-04）。制作教程见 [PET_PACK_GUIDE.md](PET_PACK_GUIDE.md)。

结论：对话界面角落住一只会动的宠物，跟着 agent 的状态换动作（思考、调工具、等你确认、做完庆祝、出错叹气），
点它、拖它、甩它都有反应，闲着时自己玩、走来走去、困了睡觉。宠物是**宠物包**：一个文件夹或 zip，里面一份 `pet.json` 加一组透明动画。

- 看看**不自带任何第三方形象**，只认格式；用户自己装，或者自己做（教程面向所有人）。
- 动画有两种：**透明视频**（一个动作一段，好玩，推荐）和 **精灵图**（8×9 格子，省事）。
- 只认本格式（`format: "looklook-pet"`）；别家的宠物格式不兼容、不换算（2026-11-04 去掉了 Codex 宠物和 dsh-pet 的兼容）。

## 1. 原则

本格式只描述**宠物本身**（长什么样、有哪些动作、什么时候播），摆在哪、多大、开不开是看看的设置。
文件名统一用 ASCII，zip 在各系统上解压不乱码。

## 2. 包的结构

```text
claude-girl/                 ← 文件夹名随意，以 pet.json 里的 id 为准
├── pet.json                 ← 清单（必需）
├── preview.webp             ← 封面（必需）：站立正面，透明底，长边 ≤ 512 px
├── anim/                    ← 透明视频（renderer = video）
│   ├── idle.webm            ← VP9 + 透明通道（必需）
│   ├── idle.mov             ← HEVC + 透明通道（可选，苹果设备用，强烈建议）
│   └── …
├── sprite.webp              ← 精灵图（renderer = sprite）
└── LICENSE.txt              ← 可选：完整许可文本
```

分发时把整个文件夹打成 zip，扩展名 `.llpet`（就是普通 zip，`pet.json` 在 zip 根目录或唯一一层子目录里都行）。

限制（导入时检查，超出就拒绝并说明原因）：

| 项 | 上限 |
| --- | --- |
| 整个包 | 200 MB |
| 单个文件 | 8 MB |
| 文件个数 | 400 |
| 文件名 | `[a-z0-9][a-z0-9._-]*`，≤ 64 字符；只能在包里，不许 `..`、绝对路径、符号链接 |
| 文件类型 | `.json` `.webm` `.mov` `.webp` `.png` `.txt` `.md`，其余忽略 |

## 3. pet.json

JSON（允许 `//` 注释和尾逗号，按 JSONC 解析）。下面每个字段都写了是否必需和缺省值。

### 3.1 基本信息

```jsonc
{
  "format": "looklook-pet",          // 必需，固定值
  "version": 1,                      // 必需，格式版本
  "id": "claude-girl",               // 必需，[a-z0-9-]，≤ 40；同 id 再导入 = 覆盖升级
  "name": { "zh-CN": "Claude 娘", "en-US": "Claude-chan" },   // 必需；也可以只写一个字符串
  "description": { "zh-CN": "橙色长发，抱着书的小姑娘" },        // 可选
  "packVersion": "1.0.0",            // 可选，包自己的版本，显示用
  "author": { "name": "某某", "url": "https://…" },            // 必需（至少 name）
  "license": {                       // 必需
    "spdx": "CC-BY-NC-SA-4.0",       // SPDX 标识；不是标准许可就写 "LicenseRef-custom"
    "commercial": false,             // 必需：允不允许商用
    "url": "https://…",              // 可选，许可原文链接
    "note": "形象出自……，仅限个人使用"  // 可选，显示在设置里
  },
  "credits": [                       // 可选：创作链，按先后
    { "role": "原角色", "name": "…", "url": "…" },
    { "role": "动画", "name": "…" }
  ],
  "agents": ["claude"],              // 可选：建议搭配哪种对话（claude / codex / dsh / opencode），选宠物时排在前面
  "renderer": "video"                // 必需：video 或 sprite
}
```

`license` 和 `credits` 会原样显示在设置的宠物卡片上；`commercial: false` 的包卡片上标“非商用”。看看不替任何包判断授权，只如实显示。

### 3.2 画面

```jsonc
"stage": {
  "width": 640, "height": 360,       // 必需：所有动画的画面尺寸（像素），每段都必须一样
  "ground": 0.85,                    // 可选，缺省 0.85：脚底在画面高度的哪个位置（0 顶、1 底）
  "box": [0.38, 0.15, 0.24, 0.72],   // 可选：人物所在的框 [x, y, w, h]，0~1；点击、拖拽、碰边都按它算。缺省按透明像素自动算
  "facing": "left",                  // 可选，缺省 "left"：素材里人物默认朝哪边
  "fps": 30                          // 可选，仅 sprite 有意义的缺省帧率
}
```

视频动画要求（制作时的细节见教程）：

- 人物在每段里**大小、位置一致**，水平居中，脚踩在 `ground` 线上；
- 每段的**第一帧和最后一帧都是“标准站姿”**（和 `idle` 第一帧一样），这样任意两段能直接接上；
- 例外：转身段结尾朝向相反；睡觉的入睡段结尾是睡姿、循环段首尾都是睡姿、醒来段首帧是睡姿；
- `loop: true` 的段首尾两帧必须相同。

精灵图（`renderer: "sprite"`）：

```jsonc
"sprite": {
  "image": "sprite.webp",            // 必需
  "cell": [192, 208],                // 可选，缺省 192×208
  "columns": 8                       // 可选，缺省 8
}
```

此时 `stage.width/height` 等于 `cell`。

### 3.3 动作片段（clips）

`clips` 把每段动画登记一次，后面的状态、互动只用名字引用。名字就是文件名去掉扩展名（`anim/idle.webm` → `idle`）。

```jsonc
"clips": {
  "idle":        { "loop": true },
  "idle-hum":    {},
  "think":       {},
  "type":        {},
  "look-around": { "turn": true },                              // 播完人物朝向反过来
  "walk":        { "move": { "min": 60, "max": 240, "lead": 0.5, "tail": 0.5 } },
  "sign-ok":     { "mirror": false },                           // 画面里有字：人物朝右（被镜像）时不播
  "cheer":       { "title": { "zh-CN": "雀跃庆祝" } }           // 可选：设置里试播列表显示的名字
}
```

| 字段 | 缺省 | 说明 |
| --- | --- | --- |
| `loop` | false | 本段可以无缝循环（首尾帧相同） |
| `turn` | false | 播完朝向反过来 |
| `mirror` | true | 能不能左右翻转着播 |
| `move` | 无 | 这是一段“原地走 / 跑”：播的时候看看把宠物沿地面平移。`min`/`max` 是单次距离（以 `stage.width` = 640 为准，按实际大小缩放），`lead`/`tail` 是开头、结尾原地不动的秒数；`pace`（缺省 120）只对 `loop: true` 的走动段有用：每秒走多少（同样以 640 宽为准），走完距离才算这段结束 |
| `dir` | 无 | `left` / `right`：这段只往这个方向走、不镜像（素材里左右各画了一段时用，比如精灵图里分左右的两行跑步） |
| `speed` | 1 | 播放速度 |
| `row`, `frames`, `fps`, `durations` | — | 仅 sprite：在第几行、用前几格、帧率或每帧毫秒数组 |

视频的时长以文件本身为准（建议 2~10 秒）。没有登记的文件不会被播放；登记了但文件缺失 → 导入时报错。

### 3.4 跟着 agent 走的状态（states）

看看把每个对话的状态归成下面几种，每种配一个**动作池**：字符串数组（等概率），或 `{ "clip": "...", "weight": 3 }` 加权，可混写。

| 状态 | 什么时候 | 怎么播 | 没配时用 |
| --- | --- | --- | --- |
| `idle` | 空闲 | 进入“闲时”（§3.5） | **必需** |
| `thinking` | 一轮开始、模型在想、在输出文字 | 持续：一段播完再抽一段 | `idle` |
| `tool` | 在调工具（跑命令、改文件……） | 持续 | `thinking` |
| `toolDone` | 一个工具刚跑完 | 播一次，回到 `thinking`/`tool` | 不播 |
| `waiting` | 等你批准、回答问题、看计划 | 持续，直到你处理 | `idle` |
| `terminal` | 要到终端里操作（TUI 上的对话框） | 持续 | `waiting` |
| `done` | 一轮正常结束 | 播一次，回到 `idle` | 不播 |
| `error` | 一轮出错 / 被中断 / 额度用完 | 播一次，回到 `idle` | 不播 |
| `offline` | 没连上（agent 没在跑） | 持续；没配就淡化成半透明封面 | — |

```jsonc
"states": {
  "idle":     ["idle"],
  "thinking": ["think", "think-bubble"],
  "tool":     ["type", { "clip": "tidy", "weight": 2 }],
  "toolDone": ["nod"],
  "waiting":  ["look-at-you", "tap-foot"],
  "done":     ["cheer"],
  "error":    ["sigh"]
}
```

#### 3.4.1 看得懂在干什么（可选）

看看从对话里认出一些事（`src-ui/shared/pet/events.js`），会说一句台词（§3.6），下面这几种还会插播一段动作。
都不是必需的，没配就借 `done` / `error` 的动作，再没有就只说话：

| 状态 | 什么时候 | 没配时用 |
| --- | --- | --- |
| `testPass` | 跑测试的命令成功了（含“之前挂的测试修好了”） | `done` |
| `testFail` | 跑测试的命令失败了 | `error` |
| `commit` | `git commit` / `git push` 成功了 | `done` |
| `hello` | 今天第一次见面打招呼 | `done` |
| `levelUp` | 好感度升级（§3.7） | `done` |

只说话、不插播动作的：发出消息、想了很久、改文件 / 改了很多文件、搜索、上网、装依赖、编译、派子代理、连着出错、
等你批准危险命令（删文件、强推、`reset --hard`……）、一轮做完的总结（很快 / 很久 / 很忙）、深夜、连续干太久。

**最小可用包**只要 `idle` 一段；7 段（idle、thinking、tool、waiting、done、error、click）就能把所有状态演出来。

### 3.5 互动和闲时（actions、behavior）

```jsonc
"actions": {
  "click":  ["click-happy", "click-shy", "click-pout"],   // 点一下
  "pet":    ["happy"],                                    // 可选：被摸头（鼠标在身上来回蹭）；没配用 click
  "annoyed": ["pout"],                                    // 可选：2.5 秒里连点 5 下以上，烦了；没配用 click
  "drag":   ["held"],                                     // 被拎起来悬空（应能 loop）
  "land":   ["land"],                                     // 可选：被甩出去落地
  "turn":   ["look-around"],                              // 转身（每项都必须 turn: true）
  "move":   ["walk", "run"],                              // 走动（每项都必须有 move）
  "sleep":  [ { "start": ["sleep-start"], "loop": ["sleep-loop"], "end": ["sleep-end"] } ],
  "random": [                                             // 闲时随机动作，按分组权重抽
    { "id": "small", "weight": 20, "clips": ["stretch", "yawn", "hum"] },
    { "id": "play",  "weight": 15, "clips": ["rubik", "bubbles"] }
  ]
},
"behavior": {
  "idle": 40, "turn": 5, "move": 10,                      // 和 random 各组 weight 一起按比例抽，不要求加起来 100
  "sleepAfter": 600                                       // 闲多少秒后自己睡，缺省 600；0 = 不自己睡
}
```

`sleep` 是“套”的数组：入睡时整套随机挑一套，三段都从这套里取，避免“蹲着睡下、趴着醒来”。

### 3.6 气泡台词（lines，可选）

```jsonc
"lines": {
  "zh-CN": {
    "thinking": ["让我想想~"], "tool": ["忙着呢"], "waiting": ["该你啦"],
    "done": ["搞定！"], "error": ["呜，出错了"], "click": ["干嘛~"],
    "sleepStart": ["困了，睡一会儿"], "sleepEnd": ["我醒啦"], "wake": ["别吵我……"]
  },
  "en-US": { "done": ["Done!"] }
}
```

进入对应状态或动作时随机抽一句，头顶冒 3~6 秒气泡。按界面语言取，取不到用 `zh-CN`（英文界面不退回中文）。

台词从三个地方找，前面的优先：

1. 包里写的 `lines`；
2. 按人设生成的一批（设置 → 宠物 → 台词 → 生成：让模型照 `persona` 一次写一批存起来，之后只是挑一句，不再花 token）；
3. 看看内置的通用台词（`src-ui/shared/pet/lines.js`）。包里写 `"builtinLines": false` 就不用内置的。

```jsonc
"persona": { "zh-CN": "傲娇的猫娘，嘴上嫌弃主人写的代码，其实很关心；句尾爱带“喵”" },   // 可选
"builtinLines": true                                                                   // 可选，缺省 true
```

台词键（全部见 `src/pet_lines.rs` 的 `KEYS`）：除了上面那些，还有
`hello` `morning` `evening` `lateNight` `longSession` `sent` `longThink` `edit` `editMany` `search` `web` `testRun` `testPass` `testFail` `fixed`
`install` `build` `commit` `push` `agentSpawn` `errorStreak` `danger` `recapQuick` `recapLong` `recapBusy` `petted` `annoyed` `dragged` `thrown` `levelUp`。
句子里可以用占位符，数据填不上的句子不会被选：`{min}`（分钟）、`{n}`（个数）、`{file}`（文件名）、`{dur}`（用时，如 `6m 12s`）、
`{files}`、`{cmds}`（这一轮改了几个文件、跑了几条命令）、`{level}`（好感等级）。

“碎碎念”（设置里的开关，默认开）管的是 §3.4.1 那些按运行数据说的话；关掉后只在状态变化、点它时说话。

### 3.7 好感度

和每只宠物的好感（经验）存在这台电脑上（`pet.bond`，按 `source:id`），手机、电脑打开都算：
摸头 +3、点一下 +1（3 秒内连点只算一次）、陪你做完一轮 +2、测试通过 +1、修好挂掉的测试 +3、提交 +2、每天第一次见面 +5。
等级：Lv2 20、Lv3 60、Lv4 150、Lv5 300、Lv6 600、Lv7 1000，之后每级 +600。升级时插播 `levelUp`、头顶冒心。
设置的宠物卡片上显示等级、一起过了几天、做完几轮、摸了几次头。

## 4. 播放规则

**优先级**（高的打断低的）：拖拽 > 点击 > 一次性状态（`done` / `error` / `toolDone`）> 持续状态（`thinking` / `tool` / `waiting` / `terminal`）> 睡觉 > 闲时。

- 高优先级来了**立即切**，两段之间 150 ms 交叉淡入淡出（素材首尾都是站姿，跳变不明显）；同一优先级里换池子（`thinking` → `tool`）等当前这段播完再换，不抽搐。
- 抽片段时避开刚播过的那段（池子里只有一段除外）。
- `done` 只在“跑着 → 空闲”时播；刷新页面、刚打开对话时不补播。
- 睡着时 agent 开始干活 → 播醒来段再进状态；睡着时点它 = 叫醒。
- 朝向：走动方向决定朝向，`mirror: false` 的段在朝右时跳过（池子里全是这种就不翻转直接播）。
- `move`：只沿对话区底边走，不出界；走到边上就掉头。
- 拖拽：手指或鼠标按住拖，**拖到哪停在哪**（网页里是整个页面，桌面宠物是整个屏幕）；松手时有速度就甩出去，滑一段、碰边反弹后停下（不往下掉）。位置记在浏览器里。
- 摸头：鼠标没按着、在身上左右来回蹭（1.2 秒里换向 4 次）。连点：2.5 秒里 5 下烦了，8 下以上跳开。
- 看人：闲着时鼠标在另一边停 1.5 秒，转过去看（有 `turn` 段播转身，没有就直接翻过来）；在输入框里打字时看向输入框。
- 头顶的小特效（心、怒、闪光、汗、Zzz……）是看看画的，不进包。
- 系统开了“减少动态效果”：只显示封面，状态变化只换气泡。
- 页面在后台（`document.hidden`）时暂停播放。

## 5. 别家格式

不兼容。`pet.json` 的 `format` 不是 `looklook-pet` 的一律报 `pet_format`（“不是看看的宠物包”），扫描、导入、从 GitHub 装都一样。

## 6. 看看这边怎么实现

### 6.1 去哪找宠物

| 来源 id | 目录 | 能删 |
| --- | --- | --- |
| `local` | `{LOOKLOOK_HOME}/pets/<id>/`（导入的、从 GitHub 装的都放这） | 能 |

启动时和打开设置页时扫一次，结果缓存；单个包出错只跳过它，错误显示在设置里。
`{source}:{id}` 是宠物的全局标识（现在只有 `local`）。

### 6.2 接口（客户端，`src/pets.rs`）

- `GET /api/pets` → `[{ key, source, id, name, author, license, renderer, preview, agents, error? }]`
- `GET /api/pets/{source}/{id}` → 完整清单（第 3 节的模型，文件换成 URL）
- `GET /api/pets/{source}/{id}/file/{name}` → 片段 / 封面 / 精灵图。支持 Range（视频要），`ETag` + `Cache-Control: max-age=31536000, immutable`（URL 带内容哈希）。
  路径只认清单里登记过的文件名，其余 404。
- `POST /api/pets/import`（multipart，`.llpet` / `.zip`）→ 解压到临时目录 → 校验（§2 限制、§3 字段、片段都在、视频能读出尺寸）→ 原子改名进 `pets/<id>/`；同 id 覆盖。
- `DELETE /api/pets/local/{id}`
- 从 GitHub 装（`src/pet_github.rs`）：
  - `POST /api/pets/from-github {url}`：认 `owner/repo`、`owner/repo@ref`、仓库链接、`/tree/<ref>/<文件夹>` 链接 → 下载 `github.com/<owner>/<repo>/archive/<ref|HEAD>.zip`（≤ 600 MB）→
    找出所有宠物（套在别的宠物文件夹里的、`.` 开头的、`node_modules` 里的不算；最多 60 只）→ 每只只解它自己的文件到 `pets/.gh-<令牌>/p/<序号>/`，按导入一样校验 →
    返回 `{token, repo, items: [摘要 + path + index]}`，坏的也列出来写原因。仓库里别的文件不解。
    只认带 `pet.json` 的文件夹，且要是本格式。看看只是替用户从原仓库下载，不托管、不推荐第三方形象。
  - `GET /api/pets/from-github/preview/{token}/{index}`：挑的时候看封面。
  - `POST /api/pets/from-github/install {token, indexes}`：装挑中的，同 id 覆盖；挑的里面 id 重复 → `dup_id`。装完删临时目录；没装的 1 小时后下次查找时清掉。
  - 装好的包里写 `.origin.json`（`{github, ref, path}`），列表摘要带 `origin`，卡片上显示“来自 GitHub”并链回仓库。
- 命令行：`looklook pet check <目录>`（制作时自查，见教程）、`looklook pet install <文件 | 文件夹 | GitHub 仓库> [--pick id,… | --all]`
  （仓库里只有一只直接装；好几只又没写 `--pick` / `--all` 时列出来、退出码 2）。

远程（中转 / WebRTC）打开时文件也从这台电脑拿；片段按需加载，一个状态只先拿池子里抽中的那段。

### 6.2.1 好感度、台词、桌面宠物的接口

- `POST /api/pets/{source}/{id}/bond {add: {xp, clicks, pets, turns, tests, fixes, commits, errors}, day: "YYYY-MM-DD"}` → 累计的好感；`day`（页面的本地日期）换了一天时 `days + 1`、返回 `newDay: true`（页面打招呼）。
- `GET /api/pets/{source}/{id}/lines` → 生成的台词 `{ "zh-CN": { 键: [句子] }, at, via }`；`POST … {lang, via: auto|claude|codex|api}` 生成（`src/pet_lines.rs`：
  OpenAI 兼容接口，或本机 `claude -p --model haiku`、`codex exec`，在空的临时目录里跑，3 分钟超时）；`DELETE …` 删掉。
- `GET /api/desktop/pet` → `{running, focus}`；`PUT /api/desktop/pet {hit, grab, focus}`（只认本机）。见 §6.5。

### 6.3 设置

存在 `Settings`（`src/settings.rs`）里：

```jsonc
"pet": {
  "enabled": true,
  "byAgent": { "claude": "local:claude-girl", "codex": "local:robot", "dsh": null },
  "default": null,            // 上面没配的对话类型用哪只
  "size": 1,                  // 缩放，1 = 桌面 130 px 高、手机 96 px 高
  "lines": true,              // 显示气泡
  "chatter": true,            // 碎碎念（§3.6）
  "desktop": true,            // 桌面应用里宠物住在桌面上（§6.5）
  "api": { "url": "https://api.deepseek.com/v1", "model": "deepseek-chat", "key": "…" }   // 可选：生成台词用；页面只看到 keySet
}
```

浏览器各自记“这只藏起来了”（localStorage），和以前的小胖鱼一样。

### 6.3.1 从其他电脑导入

同一账户有多台电脑时，设置 → 宠物下面多一张“从其他电脑导入”：点开一台电脑列出它导入过的宠物（`local` 的），点“导入”才开始。

- 导出方（A）：`GET /api/pets/local/{id}/export`，把 `pets/<id>/` 打成不压缩的 zip（不带隐藏文件、不跟符号链接）流式发出；
- 导入方（B，界面当前选中的电脑）：`POST /api/pets/from-device {device, id}` 马上返回，B 的后端经 `rtc::outbound` 向 A 取包，
  存成临时文件后按普通导入校验、装好（同 id 覆盖），`.origin.json` 记 `{device}`；`GET /api/pets/from-device` 看进度
  （进行中的 + 10 分钟内做完的），界面每秒轮询，做完弹提示。离开页面不影响。
- A 是旧版本（没有这个接口）时报 `PET_PEER_OLD`。实现在 `src/pet_transfer.rs`。

### 6.4 前端

- `shared/pet/` 下：`manifest.js`（取清单、选格式）、`brain.js`（第 4 节的状态机，纯函数，`node --test` 测）、`ChatPet.jsx`（渲染）。
- 视频：两个 `<video muted playsinline>` 叠着，一个在播、一个预载下一段，`ended` 时互换。
- 格式：能播 VP9 透明的（Chromium、Edge WebView2、安卓）用 webm；Safari / iOS / macOS App（WKWebView）用 mov；苹果设备上包里没有 mov → 显示封面加气泡，设置卡片上提示“这个包在苹果设备上不会动”。
- 精灵图：一个 `<div>` 用 `background-position` 按帧切。
- 状态来源就是对话页已有的 `status`：`phase`（`running` / `approval` / `tui` / `idle` / `detached`）、`tool`、`error`、`pending.kind`。
  `running` 且有 `tool` → `tool`；`tool` 从有变没 → `toolDone`；`running → idle` 且没有 `error` → `done`，有 → `error`。

### 6.5 桌面宠物（桌面应用）

Windows / macOS 桌面应用（`src/desktop.rs`）开一个铺满主屏工作区的透明、无边框、置顶、不进任务栏的窗口（标签 `pet`），
加载 `/pet-desk`（`src-ui/pages/DeskPet.jsx`，只放宠物）。应用最小化、主窗口关掉（隐藏）时它都在，能拖到屏幕任何地方。

- 鼠标穿透：页面每次宠物动了把人物的框报给 `PUT /api/desktop/pet`（`src/desk_pet.rs` 记着），桌面这边每 40 ms 看一次光标在不在框里，
  在就让窗口接鼠标，不在就 `set_ignore_cursor_events(true)` 穿透到下面的窗口；拖着时（`grab`）一直接。
- 跟哪个对话：主窗口的对话页把正在看的对话报成 `focus`，宠物窗口每 2 秒看一次、跟着换；还没打开过对话时用默认宠物待着。
- 开关：设置里的“桌面宠物”（只在桌面应用里显示），改了通过 `desk_pet::set_wanted` 通知桌面那边开、关窗口；开着时主窗口的对话页不再放宠物。
- macOS 透明窗口要 Tauri 的 `macos-private-api`（Cargo 特性 + `tauri.conf.json` 的 `macOSPrivateApi`）。

### 6.6 分期

1. 第一期：格式、导入、视频 + 精灵图播放、状态联动、点击、拖拽甩抛、闲时随机 / 走动 / 睡觉、气泡、`looklook pet check`。
2. 第二期（2026-11-04）：看得懂在干什么、碎碎念（按运行数据套台词，不花 token；按 `persona` 一次性生成一批台词）、
   摸头 / 连点 / 看人、整页随意拖放、好感度、桌面宠物窗口。
3. 第三期：做包的辅助（从立绘出提示词、批量抠像脚本）。

## 7. 许可

- 看看的代码（含播放器）MIT。
- 看看发布包和开源仓库里**不放任何第三方宠物素材**。测试用的宠物包由脚本生成（色块小人），CC0。
- 用户装的包授权归各自作者；看看只显示 `license` / `credits`，不做判断。
