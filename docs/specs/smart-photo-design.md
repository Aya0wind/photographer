# 智能照片管理应用（Smart Photo）· 设计文档

版本：V3（定稿） ｜ 日期：2026-09-18 ｜ 状态：已获用户批准，进入实施

---

## 1. 项目概述

基于 Tauri 2 的 Windows 桌面照片管理应用，面向 RAW 摄影师工作流：读卡器/相机插入自动导入、EXIF 元数据纳管、本地 AI 语义/人脸搜索、与 Lightroom 互通。全本地运行，无云依赖。

**核心指标**：性能优先（吃满硬件）、20 万+ 张库规模、全程可中断可恢复、处处可配置。

## 2. 需求范围

### 2.1 核心需求

1. 读卡器/相机连接自动弹窗，确认后自动导入；导入方案（命名/目录模板）可配置
2. EXIF/视频元数据读取入库 SQLite，多维条件搜索
3. 快速交付 Lightroom（watched folder）及其他应用
4. RAW/视频自动识别；按拍摄日期归档；重复跳过/重命名；导入进度通知；完成总结+详细日志
5. 本地模型照片内容/人脸识别，支持自然语言搜索与按人搜索
6. 流畅动画；错误处理与操作中断全程可恢复

### 2.2 首版新增（用户确认）

| # | 功能 | 说明 |
|---|---|---|
| F1 | 安全清卡 | 仅删除哈希校验通过且已入库文件；二次确认；清单入日志；插卡扫描秒级跳过已入库文件 |
| F2 | 双目的地导入 | ImportStage 实现第二目的地异步写；失败仅警告不阻塞主流程 |
| F3 | AI 选片 | 连拍分组+清晰度/曝光/眨眼评分；标最佳帧；画廊组折叠+看图器对比模式 |
| F4 | 监视文件夹 | 多目录监视（手机互传/微信/截图），写入稳定 1.5s 后触发，走完整导入引擎 |
| F5 | 智能相册 | CLIP 零样本 ~20 类场景打标，动态相册，标签进搜索体系 |
| F6 | 那年今天 | 首页卡片，N 年前 ±3 天 |
| F8 | 库内去重 | pHash64 全库扫描，海明距离 ≤10 聚组，人工决定，永不自动删除 |
| F9 | 器材统计 | 相机/镜头/焦段/ISO/光圈分布+月度趋势图表 |
| — | 原地索引 | 已有目录不移动不修改直接纳管（source=external），离线盘标灰保留记录 |

### 2.3 二期路线图（不排期）

F7 地图视图（离线瓦片）、F10 视频代理转码、F11 XMP 批量写入、F12 应用级回收站、F13 多库管理。

## 3. 决策记录

| 日期 | 决策 |
|---|---|
| 2026-09-18 | 仅 Windows 10 21H2+；相机直连（WPD/MTP）首版支持；前端 React |
| 2026-09-18 | 无许可限制，纯技术选型（libraw/ffmpeg 全功能可用） |
| 2026-09-18 | 性能优先：单次读取流水线、GPU 加速、20 万+ 库规模（HNSW 向量索引 day one） |
| 2026-09-18 | UI：B+A 混合（画廊/搜索/看图=深色现代消费风；导入向导/任务中心/日志=专业高密度） |
| 2026-09-18 | 看图器纯浏览+评分（调色交给 LR）；视频内建播放+系统回退 |
| 2026-09-18 | F1-F4 进首版；F5/F6/F8/F9 进首版（F7 二期）；支持原地索引 |
| 2026-09-18 | AI 首次引导三选一；插入设备默认每次弹窗确认 |
| 2026-09-18 | 默认目录模板：年/日期/原文件名；评分 DB+XMP 边车同步；首版仅中文 |
| 2026-09-18 | 配置体系：默认安全/行为皆可配/危险分级（格式化永远强确认） |

## 4. 总体架构

```
┌──────────── 前端 React 19 (WebView2, GPU 合成) ────────────┐
│ 引导向导/画廊(虚拟网格)/看图器/搜索/任务中心/日志/去重/统计/设置  │
└────── ▲ Tauri events(节流推送) / asset://协议(缩略图) ───────┘
┌─────── │ ────────────── Rust 核心 ─────────────────────────┐
│ 设备监控(卷+WPD) → 导入引擎(ImportStage 链) → SQLite(WAL)    │
│        → 缩略图三级管线 → AI索引队列(ort/DirectML)            │
│ 事件总线(领域事件 broadcast) · 配置中心(热应用) · 注册表(扩展点) │
└─────────────────────────────────────────────────────────────┘
```

