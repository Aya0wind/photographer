# 修正边界测试库记录：CDP 转义事故把 photoRoot/dbDir 写成了盘符相对路径
# （I:SmartPhotoedge-photos），规范化为绝对路径。app 的配置热重载会自动生效。
import json
import sys

p = r"C:\Users\12003\AppData\Roaming\photohub\settings.json"
d = json.load(open(p, encoding="utf-8"))
fixed = []
for lib in d.get("libraries", []):
    if lib.get("id") == "30ee5b50-e841-4a78-a5bc-8675fa7a72fd":
        lib["photoRoot"] = "I:\\SmartPhoto\\edge-photos"
        lib["dbDir"] = "I:\\SmartPhoto\\edge-db"
        fixed.append(lib["name"])
json.dump(d, open(p, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
print("fixed:", fixed)
