#!/usr/bin/env python3
# -*- coding: utf-8 -*-
r"""存量子组物理归位：DB subgroup 标签 → 真实子文件夹（0022 子组物理化）。

布局定案（2026-09-28 + 0022 修订，与 Rust 侧唯一公式
crate::db::Db::album_item_home_rel 同语义）：
    photoRoot/{相册 created_at:YYYY}/{相册 created_at:MM}/{dir_name}/          ← 相册根（平铺）
    photoRoot/{相册 created_at:YYYY}/{相册 created_at:MM}/{dir_name}/{子组}/    ← 子组（唯一不平铺例外）

背景：0022 之前导入/移组只写 album_item.subgroup（DB 标记），文件物理上
平躺在相册主目录——即**当前库里的全部子组数据**。本脚本按 DB subgroup
真值把平躺（或落错子组文件夹）的文件挪进 `.../{dir_name}/{净化子组}/`，
并改写 assets.path。

对每个相册的每个子组（album_item.subgroup 非空的 DISTINCT 值）：
1. 目标目录 = photoRoot/{创建YYYY}/{创建MM}/{dir_name}/{sanitize(subgroup)}/
   （子组段净化与 Rust crate::db::sanitize_dir_name 同口径——非法字符折叠
   `-`、尾点/空格剥离、保留设备名加前缀、80 字符截断、空兜底）。
2. 该子组每个资产：当前路径父目录 ≠ 目标目录 → 挪移（同卷 rename 毫秒级；
   失败回退 copy+哈希校验+删源——与 claim 挪移同款语义）；XMP 边车随行
   （`{stem}.xmp`）。目标同名冲突追加 " (2)"（claim 约定）。
3. assets.path 改写为新路径（盘上缺席的行也按计划改写——公式可推导）。
4. 幂等：已在目标目录的行跳过；重跑无副作用。

注意：
- subgroup IS NULL 的行（相册根散照）不动——它们本来就该平铺。
- 相册根平铺照片与子组文件夹共存；子组文件夹名与某**文件**同名时
  mkdir 失败 → 该行报错跳过（人工处理；与 Rust 侧「sanitize 后直接用」
  简化定案一致）。
- 缩略图缓存键 = (path, mtime)：path 变更后 miss，应用下次访问自动重
  生成（无需手工清理，仅提示耗时）。
- 应用必须在关闭状态运行本脚本（SQLite 独占写 + 文件挪移）。

用法：
    python scripts/migrate_subgroup_folders.py "I:\SmartPhoto\主库\library.db" --photo-root "Y:\照片"          # 预览（dry-run）
    python scripts/migrate_subgroup_folders.py "I:\SmartPhoto\主库\library.db" --photo-root "Y:\照片" --apply  # 执行
    python scripts/migrate_subgroup_folders.py --selftest                                                      # 纯函数表驱动自检
"""

import argparse
import hashlib
import os
import shutil
import sqlite3
import sys
import unicodedata
from pathlib import Path

# 相册主目录公式复用既有迁移脚本的唯一镜像（new_home_rel 与 Rust
# album_home_rel_parts 同语义；改公式两处同步）
sys.path.insert(0, str(Path(__file__).resolve().parent))
from migrate_album_layout import new_home_rel  # noqa: E402

BS = chr(92)  # 反斜杠（避免源码里转义混乱）

RESERVED = [
    "CON", "PRN", "AUX", "NUL",
    "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
]


# ---------------------------------------------------------------------------
# 纯函数（Rust 侧 crate::db 为权威；此处为迁移镜像）
# ---------------------------------------------------------------------------

def _is_control(c: str) -> bool:
    """Rust char::is_control() 等价（Unicode Cc 类别）。"""
    return unicodedata.category(c) == "Cc"


def sanitize_dir_name(raw: str) -> str:
    r"""子组名 → 物理目录段（Rust crate::db::sanitize_dir_name 同口径）：
    trim → 非法字符 `< > : " / \ | ? *` 与控制符折叠 `-` → 尾点/空格剥离 →
    超长截 80 字符（截后尾 `./空格/-` 再剥）→ 保留设备名加 `album-` 前缀 →
    空兜底 `album`。"""
    out = []
    for c in raw.strip():
        if c in "<>:\"/\\|?*" or _is_control(c):
            out.append("-")
        else:
            out.append(c)
    out = "".join(out).strip().rstrip(". ")
    if len(out) > 80:
        out = out[:80].rstrip(". -")
    if out.upper() in RESERVED:
        out = f"album-{out}"
    if not out:
        out = "album"
    return out