| 领域 | 选型 |
|---|---|
| 框架 | Tauri 2.x（托盘常驻、单实例、通知、自动更新官方插件） |
| 前端 | React 19 + TypeScript + Vite；motion 动画；Zustand；TanStack Query + Virtual；Tailwind 4 + shadcn/ui；react-router；react-i18next |
| 数据库 | SQLite（rusqlite，WAL，FTS5）+ usearch(HNSW 向量) |
| EXIF | kamadak-exif（JPEG/TIFF 类 RAW） |
| RAW | rawler 提内嵌预览；libraw 完整解码（精览按需） |
| 视频 | ffmpeg/ffprobe 侧车（-hwaccel d3d11va 抽帧） |
| AI | ONNX Runtime（ort crate）DirectML（回退 CPU）：CLIP ViT-B/32 FP16、SCRFD、ArcFace |
| 图像处理 | libjpeg-turbo 解码、fast_image_resize（AVX2 SIMD） |

**GPU 加速落点**：AI 推理 DirectML；视频抽帧 DXVA/NVDEC；UI WebView2 合成层；图像 SIMD；看图器 GPU 纹理平铺渲染。

## 5. 模块设计

### 5.1 设备监控与源适配器

```rust
trait DeviceSource {
  fn list(&self) -> Vec<FileEntry>;       // 路径/对象ID、大小、修改/拍摄时间
  fn open_head(&self, id) -> 头部流(≤1MB); // EXIF 探测
  fn stream(&self, id) -> 全文件流;
}
```

- `VolumeSource`：盘符设备，std::fs，2-4 并发流，FILE_FLAG_SEQUENTIAL_SCAN
- `MtpSource`：WPD COM（IPortableDevice/Content/Resources），1-2 并发流；属性批量枚举；属性缺拍摄时间时读头 1MB 解 EXIF；Seek 不可用时查重降级为 大小+文件名+头部哈希；拔线→任务暂停，重连差量续传
- 检测：`RegisterDeviceNotification`（卷接口 + WPD 接口双注册），`WM_DEVICECHANGE`
- 托盘常驻（空闲 <50MB 内存），可选开机自启，关闭=最小化托盘

### 5.2 导入引擎

单文件单次读取流水线（读卡同时完成哈希+EXIF+写入）：

```
读卡(8MB缓冲, N并发) ├─ 在线哈希 xxHash64+SHA-256
                     ├─ 头部1MB：EXIF+类型识别(扩展名+魔数)
                     └─ 写目标 .part → 校验 → 原子重命名
→ SQLite 批量事务(500/批) → 缩略图队列 → AI 队列
```

- 查重分层：路径同 名→(大小+mtime)→（读卡器源）首尾 64KB 局部哈希→全量哈希；命中按配置跳过/重命名（`_1` 后缀）/询问
- 断点恢复：`job_files` 表记录每文件状态；崩溃/强退后重启询问 继续/回滚/放弃；拔卡暂停可续传
- 取消为软取消（完成当前文件即停）；单文件失败不中断整批
- ImportStage 中间件链（复制/哈希/校验/缩略图/入库均为 stage）：F2 双目的地、导入转码等未来以新 stage 注册
- 目录/命名模板：令牌系统（{YYYY}/{MM}/{DD}/{相机}/{镜头}/{原文件名}…），默认 `年/日期/原文件名`
- F1 清卡：仅列哈希校验通过且已入库文件；二次确认显示数量/容量；直接删除（可移动设备回收站不释放空间，不走回收站）；清单入日志；"自动格式化"默认关+硬安全阀（零跳过零失败才执行）+首次开启红色强警告
- F4 监视文件夹：notify 目录监听，大小稳定 1.5s 触发，移动模式入库
- 原地索引：扫描+哈希+元数据入库（external 标记），不动文件；离线标灰

### 5.3 元数据提取

JPEG/TIFF（含 NEF/ARW/CR2/DNG）kamadak-exif；CR3/RAF 等经 rawler；视频 ffprobe（拍摄时间优先取 creation_time，回退文件修改时间）。字段：拍摄时间、相机、镜头、ISO/光圈/快门/焦距、GPS、方向、宽度高度、时长/编解码（视频）。

### 5.4 数据库 Schema（要点）

