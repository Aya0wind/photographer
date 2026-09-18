# M0 基础骨架 · 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 搭起可运行的 Smart Photo 骨架：Tauri 2 + React 19 应用窗口、托盘常驻、配置框架（热应用）、首次引导向导、中文 i18n、暗色主题设计令牌。

**Architecture:** Rust 侧实现 settings.json 加载/迁移/保存与 `settings.*` IPC 命令，改动通过 `settings://changed` 事件热广播；前端 App Shell（侧栏路由）+ 引导向导（未完成引导强制跳转）。设备/导入/AI 等模块在 M1+ 加入，M0 只留 ipc 命名空间与目录。

**Tech Stack:** Tauri 2.x、React 19 + TypeScript(strict) + Vite、Tailwind CSS 4（@theme 令牌）、react-router、Zustand、TanStack Query、react-i18next（仅 zh）、motion。

**Spec:** `docs/specs/smart-photo-design.md`（§4 架构、§5.10 配置体系、§5.9 UI、§10 默认值）

## Global Constraints（出自 spec，全任务隐含遵守）

- 仅 Windows 10 21H2+；仅中文 UI（i18n 架构第一天接入，文案不写死组件）
- 深色令牌：底 `#0E0F12`/面 `#16181D`/面板 `#1E2128`；文字 `#E8EAED`/`#9AA0A6`/`#5F6368`；强调 `#F0A83C`
- settings.json 带 `schemaVersion`，只加不改字段，迁移向后兼容，保存原子写
- IPC 命令命名空间：`settings.*`（M0）；后续 `import.*`/`search.*`/`ai.*`
- 关闭主窗 = 最小化托盘（默认，可配）；开机自启默认关
- Rust 代码 cargo fmt + clippy 无警告；核心模块单测 TDD；conventional commits

---

### Task 0: 环境准备（最新版 Rust + 最新版 Node，用户级安装）

**Files:**
- Create: `.tools/env.sh`、`.gitignore`（追加 `.tools/`）

**Interfaces:**
- Produces: `source .tools/env.sh` 后 PATH 含最新 Node 与 cargo（用户级安装，不动系统 Node 17 的原有全局，避免破坏其他项目——如需系统级替换另行确认）

**执行策略（用户指示）：** Task 0/1 内联完成（阻塞项）；Task 2（前端壳）与 Task 3+4（Rust 配置框架+托盘）**并行派发 subagent**；Task 5/6 主会话收敛。共享文件（lib.rs/Cargo.toml/package.json）只在各自 lane 内改动，避免冲突。

- [ ] **Step 1: 安装 rustup 最新 stable（用户级 ~/.cargo，MSVC 已就绪，无 UAC）**

```bash
curl -sSfL https://win.rustup.rs/x86_64 -o "$TEMP/rustup-init.exe" && "$TEMP/rustup-init.exe" -y --default-toolchain stable --default-host x86_64-pc-windows-msvc
```

- [ ] **Step 2: 安装最新版 Node（官方 dist 最新 release，解压到 ~/.local/nodejs，用户 PATH，无 UAC）**

```bash
# 从 dist/index.json 取最新版本号（数组首项即最新）
LATEST=$(curl -sSfL https://nodejs.org/dist/index.json | head -c 2000 | grep -o '"version":"v[0-9.]*"' | head -1 | cut -d'"' -f4)
curl -sSfL -o "$HOME/.local/node-latest.zip" "https://nodejs.org/dist/${LATEST}/node-${LATEST}-win-x64.zip"
mkdir -p "$HOME/.local/nodejs" && unzip -qo "$HOME/.local/node-latest.zip" -d "$HOME/.local/nodejs-tmp" \
  && rm -rf "$HOME/.local/nodejs" && mv "$HOME/.local/nodejs-tmp/node-${LATEST}-win-x64" "$HOME/.local/nodejs" \
  && rm -rf "$HOME/.local/nodejs-tmp" "$HOME/.local/node-latest.zip"
setx PATH "$HOME\\.local\\nodejs;$PATH" >/dev/null   # 写入用户 PATH（新开终端生效）
```

- [ ] **Step 3: 写 .tools/env.sh（本会话及 subagent 用）**

```bash
#!/usr/bin/env bash
# 项目工具链环境：source 本文件后获得最新 Node 与 cargo
export PATH="$HOME/.local/nodejs:$PATH"
export PATH="$HOME/.cargo/bin:$PATH"
```

- [ ] **Step 4: 验证**

