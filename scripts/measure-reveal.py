# -*- coding: utf-8 -*-
"""启动计时探针：轮询 EnumWindows，记录 splash 窗口与主窗口（1280 宽）的
可见时刻，判别 reveal 链路 vs 8s 安全网。用法：python measure-reveal.py <exe>"""
import ctypes
import ctypes.wintypes as wt
import subprocess
import sys
import time

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32

WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)

def get_windows(pid):
    found = []
    def cb(hwnd, _):
        owner = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            rect = wt.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            n = user32.GetWindowTextLengthW(hwnd)
            buf = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(hwnd, buf)
            found.append((rect.right - rect.left, rect.bottom - rect.top, buf.value))
        return True
    user32.EnumWindows(WNDENUMPROC(cb), 0)
    return found

exe = sys.argv[1]
t0 = time.perf_counter()
proc = subprocess.Popen([exe])
t_splash = None
t_main = None
while time.perf_counter() - t0 < 20:
    for (w, h, title) in get_windows(proc.pid):
        # splash 460x300；主窗 1280x800（可能被用户缩放，按宽度分界）
        if t_splash is None and w < 900:
            t_splash = time.perf_counter() - t0
        if t_main is None and w >= 900:
            t_main = time.perf_counter() - t0
    if t_main is not None:
        break
    time.sleep(0.03)

print(f"splash visible: {t_splash and round(t_splash*1000)}ms")
print(f"main   visible: {t_main and round(t_main*1000)}ms")
print(f"delta  : {t_splash is not None and t_main is not None and round((t_main-t_splash)*1000)}ms")
kernel32.TerminateProcess(proc.handle, 0)
