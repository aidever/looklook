#!/usr/bin/env python3
"""客户端 GitHub Release 的版本信息（client-linux / client-windows / client-macos / client-release 工作流用）。

    python3 scripts/release-info.py check [--version V]
        版本号是 semver、中英文更新说明都在且不是草稿。
    python3 scripts/release-info.py fragment <linux|windows|macos> [--dist dist] [--out dist/stage]
        把 scripts/package.sh 在 dist/ 里产出的安装包登记成 platform-<平台>.json（文件名、SHA-256、大小、源码指纹），
        标准输出逐行列出要上传的文件（安装包 + 这个 json）。
    python3 scripts/release-info.py merge <放 platform-*.json 的目录> --channel beta|stable
                                     [--min-supported V] [--require-all] [--allow-mixed] [--out 目录]
        合并已构建的平台 → release.json、SHA256SUMS、notes.md（Release 正文）。
        --require-all：三个平台缺一不可、源码指纹必须一致（--allow-mixed 放宽后者），转正式版时用。

每个平台单独构建，各自把安装包和 platform-*.json 传到同一个 GitHub Release（client-v{版本}，先是 Pre-release），
再合并出 release.json（beta 通道，只列已构建的平台）；三个平台都齐了以后 client-release 把它转成正式版（stable）。
看看服务端按 release.json 同步安装包、提供检查更新与下载（某平台取“该通道里有这个平台安装包的最新版本”）。

更新说明必须中英文都有：release-notes/{版本}.zh-CN.md 与 release-notes/{版本}.en-US.md。
"""

import argparse
import datetime
import hashlib
import json
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGETS = ["linux-x86_64", "linux-aarch64", "windows-x86_64", "darwin-x86_64", "darwin-aarch64"]
SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$")
# 每个平台必须产出的文件（后缀相对 looklook-{版本}-）
PLATFORMS = {
    "linux": ["linux-x86_64.tar.gz", "linux-aarch64.tar.gz"],
    "windows": ["windows-x86_64.zip", "windows-x86_64-setup.exe"],
    "macos": ["darwin-aarch64.tar.gz", "darwin-aarch64.dmg"],
}
# 平台 → 它负责的 target
PLATFORM_OF = {"linux": "linux-", "windows": "windows-", "macos": "darwin-"}


def cargo_version() -> str:
    m = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M)
    if not m:
        sys.exit("Cargo.toml 中没有 version")
    return m.group(1)


def check_version(v: str) -> str:
    if not SEMVER.match(v):
        sys.exit(f"版本号不是 semver：{v}")
    return v


def notes(version: str) -> tuple[str, str]:
    """读取中英文更新说明；缺失、为空或还是草稿时退出。"""
    out = []
    for lang in ("zh-CN", "en-US"):
        path = ROOT / "release-notes" / f"{version}.{lang}.md"
        if not path.is_file():
            sys.exit(f"缺少更新说明 {path}（中英文两份都必须有）")
        text = path.read_text(encoding="utf-8").strip()
        if not text:
            sys.exit(f"更新说明 {path} 为空")
        if "<!-- draft" in text:
            sys.exit(f"更新说明 {path} 还是草稿，改好后删掉 <!-- draft … --> 注释")
        out.append(text)
    return out[0], out[1]


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def git(*args: str) -> str:
    try:
        return subprocess.run(["git", *args], cwd=ROOT, check=True, capture_output=True, text=True).stdout
    except (OSError, subprocess.CalledProcessError):
        return ""


def source_fingerprint() -> str:
    """源码指纹：开源仓库里 client/、protocol/、assets/ 的文件树（不含更新说明——改说明不用重新构建）。
    三个平台指纹相同 = 用同一份代码构建；工作流文件等其它改动不影响它。"""
    lines = [
        l for l in git("ls-tree", "-r", "--full-tree", "HEAD").splitlines()
        if "\t" in l and l.split("\t", 1)[1].startswith(("client/", "protocol/", "assets/"))
        and not l.split("\t", 1)[1].startswith("client/release-notes/")
    ]
    return hashlib.sha256("\n".join(lines).encode()).hexdigest()[:16] if lines else ""


def now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def cmd_check(args) -> None:
    v = check_version(args.version or cargo_version())
    notes(v)
    print(v)