```bash
source .tools/env.sh && node -v && npm -v && cargo -V && rustc -V
```
Expected: node = dist 最新 release；cargo 为最新 stable

- [ ] **Step 5: .gitignore 追加 `.tools/`，commit**

### Task 1: Tauri 2 + React 19 脚手架

**Files:**
- Create: 项目根全部脚手架文件（src-tauri/、src/、package.json 等）

**Interfaces:**
- Produces: `npm run dev`（vite）、`cargo check` 通过的空应用；`src-tauri/src/lib.rs` 含 `run()`

- [ ] **Step 1:** `npm create tauri-app@latest sp-scaffold -- --template react-ts --manager npm --yes`（在临时目录），产物上移到项目根
- [ ] **Step 2:** `npm install`
- [ ] **Step 3:** `cargo check`（在 src-tauri/）验证 Rust 侧编译
- [ ] **Step 4:** `npm run build`（tsc + vite build）验证前端编译
- [ ] **Step 5:** commit `chore: tauri2 + react19 scaffold`

### Task 2: 主题令牌 + App Shell + 路由 + i18n

**Files:**
- Create: `src/app/App.tsx`、`src/app/routes.tsx`、`src/app/shell/Sidebar.tsx`、`src/features/{gallery,search,import,tasks,settings}/pages/*.tsx`（占位）、`src/ipc/index.ts`、`src/i18n/{index.ts,zh.json}`、`src/styles/app.css`
- Modify: `vite.config.ts`（tailwind 插件）、`src/main.tsx`

**Interfaces:**
- Produces: 路由 `/`（Shell 内嵌 gallery/search/import/tasks/settings 占位页）、`/onboarding`；`src/ipc` 导出类型化 `invoke` 包装 `ipc<T>(cmd, payload)`；i18n `t()` 全站可用
- Consumes: Task 3 的 `settings_get`/`settings_set`（本任务先留调用位，Task 3 完成后联通）

- [ ] **Step 1:** 安装 `tailwindcss @tailwindcss/vite react-router zustand @tanstack/react-query react-i18next i18next motion`
- [ ] **Step 2:** `app.css` 写 @theme 令牌：

```css
@import "tailwindcss";
@theme {
  --color-bg: #0E0F12;
  --color-surface: #16181D;
  --color-panel: #1E2128;
  --color-text-primary: #E8EAED;
  --color-text-secondary: #9AA0A6;
  --color-text-muted: #5F6368;
  --color-accent: #F0A83C;
  --font-sans: "Segoe UI Variable Text", "Segoe UI", "Microsoft YaHei UI", sans-serif;
  --font-mono: "Cascadia Mono", Consolas, monospace;
}
```

- [ ] **Step 3:** Sidebar（画廊/搜索/导入/任务/设置，icon+label，激活态 accent）+ Shell 布局 + 占位页（每页一行标题+待开发说明）
- [ ] **Step 4:** i18n 接入：`zh.json` 含全部导航与占位文案；所有组件经 `t()`
- [ ] **Step 5:** `npm run build` 验证 + commit `feat: app shell with theme tokens, routing, i18n`

### Task 3: 配置框架（Rust，TDD）

**Files:**
- Create: `src-tauri/src/settings/mod.rs`（结构+默认值+迁移+原子保存）、`src-tauri/src/ipc/settings.rs`（命令）、`src-tauri/tests/settings_test.rs`
- Modify: `src-tauri/src/lib.rs`（注册命令/状态）、`src-tauri/Cargo.toml`（serde/serde_json/thiserror、tauri-plugin-dialog）

**Interfaces:**
- Produces（后续任务依赖的确切签名）:

```rust
pub struct Settings { schema_version: u32, library_root: Option<String>,
    onboarding_completed: bool,
    import: ImportSettings, ai: AiSettings, system: SystemSettings }
pub enum DuplicatePolicy { Skip, Rename, Ask }   // serde camelCase
pub enum IndexSchedule { IdleOnly, AfterImport, Manual }
pub struct ImportSettings { prompt_on_device: bool(true), skip_imported: bool(true),
    dir_template: String("{YYYY}/{MM-DD}/{原文件名}"), duplicate_policy, notify_milestones: bool(true) }
pub struct AiSettings { enable_clip: bool(false), enable_face: bool(false),
    enable_scene_tags: bool(false), index_schedule, cpu_limit_percent: u32(50), use_gpu: bool(true) }
pub struct SystemSettings { launch_at_login: bool(false), close_to_tray: bool(true), language: String("zh") }
impl SettingsManager {
    pub fn load(dir: &Path) -> Result<Self>;      // 缺文件→默认；旧版→迁移
    pub fn save(&self, dir: &Path) -> Result<()>; // 原子写 settings.json.tmp→rename
}
// IPC: settings_get(state) -> Settings
// IPC: settings_set(app, state, settings: Settings) -> Result<()>  保存后 emit "settings://changed"
```