def item_home_rel(created_at: str, dir_name: str, subgroup: str | None) -> str:
    """条目目录相对段：`{创建YYYY}/{创建MM}/{dir_name}[/{净化子组}]`
    （Rust crate::db::Db::album_item_home_rel 同语义）。"""
    home = new_home_rel(created_at, dir_name)
    if subgroup is None:
        return home
    return f"{home}/{sanitize_dir_name(subgroup)}"


def target_path(photo_root: str, created_at: str, dir_name: str,
                subgroup: str, fname: str, fixups: dict[str, str] | None = None) -> str:
    r"""目标全路径（`\` 形态）：photoRoot/{...}/{子组}/{文件名}；
    fixups = 冲突改名表（原名 → " (2)" 名）。"""
    rel = item_home_rel(created_at, dir_name, subgroup)
    fname = (fixups or {}).get(fname, fname)
    return photo_root + BS + rel.replace("/", BS) + BS + fname


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
    # 子组段净化（Rust sanitize_dir_name 同口径）
    assert sanitize_dir_name("成片") == "成片"
    assert sanitize_dir_name("机内直出") == "机内直出"
    assert sanitize_dir_name("a:b/c\\d|e?f*g") == "a-b-c-d-e-f-g"
    assert sanitize_dir_name("  trailing.. .  ") == "trailing"
    assert sanitize_dir_name("CON") == "album-CON"
    assert sanitize_dir_name("***") == "---"
    assert sanitize_dir_name("   ") == "album"
    assert sanitize_dir_name("x" * 100) == "x" * 80

    # 条目目录公式：None = 平铺主目录；Some = 追加净化子组段
    assert item_home_rel("2026-09-27T05:09:56.381Z", "album-1", None) == "2026/09/album-1"
    assert item_home_rel("2026-09-27T05:09:56.381Z", "album-1", "成片") == "2026/09/album-1/成片"
    assert item_home_rel("2025-12-31T23:59:59.999Z", "婚礼", "A/B") == "2025/12/婚礼/A-B"
    assert item_home_rel("not-a-date", "x", "组") == "x/组"

    root = f"Y:{BS}照片"
    assert target_path(root, "2026-09-27T05:09:56.381Z", "album-1", "成片", "a.jpg") == \
        f"{root}{BS}2026{BS}09{BS}album-1{BS}成片{BS}a.jpg"
    assert target_path(root, "2026-09-27T05:09:56.381Z", "album-1", "成片", "a.jpg",
                       {"a.jpg": "a (2).jpg"}) == \
        f"{root}{BS}2026{BS}09{BS}album-1{BS}成片{BS}a (2).jpg"

    # 冲突后缀
    assert unique_name("a.jpg", set()) == "a.jpg"
    assert unique_name("a.jpg", {"a.jpg"}) == "a (2).jpg"
    assert unique_name("a.jpg", {"a.jpg", "a (2).jpg"}) == "a (3).jpg"
    assert unique_name("noext", {"noext"}) == "noext (2)"
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


def _sidecar(path: Path) -> Path:
    """XMP 边车路径（Rust crate::metadata::xmp::sidecar_path 同语义）。"""
    return path.with_name(path.stem + ".xmp")


def _move_with_sidecar(src: Path, dst: Path) -> str:
    """挪移主文件 + XMP 边车随行（在盘才动）。"""
    channel = _move_file(src, dst)
    side_src, side_dst = _sidecar(src), _sidecar(dst)
    if side_src.is_file():
        side_dst.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.replace(side_src, side_dst)
        except OSError:
            shutil.copy2(side_src, side_dst)
            os.remove(side_src)
    return channel


