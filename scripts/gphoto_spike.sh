#!/bin/bash
# libgphoto2 A7R V spike（在 MSYS2 UCRT64 内运行）
set -u
export MSYS2_ARG_CONV_EXCL="*"  # /main/... 是相机配置路径，不是文件系统路径
cd /tmp || exit 1
for k in /main/imgsettings/iso /main/capturesettings/f-number /main/capturesettings/shutterspeed2 /main/capturesettings/shutterspeed /main/status/batterylevel; do
  echo "--- $k"
  gphoto2 --get-config "$k" 2>&1 | head -8
done
echo "=== PREVIEW ==="
gphoto2 --capture-preview --force-overwrite 2>&1 | tail -2
ls -la /tmp/preview.jpg 2>/dev/null
echo "=== SET ISO TEST (read current first) ==="
gphoto2 --get-config /main/imgsettings/iso 2>&1 | grep -E "Current|Choice" | head -12
