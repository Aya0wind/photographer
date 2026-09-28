#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""存量相册物理布局迁移：`{dir_name}/{YYYY}/{MM-DD}` → `{创建YYYY}/{创建MM}/{dir_name}`（相册内平铺）。

布局定案 2026-09-28（与 Rust 侧唯一公式 crate::db::album_home_rel_parts 同语义）：
    photoRoot/{相册 created_at:YYYY}/{相册 created_at:MM}/{dir_name}/照片平铺
外层两段 = 相册**创建时间**（UTC 口径，album.created_at 原值）的字面量段；
相册内不再有拍摄日分层（按天分组在应用 UI 完成）。布局已固定不可配置
（库级 dirTemplate/importSubdir 配置随本次改版退役）。

对每个相册：
1. 旧目录 photoRoot/{dir_name} 不在盘 → 幂等跳过（从未物化或已迁移）。
2. 在盘 → 建 photoRoot/{创建YYYY}/{创建MM}/{dir_name}/，把旧目录下
   {YYYY}/{MM-DD}/** 全部**提升平铺**挪进去（同卷 rename 毫秒级；失败回退
   copy+xxhash 校验+删源——与 claim 挪移同款语义）。重名冲突追加 " (2)"。
3. assets.path 前缀改写：旧前缀 photoRoot/{dir_name}/（库内存在 `\` 与 `/`
   两种分隔符形态，均处理）→ 新前缀；冲突改名的文件按精确路径二次修正。
4. 清空旧目录内层空目录；旧目录空则删除。

注意：
- **不动** photoRoot/SmartPhoto 旧日期根（非相册归属——SmartPhoto 不在
  album.dir_name 之列，脚本只按 album 表逐册处理）。
- 缩略图缓存键 = (path, mtime)：path 变更后全量 miss，应用下次访问自动
  重生成（无需手工清理，仅提示耗时）。
- 历史 journal（job_files.dst）保持旧值不动：dirTemplate 已退役，恢复
  任务按当前公式重渲染落位。
- 应用必须在关闭状态运行本脚本（SQLite 独占写 + 文件挪移）。

用法：
    python scripts/migrate_album_layout.py "I:\\SmartPhoto\\主库\\library.db" --photo-root "Y:\\照片"          # 预览（dry-run）
    python scripts/migrate_album_layout.py "I:\\SmartPhoto\\主库\\library.db" --photo-root "Y:\\照片" --apply  # 执行
    python scripts/migrate_album_layout.py --selftest                                                          # 纯函数表驱动自检
"""

import argparse
import hashlib
import os
import shutil
import sqlite3
import sys
from datetime import datetime, timezone
from pathlib import Path

BS = chr(92)  # 反斜杠（避免源码里转义混乱）


# ---------------------------------------------------------------------------
# 纯函数（公式唯一镜像；Rust 侧 album_home_rel_parts 为权威）
# ---------------------------------------------------------------------------

def new_home_rel(created_at: str, dir_name: str) -> str:
    """created_at（RFC3339，UTC 口径）+ dir_name → `{YYYY}/{MM}/{dir_name}`。

    解析失败兜底 dir_name 直挂根（与 Rust 侧 unwrap_or_else 同语义）。
    """
    try:
        t = datetime.fromisoformat(created_at.replace("Z", "+00:00"))
        if t.tzinfo is None:
            t = t.replace(tzinfo=timezone.utc)
        return f"{t.year:04d}/{t.month:02d}/{dir_name}"
    except (ValueError, TypeError):
        return dir_name


def _sep_forms(p: str) -> set[str]:
    r"""同一目录的 `\` / `/` 两种分隔符形态。"""
    return {p, p.replace("/", BS), p.replace(BS, "/")}


def old_prefix_variants(photo_root: str, dir_name: str) -> list[str]:
    """旧布局资产前缀（含尾分隔符）候选：photoRoot{S}{dir_name}{S2}，
    S/S2 ∈ {`\\`, `/`}——库内 path 同时存在两种形态（引擎 render_dir 产物
    段内是 `/`，claim 挪移 join 产物是 `\\`）。"""
    variants = []
    for root in _sep_forms(photo_root):
        for s1 in (BS, "/"):
            for s2 in (BS, "/"):
                v = f"{root}{s1}{dir_name}{s2}"
                if v not in variants:
                    variants.append(v)
    return variants


def map_path(path: str, variants: list[str], new_dir_no_sep: str, fixups: dict[str, str] | None = None) -> str | None:
    """旧前缀命中 → 新**平铺**路径：`photoRoot/{创建YYYY}/{创建MM}/{dir_name}/
    {文件名}`——旧内层 `{YYYY}/{MM-DD}/` 段丢弃，只保留最后一段文件名
    （平铺语义；与盘上搬移一致）。fixups = 平铺冲突改名表（旧名 → 新名，
    claim 约定 ` (2)` 后缀）。不命中返回 None。"""
    for v in variants:
        if path.startswith(v):
            fname = path[len(v):].replace(BS, "/").rsplit("/", 1)[-1]
            fname = (fixups or {}).get(fname, fname)
            return f"{new_dir_no_sep}{BS}{fname}"
    return None


def unique_name(fname: str, taken: set[str]) -> str:
    """内存占用表内的唯一名（` (2)`、` (3)`… 后缀，claim 约定）。"""
    if fname not in taken:
        return fname
    stem, dot, ext = fname.rpartition(".")
    if not dot or not stem:
        stem, ext = fname, ""
    else:
        ext = f".{ext}"
    n = 2
    while True:
        cand = f"{stem} ({n}){ext}"
        if cand not in taken:
            return cand
        n += 1


def _selftest() -> None:
    """纯函数表驱动自检（不触文件系统）。"""
    # 公式：外层=相册创建年月（UTC 口径），相册内平铺
    assert new_home_rel("2026-09-27T05:09:56.381Z", "album-1") == "2026/09/album-1"
    assert new_home_rel("2025-12-31T23:59:59.999Z", "婚礼") == "2025/12/婚礼"
    assert new_home_rel("2026-01-01T00:00:00.000+00:00", "a") == "2026/01/a"
    assert new_home_rel("2026-03-05T08:00:00Z", "x") == "2026/03/x"
    # 解析失败兜底直挂根
    assert new_home_rel("not-a-date", "x") == "x"
    assert new_home_rel("", "y") == "y"

    root = f"Y:{BS}照片"
    variants = old_prefix_variants(root, "album-2")
    # 反斜杠形态（claim 挪移产物）
    back = f"{root}{BS}album-2{BS}2026{BS}09-18{BS}a.jpg"
    # 正斜杠形态（引擎 render_dir 产物：dir_name 段后跟 `/`）
    fwd = f"{root}{BS}album-2/2026/09-18/a.jpg"
    new_dir = f"{root}{BS}2026{BS}09{BS}album-2"
    # 平铺：内层日期段丢弃，只保留文件名
    assert map_path(back, variants, new_dir) == f"{new_dir}{BS}a.jpg"
    assert map_path(fwd, variants, new_dir) == f"{new_dir}{BS}a.jpg"
    # 冲突改名表
    assert map_path(back, variants, new_dir, {"a.jpg": "a (2).jpg"}) == f"{new_dir}{BS}a (2).jpg"
    # 无关路径不动
    assert map_path(f"{root}{BS}SmartPhoto{BS}2026/08-02/b.jpg", variants, new_dir) is None
    assert map_path(f"{root}{BS}album-21{BS}c.jpg", variants, new_dir) is None, "前缀不得越过 dir_name 边界"
    print("selftest OK")


# ---------------------------------------------------------------------------
# 执行
# ---------------------------------------------------------------------------

def _move_file(src: Path, dst: Path) -> str:
    """同卷 rename；失败回退 copy+哈希校验+删源（对齐 claim 挪移语义）。
    返回使用的通道（rename / copy-verify）。"""
    dst.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.replace(src, dst)
        return "rename"
    except OSError:
        pass
    h = hashlib.blake2b()
    with open(src, "rb") as r, open(dst.with_suffix(dst.suffix + ".part"), "wb") as w:
        while True:
            chunk = r.read(8 * 1024 * 1024)
            if not chunk:
                break
            h.update(chunk)
            w.write(chunk)
    src_hash = hashlib.blake2b()
    with open(src, "rb") as r:
        while True:
            chunk = r.read(8 * 1024 * 1024)
            if not chunk:
                break
            src_hash.update(chunk)
    if src_hash.digest() != h.digest():
        dst.with_suffix(dst.suffix + ".part").unlink(missing_ok=True)
        raise RuntimeError(f"校验失败（源保留）: {src}")
    os.replace(dst.with_suffix(dst.suffix + ".part"), dst)
    os.remove(src)
    return "copy-verify"


def _prune_empty_dirs(root: Path) -> int:
    """自底向上清空 root 内空目录；root 本身空则一并删除。返回删除数。"""
    removed = 0
    for dirpath, dirnames, _filenames in os.walk(root, topdown=False):
        p = Path(dirpath)
        try:
            p.rmdir()
            removed += 1
        except OSError:
            pass  # 非空/占用：保留
    try:
        root.rmdir()
        removed += 1
    except OSError:
        pass
    return removed


def migrate(db_path: Path, photo_root: str, apply: bool) -> int:
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()
    albums = cur.execute(
        "SELECT id, created_at, dir_name FROM album ORDER BY id"
    ).fetchall()

    total_files = total_rewrites = 0
    for album_id, created_at, dir_name in albums:
        old_dir = Path(photo_root) / dir_name
        rel = new_home_rel(created_at, dir_name)
        new_dir = Path(photo_root) / rel
        if not old_dir.exists():
            print(f"  [跳过] album {album_id} {dir_name!r}: 旧目录不在盘"
                  f"（未物化或已迁移；新主目录应为 {rel}）")
            continue
        if new_dir.exists() and any(new_dir.iterdir()):
            print(f"  [警告] album {album_id} {dir_name!r}: 新目录已非空"
                  f"（{new_dir}）——疑已迁移，跳过；如需重跑请先人工确认")
            continue

        # ① 规划：旧目录全部文件提升平铺进新目录（内存占用表防同批同名
        # 互覆——盘上查重只对既有文件有效，同批两个 DSC_1.jpg 需计划期让名）
        moved = 0
        channels = {"rename": 0, "copy-verify": 0}
        plan: list[tuple[Path, Path]] = []
        taken: set[str] = {p.name for p in new_dir.iterdir()} if new_dir.exists() else set()
        disk_map: dict[str, str] = {}  # norm(旧全路径) → 新全路径
        conflicts = 0
        for dirpath, _dirnames, filenames in os.walk(old_dir):
            for fn in sorted(filenames):
                src = Path(dirpath) / fn
                dst_name = unique_name(fn, taken)
                taken.add(dst_name)
                dst = new_dir / dst_name
                plan.append((src, dst))
                if dst_name != fn:
                    conflicts += 1
                disk_map[src.as_posix().replace("/", BS).casefold()] = str(dst)
        print(f"  [迁移] album {album_id} {dir_name!r}: {old_dir} → {new_dir}"
              f"（{len(plan)} 个文件，冲突改名 {conflicts}）")
        if apply:
            for src, dst in plan:
                channels[_move_file(src, dst)] += 1
                moved += 1
            _prune_empty_dirs(old_dir)
        else:
            for src, _dst in plan[:3]:
                print(f"      示例: {src}")

        # ② DB 平铺改写：优先按「旧全路径 → 新全路径」精确映射（盘上
        # 搬移计划同源，冲突改名天然一致）；盘上缺席的行按前缀命中后用
        # 占用表让名（同批同名缺席行也不会撞 UNIQUE）
        new_dir_str = str(new_dir)
        rows = cur.execute("SELECT id, path FROM assets").fetchall()
        updates: list[tuple[str, int]] = []
        variants = old_prefix_variants(photo_root, dir_name)
        for asset_id, path in rows:
            hit = disk_map.get(path.replace("/", BS).casefold())
            if hit is None:
                hit = map_path(path, variants, new_dir_str)
                if hit is not None:
                    hit = new_dir_str + BS + unique_name(
                        Path(hit).name, taken)
                    taken.add(Path(hit).name)
            if hit is not None and hit != path:
                updates.append((hit, asset_id))
        if apply and updates:
            cur.executemany("UPDATE assets SET path = ?1 WHERE id = ?2", updates)
        total_files += moved if apply else len(plan)
        total_rewrites += len(updates)
        chan = "，".join(f"{k} {v}" for k, v in channels.items() if v) or "(dry-run)"
        print(f"      搬移 {chan}；assets.path 改写 {len(updates)} 行"
              f"{'（dry-run 未执行）' if not apply else ''}")

    if apply:
        conn.commit()
    conn.close()
    verb = "迁移完成" if apply else "预览完成（--apply 执行）"
    print(f"{verb}：共 {total_files} 个文件平铺入位，assets.path 改写 {total_rewrites} 行")
    if apply and total_files:
        print("提示：缩略图缓存键 = (path, mtime)，path 已变——下次访问全量重生成"
              "（首次浏览略慢属预期）；photoRoot/SmartPhoto 旧日期根未触碰。")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("db", nargs="?", help="library.db 路径")
    ap.add_argument("--photo-root", help="照片根（如 Y:\\照片）")
    ap.add_argument("--apply", action="store_true", help="执行迁移（缺省 dry-run 预览）")
    ap.add_argument("--selftest", action="store_true", help="纯函数表驱动自检后退出")
    args = ap.parse_args()

    if args.selftest:
        _selftest()
        return 0
    if not args.db or not args.photo_root:
        ap.error("需要 db 与 --photo-root（或 --selftest）")
    db_path = Path(args.db)
    if not db_path.is_file():
        raise SystemExit(f"库文件不存在: {db_path}")
    if not Path(args.photo_root).is_dir():
        raise SystemExit(f"照片根不存在: {args.photo_root}")

    if args.apply:
        backup = db_path.with_name(db_path.name + ".bak-album-layout")
        if not backup.exists():
            shutil.copy2(db_path, backup)
            print(f"已备份数据库 → {backup}")
    print(f"布局迁移 {db_path}（photoRoot={args.photo_root}）")
    return migrate(db_path, args.photo_root, args.apply)


if __name__ == "__main__":
    sys.exit(main())