def migrate(db_path: Path, photo_root: str, apply: bool) -> int:
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()
    albums = cur.execute(
        "SELECT id, created_at, dir_name FROM album ORDER BY id"
    ).fetchall()

    total_files = total_rewrites = total_skip = 0
    for album_id, created_at, dir_name in albums:
        subgroups = cur.execute(
            "SELECT DISTINCT i.subgroup FROM album_item i \
             JOIN assets a ON a.id = i.asset_id \
             WHERE i.album_id = ?1 AND i.subgroup IS NOT NULL ORDER BY i.subgroup",
            (album_id,),
        ).fetchall()
        if not subgroups:
            continue
        for (subgroup,) in subgroups:
            home = item_home_rel(created_at, dir_name, subgroup)
            dst_dir = Path(photo_root) / home
            rel_root = new_home_rel(created_at, dir_name)
            dst_dir_norm = str(dst_dir).replace("/", BS).casefold()

            rows = cur.execute(
                "SELECT i.asset_id, a.path, a.filename FROM album_item i \
                 JOIN assets a ON a.id = i.asset_id \
                 WHERE i.album_id = ?1 AND i.subgroup = ?2 ORDER BY i.asset_id",
                (album_id, subgroup),
            ).fetchall()
            plan: list[tuple[int, str, Path, Path]] = []  # (asset_id, old_path, src, dst)
            rewrites: list[tuple[str, int]] = []
            skipped = missing = 0
            # 占用表：目标目录既有文件 + 同批已计划名（同批同名防互覆）
            taken: set[str] = {p.name for p in dst_dir.iterdir()} \
                if dst_dir.is_dir() else set()
            conflicts = 0
            for asset_id, path, fname in rows:
                src = Path(path)
                src_dir_norm = (str(src.parent) if str(src.parent) else "").replace("/", BS).casefold()
                if src_dir_norm == dst_dir_norm:
                    skipped += 1  # 已在目标子组目录：幂等跳过
                    continue
                dst_name = unique_name(fname or src.name, taken)
                taken.add(dst_name)
                if dst_name != (fname or src.name):
                    conflicts += 1
                dst = dst_dir / dst_name
                plan.append((asset_id, path, src, dst))
                rewrites.append((str(dst), asset_id))
                if not src.is_file():
                    missing += 1
            print(f"  [子组] album {album_id} {dir_name!r} / {subgroup!r} → {home}"
                  f"（{len(plan)} 待挪，已就位 {skipped}，盘上缺席 {missing}，"
                  f"冲突改名 {conflicts}）")
            if apply:
                channels = {"rename": 0, "copy-verify": 0}
                for asset_id, old_path, src, dst in plan:
                    if src.is_file():
                        try:
                            channels[_move_with_sidecar(src, dst)] += 1
                        except (OSError, RuntimeError) as e:
                            print(f"      [失败] {old_path}: {e}（DB 不改，重跑续迁）")
                            rewrites = [(p, a) for (p, a) in rewrites if a != asset_id]
                            continue
                    else:
                        print(f"      [缺席] {old_path} 不在盘——仅按公式改写 DB 路径")
                cur.executemany("UPDATE assets SET path = ?1 WHERE id = ?2", rewrites)
                chan = "，".join(f"{k} {v}" for k, v in channels.items() if v) or "(无盘上挪移)"
                print(f"      搬移 {chan}；assets.path 改写 {len(rewrites)} 行")
                total_files += sum(channels.values())
            else:
                for _aid, old_path, src, dst in plan[:3]:
                    print(f"      示例: {old_path} → {dst}")
            total_rewrites += len(rewrites) if apply else len(plan)
            total_skip += skipped

    if apply:
        conn.commit()
    conn.close()
    verb = "归位完成" if apply else "预览完成（--apply 执行）"
    print(f"{verb}：共 {total_files} 个文件挪入子组文件夹，assets.path 改写 "
          f"{total_rewrites} 行，已就位跳过 {total_skip} 行；相册根散照未触碰")
    if apply and total_files:
        print("提示：缩略图缓存键 = (path, mtime)，path 已变——下次访问重生成"
              "（首次浏览略慢属预期）。")
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
        backup = db_path.with_name(db_path.name + ".bak-subgroup")
        if not backup.exists():
            shutil.copy2(db_path, backup)
            print(f"已备份数据库 → {backup}")
    print(f"子组归位 {db_path}（photoRoot={args.photo_root}）")
    return migrate(db_path, args.photo_root, args.apply)


if __name__ == "__main__":
    sys.exit(main())
