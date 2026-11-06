#!/usr/bin/env bash
# 客户端分平台发布的 GitHub 端操作（公开仓库的工作流调用；需要 gh、GH_TOKEN，仓库取 GH_REPO / GITHUB_REPOSITORY）。
#
#   scripts/release-stage.sh preflight
#       构建前检查：版本号与更新说明；这个版本已经是正式版时失败（正式版不能覆盖，先提高版本号）。
#   scripts/release-stage.sh upload <linux|windows|macos>
#       把 dist/ 里这个平台的安装包和 platform-<平台>.json 传到 Release client-v{版本}（没有就建一个 Pre-release）。
#       重新构建同一平台时覆盖它自己的文件，不影响其它平台。
#   scripts/release-stage.sh merge
#       按已上传的 platform-*.json 重新生成 release.json（beta 通道，只列已构建的平台）、SHA256SUMS 和 Release 正文。
#       工作流里放在 concurrency 组 client-stage 里串行执行；生成后再核对一遍，期间有别的平台传上来就重做。
#   scripts/release-stage.sh promote <版本> [最低支持版本] [allow_mixed]
#       三个平台齐了、源码一致 → release.json 改成 stable 通道、去掉 Pre-release 标记并设为 Latest。不重新构建。
#
# 看看服务端每 5 分钟同步一次非草稿的 client-v* Release，release.json 变了就重新下载这个版本；
# 某平台的更新与下载取“该通道里有这个平台安装包的最新版本”，所以 beta 阶段缺平台不影响正式版用户。
set -euo pipefail
cd "$(dirname "$0")/.."

export GH_REPO=${GH_REPO:-${GITHUB_REPOSITORY:?需要 GH_REPO 或 GITHUB_REPOSITORY}}
die() { echo "::error::$*" >&2; exit 1; }
version() { python3 scripts/release-info.py check "$@"; }
title() { echo "看看电脑端 $1 · Looklook desktop"; }

# Release 状态：none / beta（Pre-release）/ stable / draft
state() {
  local s
  s=$(gh api "repos/$GH_REPO/releases/tags/client-v$1" --jq 'if .draft then "draft" elif .prerelease then "beta" else "stable" end' 2>/dev/null) || s=none
  echo "$s"
}

# 已上传的 platform-*.json：名字与资源 id（重新上传会换 id），用来判断合并期间有没有变化
frag_ids() {
  gh api "repos/$GH_REPO/releases/tags/client-v$1" \
    --jq '[.assets[] | select(.name | test("^platform-[a-z]+\\.json$")) | "\(.name):\(.id)"] | sort | join(" ")'
}

summary() { [[ -n ${GITHUB_STEP_SUMMARY:-} ]] && cat >>"$GITHUB_STEP_SUMMARY" || cat >&2; }

cmd_preflight() {
  local v s
  v=$(version)
  s=$(state "$v")
  case $s in
    stable) die "client-v$v 已经是正式版：先提高 client/Cargo.toml 的版本号" ;;
    draft) die "client-v$v 是草稿 Release：先在 GitHub 上删掉它" ;;
    beta) echo "client-v$v 已有 beta，本平台的安装包会被覆盖" ;;
    none) echo "client-v$v 还没有 Release，构建完会新建一个 Pre-release" ;;
  esac
  echo "构建 $v（现在：$s）" | summary
}

cmd_upload() {
  local platform=${1:?平台} v s
  v=$(version)
  local list files=() f
  list=$(python3 scripts/release-info.py fragment "$platform")
  while IFS= read -r f; do [[ -n $f ]] && files+=("$f"); done <<<"$list"
  (( ${#files[@]} > 1 )) || die "没有 $platform 的安装包"
  s=$(state "$v")
  case $s in
    stable) die "client-v$v 已经是正式版，不能覆盖" ;;
    draft) die "client-v$v 是草稿 Release：先在 GitHub 上删掉它" ;;
    none)
      # 几个平台可能同时到这一步：建不成时再看一眼，别人建好了就用别人的
      gh release create "client-v$v" --prerelease --target "${GITHUB_SHA:-main}" --title "$(title "$v")" \
        --notes "Beta：构建中 / building…" >/dev/null || [[ $(state "$v") == beta ]] || die "建不了 Release client-v$v"
      ;;
  esac
  gh release upload "client-v$v" "${files[@]}" --clobber
  echo "已上传 $platform：${#files[@]} 个文件 → client-v$v" | summary
}

# merge_to <版本> <通道> [release-info merge 的其它参数…]：取回 platform-*.json → 生成 → 上传 → 核对，有变化就重做
merge_to() {
  local v=$1 channel=$2; shift 2
  local tmp before after i
  tmp=$(mktemp -d)
  for i in 1 2 3 4 5 6; do
    before=$(frag_ids "$v")
    rm -rf "$tmp/frag" "$tmp/out"
    gh release download "client-v$v" --pattern 'platform-*.json' --dir "$tmp/frag"
    python3 scripts/release-info.py merge "$tmp/frag" --version "$v" --channel "$channel" --out "$tmp/out" "$@" >/dev/null
    gh release upload "client-v$v" "$tmp/out/release.json" "$tmp/out/SHA256SUMS" --clobber
    gh release edit "client-v$v" --title "$(title "$v")" --notes-file "$tmp/out/notes.md" >/dev/null
    after=$(frag_ids "$v")
    [[ $before == "$after" ]] && break
    echo "合并期间有平台更新了安装包，重新合并（第 $i 次）" >&2
    [[ $i == 6 ]] && die "平台一直在更新，合并没有稳定下来；稍后重新运行"
  done
  { echo '```json'; cat "$tmp/out/release.json"; echo '```'; } | summary
  rm -rf "$tmp"
}

cmd_merge() {
  local v s
  v=$(version)
  s=$(state "$v")
  [[ $s == beta ]] || die "client-v$v 现在是 $s，不是 beta（已转正式版的不再合并）"
  merge_to "$v" beta
}

cmd_promote() {
  local v=${1:?版本} min=${2:-} mixed=${3:-} s
  version --version "$v" >/dev/null
  s=$(state "$v")
  case $s in
    stable) echo "client-v$v 已经是正式版" | summary; exit 0 ;;
    beta) ;;
    *) die "client-v$v 还没有 beta Release（$s）：先构建三个平台" ;;
  esac
  local extra=(--require-all)
  [[ -n $min ]] && extra+=(--min-supported "$min")
  [[ $mixed == true || $mixed == allow_mixed ]] && extra+=(--allow-mixed)
  merge_to "$v" stable "${extra[@]}"
  gh release edit "client-v$v" --prerelease=false --latest >/dev/null
  echo "client-v$v 已转为正式版" | summary
}

case ${1:-} in
  preflight) shift; cmd_preflight "$@" ;;
  upload) shift; cmd_upload "$@" ;;
  merge) shift; cmd_merge "$@" ;;
  promote) shift; cmd_promote "$@" ;;
  *) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
