# 做一只看看对话宠物

这份教程教你从一张角色立绘做出一个宠物包（`.llpet`），装进看看后，它会住在对话界面的角落：
AI 在思考时托腮，跑命令时敲键盘，等你确认时朝你招手，做完了蹦起来，出错了叹气；点它、拖它、甩它都有反应，闲着会自己玩、走来走去、困了睡觉。

格式的完整定义见 [PET_PACK.md](PET_PACK.md)，这里只讲怎么做。

- 不会画画也能做：动作用 AI 视频生成，抠像和转码用 ffmpeg 命令。
- 最少 1 段动画就能装上，7 段就能把所有状态演出来，之后想加多少加多少。
- 想更省事，可以做成精灵图（第 8 节），或者直接用现成的 Codex 宠物、dsh-pet 宠物（不用转换，看看直接认）。

## 0. 先想好授权

宠物包里是角色形象，形象有作者。

- **自己画的、自己设计的**：随你。
- **别人的形象**（同人角色、某个作者的设计）：先看作者的声明。写了“禁止商用”“CC BY-NC”的，只能做来自己用或者免费分享，并保留署名；没写的，最好先问作者。
- **公司的吉祥物、商标**：不要做来公开分发。

在 `pet.json` 的 `license`、`credits` 里如实写清楚（第 6 节），看看会把它显示在宠物卡片上。

## 1. 准备立绘

一张**全身、正面、站姿**的图，后面所有动作都从它出发：

- 背景纯色或透明；人物完整，脚也要在画面里；
- 有三视图（正面 / 侧面 / 背面）更好，AI 生成转身、走路时更不容易走形；
- 把它放到 16:9 的**纯绿底**（`#00FF00`）画布上：人物水平居中，头顶在画面高度约 15%~20% 处，脚底在约 85% 处，左右留足空间（动作会伸手、跳、甩头发）。
  这张图就是之后每段视频的**首帧**，也是宠物的封面。

角色本身带大面积绿色（头发、衣服）时，改用品红底（`#FF00FF`）或蓝底（`#0000FF`），后面抠像命令里的颜色跟着换。

## 2. 列动作清单

先做最少的一套，跑通了再加。名字就是文件名，用小写英文、数字和 `-`。

**最少（7 段）**

| 文件名 | 用在 | 内容建议 | 时长 |
| --- | --- | --- | --- |
| `idle` | 空闲 | 轻轻呼吸、眨眼，几乎不动；要能无缝循环 | 3~4 秒 |
| `think` | 思考、输出 | 托腮、歪头，头顶冒个小气泡 | 4~6 秒 |
| `work` | 调工具 | 低头敲键盘 / 翻书 / 写字 | 4~6 秒 |
| `call` | 等你确认 | 看向镜头招手、踮脚张望 | 3~5 秒 |
| `cheer` | 做完了 | 蹦一下、举手欢呼 | 3~4 秒 |
| `sigh` | 出错了 | 垂头叹气、冒汗 | 3~4 秒 |
| `click` | 被点 | 惊一下然后笑 | 2~3 秒 |

**再加就更好玩**

- 点击多几种：害羞、傲娇生气、挠痒笑、挥手；
- `held`：被拎起来悬空、蹬腿（要能循环）；
- `turn`：东张西望后转向另一边（结尾朝向反过来）；
- `walk`：**原地**走路 / 小跑（看看负责平移，人物别真的走出画面）；
- 闲时小动作：伸懒腰、打哈欠、哼歌、照镜子、吃零食、玩魔方、吹泡泡……越多越不重样；
- 睡觉三段：`sleep-start`（入睡，结尾是睡姿）→ `sleep-loop`（睡着，首尾相同）→ `sleep-end`（醒来，回到站姿）。

**所有段的硬规则**：第一帧和最后一帧都是第 1 节那张标准站姿（`turn` 和睡觉三段按上面说的例外）。这条做到了，任意两段才能直接接上不跳。

## 3. 用 AI 生成视频