- [ ] **Step 1: 写失败测试**（默认值生成、load 缺文件、roundtrip 保存再加载相等、旧 JSON 缺字段容错）
- [ ] **Step 2: 跑测试确认失败**（模块不存在）
- [ ] **Step 3: 实现 mod.rs + ipc/settings.rs + lib.rs 注册**
- [ ] **Step 4: `cargo test` 全绿**
- [ ] **Step 5: commit `feat: settings framework with migration and atomic save`**

### Task 4: 托盘常驻 + 单实例 + 插件接线

**Files:**
- Modify: `src-tauri/tauri.conf.json`（trayIcon、productName Smart Photo）、`src-tauri/src/lib.rs`、`Cargo.toml`
- Create: `src-tauri/src/tray.rs`

**Interfaces:**
- Produces: 关闭→隐藏窗口留托盘；托盘菜单（显示主窗/退出）；单实例（二次启动聚焦已有窗口）；插件注册：single-instance、dialog、notification、autostart（默认关）

- [ ] **Step 1:** Cargo.toml 加 tauri（features tray-icon）、tauri-plugin-single-instance/dialog/notification/autostart
- [ ] **Step 2:** tauri.conf.json 配 tray（icon 用默认 app-icon，tooltip "Smart Photo"）
- [ ] **Step 3:** tray.rs：托盘菜单"显示/退出"；窗口 CloseRequested→按 `close_to_tray` 隐藏或退出
- [ ] **Step 4:** `cargo check` + 手动 `npm run tauri dev` 验证（关闭留托盘、双启动聚焦）
- [ ] **Step 5: commit `feat: tray residency, single instance, plugin wiring`**

### Task 5: 首次引导向导

**Files:**
- Create: `src/features/onboarding/{OnboardingPage.tsx,steps/LibraryStep.tsx,ImportSchemeStep.tsx,AiStep.tsx,DoneStep.tsx}`、`src/stores/settingsStore.ts`
- Modify: `src/app/routes.tsx`（守卫：未完成引导→/onboarding）

**Interfaces:**
- Consumes: Task 3 的 `settings_get`/`settings_set`；Task 4 的 dialog 插件（前端 `@tauri-apps/plugin-dialog` 的 `open({directory:true})` 选库目录）
- Produces: `settingsStore`（Zustand：settings 快照 + `save(partial)` 节流持久化），后续所有功能页读配置走此 store

- [ ] **Step 1:** settingsStore：加载/合并保存/监听 `settings://changed`
- [ ] **Step 2:** 四步向导 UI（B 风格卡片居中、进度点、motion 步进动画）：①欢迎+选择库根目录（文件夹选择器）②导入方案摘要（目录模板预览行 + 查重策略三选）③AI 三选一（全开/仅语义/全关，说明各自含义与耗时提示）④完成（写入配置→进入主界面）
- [ ] **Step 3:** 路由守卫 + 主界面读 `libraryRoot` 显示于设置占位页
- [ ] **Step 4:** `npm run build` + tauri dev 手动走通向导、重启后不再出现（onboarding_completed 持久）
- [ ] **Step 5: commit `feat: first-run onboarding wizard`**

### Task 6: M0 验收与收尾

- [ ] `cargo fmt` + `cargo clippy -- -D warnings` + `cargo test` 全绿
- [ ] `npm run build` 零 TS 错误
- [ ] 手动验收清单：启动→向导→主界面→关闭到托盘→托盘恢复→重启跳过向导
- [ ] README.md（项目简介/开发命令/工具链说明 source .tools/env.sh）
- [ ] commit `chore: m0 acceptance` + git tag `m0`

## Self-Review

- Spec 覆盖：M0 范围=骨架/托盘/配置框架/引导向导（spec §9 M0 行）✅；配置项默认值与 §10 一致（prompt_on_device=true、AI 引导后置默认 false、自启 false、close_to_tray true）✅
- 无占位符；类型/命名前后一致（SettingsManager、settings_get/set、settings://changed、DuplicatePolicy）
- 后续计划：M1（设备检测+导入引擎）在本计划验收后另立计划文档