```
assets: id, path, filename, size, mtime, xxhash, sha256, kind(photo/raw/video),
        captured_at, camera, lens, iso, aperture, shutter, focal, gps_lat/lng,
        orientation, w, h, duration, rating, flag, color, xmp_synced,
        source(imported/external/watched), offline, burst_group_id, phash,
        thumb_ready, ai_indexed, created_at
tags(asset_id, tag, score)                -- CLIP 场景标签
embeddings: usearch 索引文件 + asset_id 映射表
faces: id, asset_id, box, quality, person_id
people: id, name, cover_asset_id
jobs: id, type(import/cleanup/index/export), device, status, stats_json, started/finished_at
job_files: job_id, src, dst, size, state(pending/copying/verified/failed/skipped), error
logs: ts, level, job_id, message
fts: FTS5(filename, camera, lens, tags) external content
devices: 序列号, 显示名, 方案 JSON（每设备独立记忆）
settings 由 settings.json 承载（带 schema_version）
```

### 5.5 缩略图管线

内嵌预览提取（RAW 不完整解码）→ libjpeg-turbo 解码 → fast_image_resize → 三级缓存（256 网格 / 1080p / 全尺寸按需），JPEG q85 落盘 `dbDir/cache/thumbs/`（哈希分片目录，20GB LRU），内存 LRU 2GB。看图器：内嵌全尺寸预览→GPU 纹理平铺缩放平移；"精览"按钮按需 libraw 半尺寸 demosaic。视频封面：ffmpeg 硬解定点抽帧。

**存储模型（达芬奇式，用户规定）**：见 §5.11——全局配置固定在应用配置目录；库=独立数据单元（数据库目录 dbDir 自包含 + 照片根 photoRoot 分离，本机默认 I:\SmartPhoto\<库名> / Y:\照片），应用绝不向照片根写任何缓存类数据。

### 5.6 AI 索引

- 队列统一 IndexTask 接口；优先级 导入>缩略图>AI；可暂停/限速（CPU 上限默认 50%）/仅空闲
- CLIP 图像嵌入（batch 32-64 FP16 DirectML）→ usearch HNSW（20 万×512 维 <10ms）
- 人脸：SCRFD 检测 + ArcFace 特征 → 在线聚类（阈值可调）→ people 命名
- F5 场景标签：CLIP 零样本（~20 类提示词）
- F3 选片：连拍分组（同相机+时间相邻+参数近似）→ 清晰度（拉普拉斯方差 SIMD）+曝光异常+眨眼 → 最佳帧标记
- 首次引导三选一（全开/仅语义/全关）；人脸数据可一键清除；全部本地

### 5.7 搜索

结构化（日期/相机/镜头/焦段/ISO/光圈/类型/评分/目录/GPS 有无）多条件组合 + keyset 分页；FTS5 关键词；语义自然语言（CLIP 文本向量 HNSW）；人脸按 person；场景标签筛选。防抖 150ms，结果流式分页推送。

### 5.8 导出与 LR 互通

- LR watched folder：一键复制/移动选定照片到 LR Classic 监视目录（手动配置路径）
- 系统交互：资源管理器显示/复制路径/打开方式
- 目录交付：按筛选导出保持分类结构的文件夹
- 评分互通：rating/flag/color 存 DB + 自动写 XMP 边车（LR 可读；RAW 永不修改；外部库默认不写可开）

### 5.9 前端与 UI

- 设计语言：深色三级底（#0E0F12/#16181D/#1E2128）、文字三级、单一琥珀强调色 #F0A83C；Segoe UI Variable + 雅黑 UI；日志/数值 Cascadia Mono
- 画廊：Google Photos 式 justify 不等宽网格+日期吸顶分组+虚拟滚动；连拍组折叠为一叠可展开
- 看图器：沉浸全屏+底部胶片条+EXIF 抽屉+组内对比模式+视频内建播放（HEVC 缺编解码检测提示+系统回退）
- 动效：共享元素过渡（缩略图→看图器）、FLIP 重排、180-240ms ease-out，全部合成层
- 页面：引导向导（库位置→方案→AI 三选一→监视/LR 可选）/画廊/搜索/看图器/导入向导（A 密度三栏）/任务中心/日志/去重工具/统计/人物/设置；Win11 Mica 主窗口（可选）

### 5.10 配置体系

三入口（引导向导/设置页/高级折叠区）+ 两机制（上下文"记住此选择/不再询问"转配置；每设备序列号独立方案）。settings.json schema_version 迁移，热应用（SettingsChanged 事件）。三原则：默认安全、行为皆可配、危险分级（格式化永远强确认，不可配置为静默）。