def cmd_fragment(args) -> None:
    version = check_version(args.version or cargo_version())
    platform = args.platform
    dist = Path(args.dist)
    prefix = PLATFORM_OF[platform]
    name_re = re.compile(rf"^looklook-{re.escape(version)}-({'|'.join(TARGETS)})(?:\.(tar\.gz|zip|dmg)|-setup\.(exe))$")
    packages, files = [], []
    for f in sorted(dist.iterdir()) if dist.is_dir() else []:
        m = name_re.match(f.name)
        if not m or not f.is_file() or not m.group(1).startswith(prefix):
            continue
        packages.append({
            "target": m.group(1),
            "package": m.group(2) or m.group(3),
            "file": f.name,
            "sha256": sha256(f),
            "size_bytes": f.stat().st_size,
        })
        files.append(f)
    have = {p["file"] for p in packages}
    missing = [s for s in PLATFORMS[platform] if f"looklook-{version}-{s}" not in have]
    if missing:
        sys.exit(f"{dist} 里缺少 {platform} 的安装包：" + "、".join(f"looklook-{version}-{s}" for s in missing))
    run = ""
    if os.environ.get("GITHUB_RUN_ID"):
        run = f"{os.environ.get('GITHUB_SERVER_URL', 'https://github.com')}/{os.environ.get('GITHUB_REPOSITORY', '')}/actions/runs/{os.environ['GITHUB_RUN_ID']}"
    frag = {
        "platform": platform,
        "version": version,
        "commit": os.environ.get("GITHUB_SHA") or git("rev-parse", "HEAD").strip(),
        "source": source_fingerprint(),
        "built_at": now(),
        "run": run,
        "packages": packages,
    }
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"platform-{platform}.json"
    path.write_text(json.dumps(frag, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for p in packages:
        print(f"  {p['target']:<16} {p['package']:<7} {p['size_bytes'] / 1048576:6.1f} MB  {p['file']}", file=sys.stderr)
    for f in files + [path]:
        print(f)


def cmd_merge(args) -> None:
    src = Path(args.dir)
    frags = {}
    for f in sorted(src.glob("platform-*.json")):
        d = json.loads(f.read_text(encoding="utf-8"))
        if d.get("platform") not in PLATFORMS:
            sys.exit(f"{f.name}：未知平台 {d.get('platform')!r}")
        frags[d["platform"]] = d
    if not frags:
        sys.exit(f"{src} 里没有 platform-*.json（还没有任何平台构建完）")
    versions = {d["version"] for d in frags.values()}
    if len(versions) != 1:
        sys.exit("各平台的版本号不一致：" + "，".join(f"{p} {d['version']}" for p, d in frags.items()))
    version = check_version(versions.pop())
    if args.version and args.version != version:
        sys.exit(f"已构建的是 {version}，不是 {args.version}")
    if args.min_supported:
        check_version(args.min_supported)
    notes_zh, notes_en = notes(version)

    missing = [p for p in PLATFORMS if p not in frags]
    sources = {d.get("source", "") for d in frags.values()}
    if args.require_all:
        if missing:
            sys.exit(f"{version} 还差 {'、'.join(missing)} 没构建成功：先运行对应的 client-{missing[0]} 工作流")
        if len(sources) != 1 and not args.allow_mixed:
            sys.exit("各平台不是用同一份源码构建的（"
                     + "，".join(f"{p} {d.get('source') or '?'}@{d.get('commit', '')[:7]}" for p, d in frags.items())
                     + "）：重新构建落后的平台，或确认无妨后加 allow_mixed")

    packages = [p for plat in PLATFORMS if plat in frags for p in frags[plat]["packages"]]
    info = {
        "version": version,
        "channel": args.channel,
        "published_at": now(),
        "min_supported_version": args.min_supported or None,
        "notes_zh": notes_zh,
        "notes_en": notes_en,
        "packages": packages,
    }
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "release.json").write_text(json.dumps(info, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    sums = [f"{p['sha256']}  {p['file']}" for p in packages] + [f"{sha256(out / 'release.json')}  release.json"]
    (out / "SHA256SUMS").write_text("\n".join(sums) + "\n")

    body = []
    if args.channel == "beta":
        built = "、".join(frags)
        rest = "、".join(missing)
        body.append(f"> **Beta**：已构建 {built}" + (f"；还差 {rest}" if rest else "；三个平台都已构建，等待转为正式版")
                    + f"。\n> Built so far: {', '.join(frags)}" + (f"; pending: {', '.join(missing)}" if missing else "; all platforms built") + ".")
        if len(sources) > 1:
            body.append("> ⚠ 各平台的源码不完全一致 / platforms were built from different sources.")
    body.append("### 中文\n\n" + notes_zh)
    body.append("### English\n\n" + notes_en)
    (out / "notes.md").write_text("\n\n".join(body) + "\n", encoding="utf-8")

    for plat, d in frags.items():
        print(f"  {plat:<8} {d.get('commit', '')[:7]}  源码 {d.get('source') or '?'}  {d.get('built_at', '')}  {d.get('run', '')}", file=sys.stderr)
    if missing:
        print(f"  还差：{'、'.join(missing)}", file=sys.stderr)
    print(out)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    c = sub.add_parser("check")
    c.add_argument("--version")
    c.set_defaults(fn=cmd_check)
    f = sub.add_parser("fragment")
    f.add_argument("platform", choices=list(PLATFORMS))
    f.add_argument("--version")
    f.add_argument("--dist", default=str(ROOT / "dist"))
    f.add_argument("--out", default=str(ROOT / "dist" / "stage"))
    f.set_defaults(fn=cmd_fragment)
    m = sub.add_parser("merge")
    m.add_argument("dir")
    m.add_argument("--version")
    m.add_argument("--channel", required=True, choices=["stable", "beta"])
    m.add_argument("--min-supported", default="")
    m.add_argument("--require-all", action="store_true")
    m.add_argument("--allow-mixed", action="store_true")
    m.add_argument("--out", default=str(ROOT / "dist" / "merged"))
    m.set_defaults(fn=cmd_merge)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