用支持**图生视频**的工具（可灵、即梦、Vidu、豆包、海螺、Runway、Veo 等），每个动作生成一段。

- 把第 1 节的绿底立绘作为**首帧**；工具支持“首尾帧”时，**尾帧也放同一张**，这是让动作回到站姿最可靠的办法；
- 画面 16:9，固定机位，不要推拉摇移，不要换背景；
- 一次只生成一个动作，生成不满意就重来，挑最稳的那条。

提示词模板（把“动作”那几行换掉）：

```text
固定机位，正面全身，纯绿色背景（#00FF00），背景全程不变，无阴影无地面。
人物始终在画面正中，大小和位置不变，脚底保持在同一高度，全程不出画。
第 0 秒：标准正面站姿（和首帧一模一样）。
第 1~3 秒：【动作：低头双手在键盘上快速敲打，偶尔点头】
第 3~4 秒：抬头，回到标准正面站姿。
最后一帧：标准正面站姿，和第一帧一模一样。
动作幅度适中，线条干净，不要出现文字、水印和其他人物。
```

常见问题：

- **人物越来越大 / 越来越偏**：提示词里强调“大小和位置不变”，用首尾帧；还是不行就换一条；
- **背景出现阴影、地板、渐变**：抠像会留脏边，重新生成，或在提示词里加“无阴影、无地面、纯色背景”；
- **带水印**：生成工具的水印一般在角落，第 4 步裁掉或者用无水印导出。

## 4. 抠像、转码

