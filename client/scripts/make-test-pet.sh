#!/usr/bin/env bash
# 生成一个测试用的宠物包（色块小人，CC0，docs/PET_PACK.md §7）：每个状态、互动都有一段，首尾帧都是标准站姿。
# 和教程（PET_PACK_GUIDE.md 第 4 节）一样：先画在纯绿底上，再抠像成透明（drawbox 不写透明通道，不能直接画在透明底上）。
# 用法：scripts/make-test-pet.sh <输出目录> [--mov]   （需要 ffmpeg；--mov 只在 macOS 上能用）
set -euo pipefail
out=${1:?用法：make-test-pet.sh <输出目录> [--mov]}
mov=${2:-}
mkdir -p "$out/anim"
W=640 H=360

# 一段动画：$1 名字，$2 秒数，$3 人物水平偏移（像素，t 是秒），$4 竖直偏移，$5 身体颜色，$6 头顶的小方块大小（可空，t 的表达式）
# 人物先画在一块绿底小画布上（drawbox 的 t 是线宽不是时间，不能用来动），再用 overlay 按时间移动（overlay 的 t 是时间），最后抠掉绿色。
clip() {
  local name=$1 d=$2 dx=$3 dy=$4 color=$5 mark=${6:-}
  local ch="color=c=0x00FF00:s=60x192:r=30:d=$d,format=yuv420p"
  ch+=",drawbox=x=5:y=0:w=50:h=50:color=0xffe0c0@1:t=fill"     # 头
  ch+=",drawbox=x=14:y=18:w=8:h=8:color=0x222222@1:t=fill"     # 眼睛
  ch+=",drawbox=x=38:y=18:w=8:h=8:color=0x222222@1:t=fill"
  ch+=",drawbox=x=0:y=52:w=60:h=140:color=$color@1:t=fill"      # 身体
  local fc="$ch[c];[0:v][c]overlay=x='290+($dx)':y='114+($dy)':eval=frame[a]"
  if [ -n "$mark" ]; then
    fc+=";color=c=white:s=30x30:r=30:d=$d[m];[a][m]overlay=x=360:y='70-($mark)':eval=frame[a2]"
  else
    fc+=";[a]null[a2]"
  fi
  fc+=";[a2]chromakey=0x00FF00:0.15:0.05,despill=type=green,format=yuva420p[o]"
  ffmpeg -hide_banner -loglevel error -y -f lavfi -i "color=c=0x00FF00:s=${W}x${H}:r=30:d=$d" \
    -filter_complex "$fc" -map "[o]" -c:v libvpx-vp9 -pix_fmt yuva420p -b:v 0 -crf 40 -row-mt 1 -an "$out/anim/$name.webm"
}

S='sin(2*PI*t/D)'   # 0 → 0 的一个来回（D 换成秒数）
w() { echo "${1//D/$2}"; }

clip idle 3 0 "$(w "3*$S" 3)" 0xe8824a
clip think 4 0 0 0xe8824a "$(w "20*abs($S)" 4)"
clip work 4 "$(w "4*sin(8*PI*t/D)" 4)" 0 0x4a90e8
clip call 3 0 "$(w "-30*abs($S)" 3)" 0xe8c44a "$(w "10*abs(sin(6*PI*t/D))" 3)"
clip cheer 3 0 "$(w "-60*abs($S)" 3)" 0xd94ad9
clip sigh 3 0 "$(w "20*abs($S)" 3)" 0x888888
clip click 2 "$(w "10*$S" 2)" "$(w "-20*abs($S)" 2)" 0xff6fa8
clip walk 3 0 "$(w "-6*abs(sin(6*PI*t/D))" 3)" 0xe8824a
clip look 3 "$(w "8*$S" 3)" 0 0xe8824a
clip held 2 "$(w "8*$S" 2)" 0 0xe8824a
clip stretch 3 0 "$(w "-15*abs($S)" 3)" 0xe8824a
clip sleep-start 3 0 "40*t/3" 0x7a6fe8
clip sleep-loop 3 0 "40+$(w "2*$S" 3)" 0x7a6fe8
clip sleep-end 3 0 "40-40*t/3" 0x7a6fe8

ffmpeg -hide_banner -loglevel error -y -c:v libvpx-vp9 -i "$out/anim/idle.webm" -frames:v 1 -vf "scale=512:-1" "$out/preview.webp"

if [ "$mov" = "--mov" ]; then
  for f in "$out"/anim/*.webm; do
    ffmpeg -hide_banner -loglevel error -y -c:v libvpx-vp9 -i "$f" -c:v hevc_videotoolbox -allow_sw 1 -alpha_quality 0.75 -tag:v hvc1 -pix_fmt bgra -an "${f%.webm}.mov"
  done
fi

cat > "$out/pet.json" <<'JSON'
{
  "format": "looklook-pet",
  "version": 1,
  "id": "test-block",
  "name": { "zh-CN": "色块小人", "en-US": "Block buddy" },
  "description": { "zh-CN": "测试用：scripts/make-test-pet.sh 生成", "en-US": "For testing, made by scripts/make-test-pet.sh" },
  "author": { "name": "Looklook" },
  "license": { "spdx": "CC0-1.0", "commercial": true },
  "renderer": "video",
  "stage": { "width": 640, "height": 360, "ground": 0.85, "facing": "left" },
  "clips": {
    "idle": { "loop": true },
    "think": {}, "work": {}, "call": {}, "cheer": {}, "sigh": {}, "click": {},
    "walk": { "move": { "min": 80, "max": 240, "lead": 0.3, "tail": 0.3 } },
    "look": { "turn": true },
    "held": { "loop": true },
    "stretch": {},
    "sleep-start": {}, "sleep-loop": { "loop": true }, "sleep-end": {}
  },
  "states": {
    "idle": ["idle"],
    "thinking": ["think"],
    "tool": ["work"],
    "waiting": ["call"],
    "done": ["cheer"],
    "error": ["sigh"]
  },
  "actions": {
    "click": ["click"],
    "drag": ["held"],
    "turn": ["look"],
    "move": ["walk"],
    "sleep": [{ "start": ["sleep-start"], "loop": ["sleep-loop"], "end": ["sleep-end"] }],
    "random": [{ "id": "small", "weight": 20, "clips": ["stretch"] }]
  },
  "behavior": { "idle": 40, "turn": 5, "move": 15, "sleepAfter": 120 },
  "lines": {
    "zh-CN": { "thinking": ["让我想想~"], "waiting": ["该你啦"], "done": ["搞定！"], "error": ["呜，出错了"], "click": ["干嘛~"], "sleepStart": ["困了，睡一会儿"], "sleepEnd": ["我醒啦"] },
    "en-US": { "waiting": ["Your turn"], "done": ["Done!"], "error": ["Oops"], "click": ["Hey~"] }
  }
}
JSON
echo "$out"
