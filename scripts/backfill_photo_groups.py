#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""存量照片组回填（0017 配套，阶段 B2）。

把库内既有 RAW+JPEG 孪生配对（assets.pair_asset_id 双向链，导入入册时写入）
升级为 photo_group / group_asset 归组：一次快门的 raw + sooc 同组。应用内
组形成逻辑在 db::refresh_asset_pair_on（0017 起新导入自动建组）；本脚本供
存量库一次性回填，幂等可重跑（已同组的配对直接跳过）。

用法：
    python scripts/backfill_photo_groups.py "I:\\SmartPhoto\\主库\\library.db"

说明：
- 只处理 kind 为 raw/photo 的配对（视频孪生不建组，与应用内规则一致）；
  角色：raw → 'raw'，photo → 'sooc'。
- 幂等：重复运行零改动。空组壳（全成员被删）一并清理。
- 与 Rust 参考实现 db::backfill_photo_groups_from_pairs 同语义。
"""

import sqlite3
import sys

ROLE_BY_KIND = {"raw": "raw", "photo": "sooc"}


def backfill(conn: sqlite3.Connection) -> tuple[int, int, int]:
    conn.execute("PRAGMA foreign_keys = ON")
    cur = conn.cursor()
    # 需要版本 >= 17（photo_group/group_asset 已建）；否则提示先跑应用迁移
    version = cur.execute("PRAGMA user_version").fetchone()[0]
    if version < 17:
        raise SystemExit(
            f"库 user_version={version} 尚无 photo_group 表（需 0017）："
            "先用新版本应用打开一次库完成迁移，再运行本脚本"
        )

    pairs: list[tuple[int, int]] = [
        (a, p)
        for a, p in cur.execute(
            "SELECT id, pair_asset_id FROM assets "
            "WHERE pair_asset_id IS NOT NULL AND id < pair_asset_id ORDER BY id"
        ).fetchall()
    ]

    linked = created = 0
    for a_id, b_id in pairs:
        ga = cur.execute(
            "SELECT group_id FROM group_asset WHERE asset_id = ?", (a_id,)
        ).fetchone()
        gb = cur.execute(
            "SELECT group_id FROM group_asset WHERE asset_id = ?", (b_id,)
        ).fetchone()
        if ga and gb and ga[0] == gb[0]:
            continue  # 已同组：幂等跳过

        kind_a = cur.execute("SELECT kind FROM assets WHERE id = ?", (a_id,)).fetchone()
        kind_b = cur.execute("SELECT kind FROM assets WHERE id = ?", (b_id,)).fetchone()
        role_a = ROLE_BY_KIND.get(kind_a[0]) if kind_a else None
        role_b = ROLE_BY_KIND.get(kind_b[0]) if kind_b else None
        if not (role_a and role_b):
            continue  # 非 raw+photo 孪生不建组

        before = cur.execute("SELECT COUNT(*) FROM photo_group").fetchone()[0]
        target = (ga or gb or [None])[0]
        if target is None:
            cur.execute("INSERT INTO photo_group DEFAULT VALUES")
            target = cur.lastrowid
        elif gb and gb[0] != target:
            # 合并另一组（组员并入 target 后删空壳）
            cur.execute(
                "UPDATE OR REPLACE group_asset SET group_id = ? WHERE group_id = ?",
                (target, gb[0]),
            )
            cur.execute("DELETE FROM photo_group WHERE id = ?", (gb[0],))
        # 防御：两资产的其他历史归属清除（一资产至多属一组）
        cur.execute(
            "DELETE FROM group_asset WHERE asset_id IN (?, ?) AND group_id != ?",
            (a_id, b_id, target),
        )
        for asset_id, role in ((a_id, role_a), (b_id, role_b)):
            cur.execute(
                "INSERT INTO group_asset (group_id, asset_id, role) VALUES (?, ?, ?) "
                "ON CONFLICT (group_id, asset_id) DO UPDATE SET role = excluded.role",
                (target, asset_id, role),
            )
        after = cur.execute("SELECT COUNT(*) FROM photo_group").fetchone()[0]
        linked += 1
        created += abs(after - before)

    # 空组壳清理（与 assets_delete_rows 同口径）
    cur.execute(
        "DELETE FROM photo_group WHERE id NOT IN (SELECT DISTINCT group_id FROM group_asset)"
    )
    conn.commit()
    return linked, created, cur.execute("SELECT COUNT(*) FROM photo_group").fetchone()[0]


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    path = sys.argv[1]
    conn = sqlite3.connect(path)
    try:
        linked, created, total = backfill(conn)
        print(f"回填完成：配对 {linked} 组，新建 {created} 组，现有 {total} 组")
    finally:
        conn.close()


if __name__ == "__main__":
    main()