装 [ffmpeg](https://ffmpeg.org/download.html)（6.0 以上）。每段视频跑一次（`work.mp4` → `anim/work.webm`）：

```sh
mkdir -p anim
ffmpeg -i work.mp4 -vf "chromakey=0x00FF00:0.15:0.05,despill=type=green,scale=640:360:flags=lanczos,fps=30,format=yuva420p" \
  -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 0 -crf 34 -row-mt 1 -an anim/work.webm
```

- `chromakey=颜色:相似度:平滑`：边缘还留绿就把 `0.15` 调大一点（0.18、0.2），人物身上被抠出洞就调小；
- 品红底用 `0xFF00FF`，`despill` 去掉（它只处理绿和蓝）；蓝底用 `0x0000FF` 和 `despill=type=blue`；
- 结果每段一般 0.2~1.5 MB，超过 8 MB 看看不收，把 `-crf` 调大（38、42）。

批量（macOS / Linux）：

```sh
for f in src/*.mp4; do n=$(basename "$f" .mp4)
  ffmpeg -y -i "$f" -vf "chromakey=0x00FF00:0.15:0.05,despill=type=green,scale=640:360:flags=lanczos,fps=30,format=yuva420p" \
    -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 0 -crf 34 -row-mt 1 -an "anim/$n.webm"
done
```

**苹果设备用的 mov**：Safari、iPhone、看看 macOS 版不支持 webm 的透明通道，需要每段再出一份 HEVC 透明 `.mov`，否则宠物在这些设备上只显示静态封面。
这一步只能在 Mac 上做（用系统自带的编码器）：

```sh
for f in anim/*.webm; do
  ffmpeg -y -c:v libvpx-vp9 -i "$f" -c:v hevc_videotoolbox -allow_sw 1 -alpha_quality 0.75 -tag:v hvc1 -pix_fmt bgra -an "${f%.webm}.mov"
done
```

（`-c:v libvpx-vp9` 要写在 `-i` 前面，ffmpeg 自带的 vp9 解码器会丢掉透明通道。）

> 说明：webm 这条命令看看团队实测过（透明通道、尺寸、首尾帧都对）；mov 这条按 ffmpeg 文档写的，还没在真 Mac 上验证，有问题请反馈。

**封面**：取 `idle` 的第一帧，

```sh
ffmpeg -c:v libvpx-vp9 -i anim/idle.webm -frames:v 1 -vf "scale=512:-1" preview.webp
```

## 5. 检查首尾帧

接不上的段会在切换时“闪一下”。看看自带检查工具：

```sh
looklook pet check ./my-pet
```

它会列出：每段的尺寸、时长、有没有透明通道、首尾帧和标准站姿差多少、`pet.json` 里登记了但缺文件的段、超过大小的文件、苹果设备缺 mov 的段。

不想装看看也可以手动比一段的首尾帧（1.0 = 完全一样，低于 0.95 基本会看出跳）：

```sh
ffmpeg -c:v libvpx-vp9 -i anim/work.webm -frames:v 1 first.png
ffmpeg -sseof -0.1 -c:v libvpx-vp9 -i anim/work.webm -update 1 last.png
ffmpeg -i first.png -i last.png -lavfi ssim -f null - 2>&1 | grep -o 'All:[0-9.]*'
```

## 6. 写 pet.json

目录长这样：

```text
my-pet/
├── pet.json
├── preview.webp
└── anim/
    ├── idle.webm  idle.mov
    ├── think.webm think.mov
    └── …
```

最少能用的 `pet.json`（7 段）：

```jsonc
{
  "format": "looklook-pet",
  "version": 1,
  "id": "orange-girl",
  "name": { "zh-CN": "橙子", "en-US": "Orange" },
  "author": { "name": "你的名字", "url": "https://…" },
  "license": { "spdx": "CC-BY-4.0", "commercial": true, "note": "形象和动画都是我自己做的" },
  "renderer": "video",
  "stage": { "width": 640, "height": 360, "ground": 0.85, "facing": "left" },
  "clips": {
    "idle": { "loop": true },
    "think": {}, "work": {}, "call": {}, "cheer": {}, "sigh": {}, "click": {}
  },
  "states": {
    "idle": ["idle"],
    "thinking": ["think"],
    "tool": ["work"],
    "waiting": ["call"],
    "done": ["cheer"],
    "error": ["sigh"]
  },
  "actions": { "click": ["click"] },
  "lines": {
    "zh-CN": { "waiting": ["该你啦~"], "done": ["搞定！"], "error": ["呜……出错了"] }
  }
}
```

加了更多动作之后，在 `clips` 里登记，再放进对应的池子：

```jsonc
"clips": {
  "look-around": { "turn": true },
  "walk":        { "move": { "min": 60, "max": 240, "lead": 0.5, "tail": 0.5 } },
  "held":        { "loop": true },
  "sign-ok":     { "mirror": false },          // 画面里有字的，翻转后字会反，标上
  "sleep-start": {}, "sleep-loop": { "loop": true }, "sleep-end": {}
},
"actions": {
  "click":  ["click", "click-shy", "click-pout"],
  "drag":   ["held"],
  "turn":   ["look-around"],
  "move":   ["walk"],
  "sleep":  [ { "start": ["sleep-start"], "loop": ["sleep-loop"], "end": ["sleep-end"] } ],
  "random": [
    { "id": "small", "weight": 20, "clips": ["stretch", "yawn", "hum"] },
    { "id": "play",  "weight": 15, "clips": ["rubik", "bubbles"] }
  ]
},
"behavior": { "idle": 40, "turn": 5, "move": 10, "sleepAfter": 600 }
```

用别人的形象时，`license` 和 `credits` 写全，例如：

```jsonc
"license": { "spdx": "CC-BY-NC-SA-4.0", "commercial": false, "url": "https://…", "note": "仅限个人使用" },
"credits": [
  { "role": "原角色", "name": "原作者", "url": "https://…" },
  { "role": "动画", "name": "你的名字" }
]
```

所有字段的含义和缺省值见 [PET_PACK.md](PET_PACK.md) 第 3 节。

## 7. 装进看看试一试

- 把 `my-pet/` 整个文件夹放进看看数据目录下的 `pets/`（AI 助手 → 宠物 → “打开宠物目录”），或者在 AI 助手 → 宠物里点“导入”选 zip；
- AI 助手 → 宠物 里选它，给 Claude / Codex / DSH / OpenCode 分别配一只也行；
- 宠物卡片上的“试播”可以挨个播每个状态和动作，不用等 AI 真的跑起来；
- 改了文件后在设置里点“重新读取”。

## 8. 省事的做法：精灵图

不想做视频，可以做一张 8 列 × 9 行的精灵图（每格 192×208，透明底），和 Codex 宠物同一个规格，按行放这些动作：

| 行 | 动作 | 用几格 |
| --- | --- | --- |
| 0 | 待着 | 6 |
| 1 / 2 | 向右跑 / 向左跑 | 8 |
| 3 | 挥手 | 4 |
| 4 | 跳 | 5 |
| 5 | 失败 | 8 |
| 6 | 等待 | 6 |
| 7 | 干活 | 6 |
| 8 | 检查 | 6 |

文件夹里只放 `pet.json`（Codex 格式：`id`、`displayName`、`description`、`spritesheetPath`）和 `spritesheet.webp`，看看就能认；
想自己定状态对应哪一行，写成本格式的 `"renderer": "sprite"`（见 PET_PACK.md 3.2、3.3）。

从视频均匀取 N 帧拼成一行（不满 8 格的地方保持透明），9 行拼好后再竖着叠：

```sh
N=6; f=anim/idle.webm
T=$(ffprobe -v error -count_frames -select_streams v -show_entries stream=nb_read_frames -of csv=p=0 "$f")
ffmpeg -c:v libvpx-vp9 -i "$f" -vf "select='not(mod(n\,$((T/N))))',crop=ih*192/208:ih,scale=192:208,format=rgba,tile=8x1:color=0x00000000" \
  -frames:v 1 -fps_mode vfr row0.png
# row0.png … row8.png 都做好后：
ffmpeg -i row0.png -i row1.png -i row2.png -i row3.png -i row4.png -i row5.png -i row6.png -i row7.png -i row8.png \
  -filter_complex vstack=inputs=9 -c:v libwebp -lossless 1 spritesheet.webp
```

## 9. 打包分享

```sh
cd my-pet && zip -r ../orange-girl.llpet . && cd ..
```

发给别人时附上授权说明。看看不收录、不分发第三方宠物包；你可以放在自己的 GitHub、网盘或社区里，别人下载后在 AI 助手 → 宠物里导入。

**放在 GitHub 上**，别人可以直接在 AI 助手 → 宠物 → 从 GitHub 安装里填仓库（不用打包）：

```text
# 一个仓库一只：pet.json 在根目录
my-pet/
  pet.json  preview.webp  anim/…  LICENSE  README.md

# 一个仓库好几只：每只一个文件夹（放多深都行），安装时勾选要哪几只
my-pets/
  pets/orange-girl/pet.json …
  pets/blue-cat/pet.json …
  README.md
```

- 填 `owner/repo` 或仓库链接；只想装其中一只，贴那个文件夹的链接（`…/tree/main/pets/blue-cat`）；指定分支或标签写 `owner/repo@v1`。
- 命令行：`looklook pet install owner/repo`，好几只时加 `--pick orange-girl,blue-cat` 或 `--all`。
- dsh-pet 格式的仓库也能直接装（插件本体的 `assets/config.jsonc` + `assets/webm/`，或者自己做的 `<种类>-config.json` + `<种类>-animation/`），只解配置和动画。
- 只能装公开仓库；整个仓库的 zip 不超过 600 MB，每只宠物的限制和导入一样。
- 在 README 和 `pet.json` 的 `license` 里写清楚授权。

## 10. 已有的宠物直接用

- **Codex 宠物**：放在 `~/.codex/pets/` 里的，看看直接列出来；
- **dsh-pet 宠物**：装过 [dsh-pet](https://github.com/PC2005-cloud/dsh-pet) 的，它的默认宠物和 `pet/` 下的种类看看也直接列出来（碎碎念、余额这些 dsh-pet 独有的功能不带过来）。

这些都是只读的，看看不会改、不会复制它们。用之前同样留意各自的素材授权。
