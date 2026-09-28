# 安全手术（用户 2026-09-28 指令）：所有测试库照片路径迁离真实归档 Y:\照片 → Y:\Test。
# - settings.json: 三个库 photoRoot → Y:\Test\{main,test,edge}
# - 各库 DB: 仅重写指向旧根的资产路径前缀（大小写/分隔符不敏感，边界安全）
# - Y:\照片 本体不做任何写操作
import json
import sqlite3
import sys
from pathlib import Path

SETTINGS = Path(r"C:\Users\12003\AppData\Roaming\com.smartphoto.app\settings.json")
MAP = {  # (library id 前缀, 旧根, 新根)
    "lib-main": (r"Y:\照片", r"Y:\Test\main"),
    "eba4020c": (r"Y:\照片", r"Y:\Test\test"),
    "30ee5b50": (r"I:\SmartPhoto\edge-photos", r"Y:\Test\edge"),
}
DBS = [
    ("主库", Path(r"I:\SmartPhoto\主库\library.db"), r"Y:\照片", r"Y:\Test\main"),
    ("边界", Path(r"I:\SmartPhoto\edge-db\library.db"), r"I:\SmartPhoto\edge-photos", r"Y:\Test\edge"),
]


def norm(c: str) -> str:
    return "\\" if c in "/\\" else c.lower()


def strip_prefix(path: str, root: str):
    """path 前缀==root（忽略大小写与分隔符；root 后须是分隔符或恰尽）→ 返回原始尾段。"""
    if len(path) < len(root):
        return None
    for i in range(len(root)):
        if norm(path[i]) != norm(root[i]):
            return None
    if len(path) == len(root):
        return ""
    if norm(path[len(root)]) == "\\":
        tail = path[len(root) + 1:]
    elif root.endswith(("/", "\\")):
        tail = path[len(root):]
    else:
        return None
    return tail.lstrip("/\\")


def main() -> int:
    for _, new in MAP.values():
        Path(new).mkdir(parents=True, exist_ok=True)

    settings = json.loads(SETTINGS.read_text(encoding="utf-8"))
    for lib in settings["libraries"]:
        for idp, (_old, new) in MAP.items():
            if lib["id"].startswith(idp):
                old = lib["photoRoot"]
                lib["photoRoot"] = new
                print(f"settings: {lib['name']} {old} -> {new}")
    SETTINGS.write_text(json.dumps(settings, ensure_ascii=False, indent=2), encoding="utf-8")

    for name, dbp, old, new in DBS:
        db = sqlite3.connect(dbp)
        rows = db.execute("select id, path from assets").fetchall()
        rewritten = outside = 0
        updates = []
        for aid, path in rows:
            if path is None:
                continue
            tail = strip_prefix(path, old)
            if tail is None:
                outside += 1
                continue
            updates.append((new + "\\" + tail if tail else new, aid))
        db.executemany("update assets set path = ?1 where id = ?2", updates)
        db.commit()
        print(f"{name}: assets={len(rows)} rewritten={len(updates)} 其他根不动={outside}")
        if updates:
            print("  sample:", updates[0][0])
        db.close()
    print("Y:\\照片 untouched (read-only)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
