#!/usr/bin/env python3
"""模型下载边界联调服务器（配合 SMARTPHOTO_DL_TEST_BASE 重写钩子，仅 debug 构建）。

用法:
  python scripts/test_dl_server.py --root <模型目录> --port 8787

行为由 <root>/_dl_mode.txt 控制（运行期可改，逐请求读取）:
  ok                 正常服务（支持 Range/206 断点续传）
  fail               一律 503（网络级失败 → .part 保留 → 前端 failed）
  truncate:<n>       只发 n 字节后断连（模拟断流；n 可带 k/m 后缀）
  corrupt            发等长错误字节（SHA256 不匹配 → 后端整轮重试）
  hang               建连后挂起 90s（模拟读超时）

日志打到 stdout，一行一请求。
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def parse_size(token: str) -> int:
    m = re.fullmatch(r"(\d+)([kKmM]?)", token)
    if not m:
        return int(token)
    n = int(m.group(1))
    return n * (1024 if m.group(2).lower() == "k" else 1024 * 1024 if m.group(2).lower() == "m" else 1)


def read_mode(root: Path) -> str:
    try:
        return (root / "_dl_mode.txt").read_text(encoding="utf-8").strip() or "ok"
    except OSError:
        return "ok"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt: str, *args):  # 统一行日志
        rng = self.headers.get("Range") if self.headers else None
        sys.stdout.write("%s %s%s\n" % (self.address_string(), fmt % args, " Range=" + rng if rng else ""))
        sys.stdout.flush()

    def do_GET(self):
        root = Path(self.server.root)  # type: ignore[attr-defined]
        name = self.path.lstrip("/").split("?")[0]
        # 重写钩子按模型 id 请求：依次尝试 <name> / <name>.onnx / <name>.json
        candidates = [root / name, root / (name + ".onnx"), root / (name + ".json")]
        src = next((c for c in candidates if c.is_file()), None)
        if src is None:
            self.send_error(404, f"no such file {name}")
            return
        data = src.read_bytes()
        mode = read_mode(root)
        rng = self.headers.get("Range")

        if mode == "fail":
            self.send_error(503, "test forced failure")
            return
        if mode == "hang":
            time.sleep(90)
            self.send_error(504, "test hang")
            return
        slow = mode == "slow"

        if mode.startswith("truncate:"):
            limit = min(parse_size(mode.split(":", 1)[1]), len(data))
        elif mode == "corrupt":
            limit = len(data)
            data = bytes(((b + 1) & 0xFF) for b in data)  # 等长损坏字节
        else:
            limit = len(data)

        start = 0
        partial = False
        if rng:
            m = re.fullmatch(r"bytes=(\d+)-", rng.strip())
            if m and int(m.group(1)) < limit:
                start = int(m.group(1))
                partial = True
            elif m:
                # 起点超出本轮会发量（如 truncate 后再请求更长前缀）：200 从头
                start = 0

        body = data[start:limit]
        status = 206 if partial else 200
        self.send_response(status)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{limit - 1}/{len(data)}")
        self.end_headers()
        if slow:
            # 慢速滴流：1KB/0.4s——连接健康、持续有数据（取消软标志可逐块观测）
            try:
                for i in range(0, len(body), 1024):
                    self.wfile.write(body[i : i + 1024])
                    self.wfile.flush()
                    time.sleep(0.4)
            except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
                pass  # 客户端断开（取消）属正常
            return
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
            pass  # 客户端断开（取消）属正常


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--port", type=int, default=8787)
    args = ap.parse_args()
    root = Path(args.root).resolve()
    (root / "_dl_mode.txt").write_text("ok", encoding="utf-8")
    srv = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    srv.root = str(root)  # type: ignore[attr-defined]
    print(f"dl test server on http://127.0.0.1:{args.port} root={root} mode-file=_dl_mode.txt", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
