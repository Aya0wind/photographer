# Smart Photo

Windows 桌面照片管理应用（Tauri 2 + React 19 + Rust）：读卡器/相机自动导入、EXIF 索引与多维搜索、本地 AI 语义/人脸搜索、Lightroom 互通。全本地运行，性能优先。

## 开发环境

- Node 26（用户级 `~/.local/nodejs`）、Rust stable（rustup，MSVC）+ VS Build Tools
- Git Bash 中先加载工具链：`source .tools/env.sh`
- npm 源已配置 npmmirror

## 常用命令

```bash
source .tools/env.sh        # 每个新 shell 先执行
npm run tauri dev           # 开发运行（热更新）
npm test                    # 前端 vitest 全量测试
npm run build               # 前端类型检查 + 构建
cd src-tauri
cargo test                  # Rust 单测+集成测试
cargo clippy --all-targets -- -D warnings
```

## 文档

- 设计文档（唯一事实来源）：`docs/specs/smart-photo-design.md`
- 实施计划：`docs/plans/`（M0 骨架 / M1 设备+导入引擎…）
- 测试策略：设计文档 §13（测试跟上实现、自动化、易错路径重点覆盖）

## 存储模型（达芬奇式）

- 全局配置：`%APPDATA%\com.smartphoto.app\settings.json`（轻量：偏好 + 库注册表）
- 库 = 独立数据单元：数据库目录（默认 `I:\SmartPhoto\<库名>`，自包含可迁移）+ 照片存储目录（默认 `Y:\照片`）

## 里程碑

M0 骨架（托盘/配置/引导向导）→ M1 设备检测+导入引擎 → M2 清卡/双目的地/原地索引 → M3 元数据+搜索 → M4 缩略图/画廊/看图/LR → M5 AI → M6 AI选片 → M7 监视文件夹/回忆/去重/统计 → M8 打磨。