配置项总目录见 §10。

### 5.11 存储模型与迁移（达芬奇式库管理，用户规定 2026-09-18）

**层级结构**：
1. **应用安装目录**：不可变（程序本体）
2. **全局配置**：`settings.json` 固定于应用标准配置目录（`%APPDATA%\com.smartphoto.app\`）——只存 UI 偏好、**库注册表**、当前激活库 id 等轻量全局项；位置固定，无自举问题
3. **库（Library）= 独立数据单元**（类似达芬奇的数据库）：`{ id, name, dbDir, photoRoot }`
   - `dbDir` 数据库目录**自包含**：library.db（SQLite）、缩略图缓存、向量索引、日志（默认 `I:\SmartPhoto\<库名>`）
   - `photoRoot` 照片存储目录（本机默认 `Y:\照片`），与数据库目录分离
   - 多库：`settings.libraries[]` + `activeLibraryId`；v1 引导创建首库，库切换 UI 后续里程碑；一次激活一个库，库间数据完全隔离（assets 等表都在各库自己的 library.db 里）

**迁移设计**（目录可配 + 修改后自动迁移，均为库级操作）：
- 换 `dbDir`：整库目录自包含 → 两阶段移动（复制+逐文件校验 journal → 原子改注册表指向 → 旧目录留 `.bak`）；中断不丢数据
- 换 `photoRoot`：模式A「仅切换」（旧照片转外部目录语义，零风险）/ 模式B「迁移照片」（复用导入引擎移动模式 + journal + 资产路径批量更新，可暂停恢复）
- 实现：dbDir 迁移与 photoRoot 模式B 引擎侧随 M2；设置 UI 随 M4

## 6. 性能设计

**验收预算**（USB3 读卡器+NVMe+RTX 3060+8 核）：

| 指标 | 目标 |
|---|---|
| 导入吞吐 | ≥ 读卡器带宽 90%（基准模式显示 MB/s） |
| 插卡到弹窗 / 扫卡统计(2000 文件) | <1s / <2s |
| EXIF+入库 | >2000 张/分钟（与复制重叠） |
| 1000 张 RAW 缩略图 | <30s |
| 画廊冷启动首屏 | <1s |
| 结构化搜索 20 万张 / 语义 | <50ms / <10ms(HNSW) |
| 打开 100MB RAW | <200ms（内嵌预览） |
| 首次全库 AI 索引 | GPU ~20-40 分钟，进度可视可暂停可限速 |

手段：单次读取流水线、分层查重、批量事务、SIMD、GPU 批推理、keyset 分页、三级缓存、事件节流、IPC 零 base64（asset:// + ETag）。首启动 5 秒微基准自动调参（并发/批大小）。性能面板实时显示 IO/CPU/GPU/吞吐。打包附 Defender 排除指引（实时扫描可砍 20-30% 复制速度）。

## 7. 可靠性设计

- `.part` 临时文件+校验+原子重命名；任何时刻断电不留半文件
- journal（jobs/job_files）驱动恢复；重启检测未完成任务询问 继续/回滚/放弃
- 拔卡/目标盘掉线：暂停+通知，重连续传
- 错误分级：单文件失败不阻塞；总结汇总+一键重试失败项
- 所有任务可取消（软取消）；日志 SQLite+tracing 文件双写

## 8. 扩展性设计

七大 trait+注册表扩展点（均有 v1 内置实现吃狗粮）：DeviceSource / MetadataProvider / Classifier / ImportStage / 命名模板令牌 / ExportTarget / IndexTask。

事件总线领域事件：DeviceArrived/Removed/Unavailable、ImportSessionStarted/Finished、FileProgress(节流)、ImportPaused/Resumed、ThumbnailReady、AssetIndexed、PersonClustered、IndexQueueChanged、ErrorOccurred、SettingsChanged。

版本化：DB migration（只加不改）、settings schema_version、缓存格式版本（不符自动重建）、命令命名空间（import.*/search.*/ai.*）。

v1 明确不做：运行时插件加载（API 按可暴露标准设计）、主题市场、自定义 SQL、跨进程 IPC。

## 9. 里程碑（合计约 16 周，每阶段交付可用版本）

| 阶段 | 内容 | 预估 |
|---|---|---|
| M0 | 骨架+托盘常驻+配置框架+首次引导向导 | 1.5 周 |
| M1 | 设备检测（卷+WPD）+导入引擎全套 | 3 周 |
| M2 | F1 清卡+F2 双目的地+原地索引 | 1.5 周 |
| M3 | 元数据入库+多维搜索+FTS | 1.5 周 |
| M4 | 缩略图管线+画廊+看图器+评分 XMP+LR 导出 | 2 周 |
| M5 | AI：CLIP+HNSW+人脸聚类+引导选择+F5 | 3 周 |
| M6 | F3 AI 选片 | 1 周 |
| M7 | F4 监视文件夹+F6+F8+F9 | 1.5 周 |
| M8 | 打磨：动画、异常路径、性能面板、自动更新、安装包 | 1.5 周 |

## 10. 默认值清单（设置页全部可改）

| 模块 | 默认值 |
|---|---|
| 设备与导入 | 每次弹窗；仅新文件；年/日期/原文件名；查重跳过+记录；校验自动按盘型；并发自动；里程碑通知；总结弹窗 |
| 导入后清理 | 弹清理对话框；格式化默认关+硬安全阀 |
| 双目的地 | 关 |
| AI | 引导三选一；仅空闲；CPU 50%；GPU 开（回退 CPU） |
| 画廊/看图 | 三档缩略图；连拍折叠开；按拍摄时间排序；EXIF 收起；视频预览自动播放关+静音；精览按需 |
| 文件写入 | XMP 导入库开/外部库关；库数据库目录默认 I:\SmartPhoto\<库名>（自包含可迁移）；监视文件夹默认空 |
| 系统 | 自启关；关闭=最小化托盘；系统+应用内通知；中文；稳定更新通道 |
| 高级 | 日志 info；性能面板可显示 |

## 11. 风险

- WPD/MTP：COM 交互繁琐、各相机兼容性差异 → 首版覆盖主流 PTP 相机，异常设备给明确提示；读卡器路径始终可用
- 新 RAW 格式滞后：rawler 社区跟进延迟 → 未识别格式按扩展名分类、无预览降级
- 人脸聚类阈值调参：M5 预留；阈值用户可调
- LR watched folder 是唯一自动化通道（目录库封闭格式）
- Defender 实时扫描拖慢复制：文档+安装引导排除
- 20 万+ 库首次 AI 索引耗时：进度管理+可暂停+GPU 加速缓解

## 12. 工程结构

```
smart_photo/
├─ src-tauri/src/
│  ├─ main.rs, lib.rs, ipc/           # 命令层（命名空间）
│  ├─ devices/                        # 热插拔 + VolumeSource/MtpSource
│  ├─ import/                         # 引擎/stage链/journal/模板/查重/清卡/监视文件夹/原地索引
│  ├─ metadata/                       # exif/rawler/ffprobe
│  ├─ db/                             # 仓储/迁移/FTS
│  ├─ thumbs/                         # 三级管线/缓存
│  ├─ ai/                             # ort/CLIP/SCRFD/ArcFace/聚类/选片/HNSW
│  ├─ export/                         # LR watched folder/目录交付/XMP
│  ├─ events.rs, settings.rs, registry.rs, bench.rs
├─ src/                               # React
│  ├─ features/{onboarding,gallery,viewer,search,import,tasks,dedup,stats,people,settings}
│  ├─ components/ stores/ hooks/ ipc/ i18n/
└─ docs/specs/ docs/plans/
```

## 13. 测试策略（全期强制标准）

- **测试跟上实现**：任何功能不合入无测试的实现；Rust 任务 TDD（先红后绿）；修 bug 必先写回归测试
- **自动化**：`cargo test`（单测+tempdir 集成测试）+ 前端 vitest + @testing-library/react（store 逻辑/向导状态机/兜底合并/组件交互）；里程碑验收含"测试全绿"硬门，CI 脚本一键跑全部
- **易错路径重点覆盖**（专项测试清单）：哈希与分层查重、断点恢复/中断续传、命名模板（令牌/非法字符/冲突后缀/无EXIF回退）、DB 迁移幂等与旧版本兼容、WPD 对象解析与错误分支、事件节流、损坏 JSON 恢复、并发流下的 journal 状态机、大库 keyset 分页边界
- **真实素材只读测试**：Y:\照片（约 1.85 万文件，JPG/NEF/ARW/视频/XMP）可用于原地索引/EXIF/去重的只读验证；一切写入测试只用 tempdir
- 手动验收清单随里程碑归档（热插拔/真机 WPD 等无法自动化的项）
