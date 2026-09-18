# M1 真机 E2E 验收清单（真实 Tauri 环境，非浏览器预览）

> 浏览器 dev server 仅用于快速布局/令牌检查（`isIpcAvailable()=false`，全部 IPC 走 mock 回退）。
> 本清单在**真实 app** 中执行：真实 IPC、真实文件系统、真实设备事件、真实 WebView2 渲染。

## 环境准备

1. 集成提交完成后（避免与 agent 的 cargo 构建抢 target 目录锁）：
   ```bash
   source .tools/env.sh
   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9223" npm run tauri dev
   ```
2. 主验收手段：browser-use 连接 `http://localhost:9223`（WebView2 CDP）——点击/输入驱动的是**真实 IPC** 的 app，截图为真实渲染。
3. 备份手段：`powershell -ExecutionPolicy Bypass -File scripts/capture-smartphoto.ps1 <out.png>`（OS 级窗口截图，进程名 smart-photo）。
4. 测试收纳区：用临时盘符目录（如 `I:\SmartPhoto-test-e2e\照片`）做目标根，**绝不用 Y:\照片 当目标**；源可用已知样例单文件所在的小目录。

## 用例

### A. 首启引导（真实落盘）
- [ ] 首次启动进入引导；三项目预填（主库 / I:\SmartPhoto\主库 / Y:\照片）
- [ ] 走完四步 → 开始使用 → `%APPDATA%\com.smartphoto.app\settings.json` 真实生成，字段正确（含 importSubdir=SmartPhoto）
- [ ] 再次启动不再出现引导

### B. 文件夹导入（复制模式）
- [ ] 向导左栏文件系统树：盘符根 → 逐级展开 → 选中样例小目录 → 中央网格列出真实文件（数量/大小/类型正确）
- [ ] 预览路径 = 目标根 + SmartPhoto + 模板渲染
- [ ] 开始导入（复制）→ 任务中心实时进度 → 完成总结（已复制 N）→ 目标目录文件真实存在、源目录不动
- [ ] 日志页可查；再次导入同目录 → 查重跳过

### C. 移动模式（纳管）
- [ ] 顶部切「移动」→ 导入到测试目标 → 源文件被删、空源子目录被清理、库内 assets 记录正确
- [ ] 删源失败容错：源文件设只读 → 任务成功 + 告警计数（可选手动用例）

### D. 守卫与恢复
- [ ] 选目标根 = 源目录内 → 启动被拒绝并有提示
- [ ] 导入中途暂停/恢复；导入中取消（软取消完成当前文件）
- [ ] 导入中拔卡/拔线（若用读卡器）→ 自动暂停；重连 → 恢复续传（真实卡片时验证）

### E. 真实设备（需用户插卡/接相机）
- [ ] 插卡 → 弹窗 → 确认 → 向导预选中该卡 → 文件清单正确（大目录渐进加载）
- [ ] 完整导入一张卡 → 总结 + 里程碑通知 + 完整日志
- [ ] MTP 相机直连同路径走一遍（单流强制）

### F. 视觉（在真实窗口截图，非浏览器）
- [ ] 引导四步 / 主壳 / 向导三栏+模式条 / 任务中心 / 总结弹窗，逐屏截图检查（DPI 缩放下的清晰度、窗口标题栏、原生对话框样式）
- [ ] 对比浏览器预览：字体渲染、滚动条、focus 环无回归

## 完成标准
全绿 → `git tag m1`。任何 D/E 失败项记 issue 并回到对应 lane 修复。
