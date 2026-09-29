# 拍摄地图模块（maplibre-gl 世界地图 + GPS 照片分层气泡）

## 背景·目的

用户的照片资产已含 GPS 坐标（`assets.gps_lat/gps_lon`，exif_lite 提取 + gen-2 自愈回填），
但没有可视化手段。本模块在器材统计（/gear）之下新增「拍摄地图」页面：

- **世界地图**（maplibre-gl，WebGL 本地渲染，暗色主题匹配应用）
- 把每张带 GPS 的照片逆地理编码为 国家/省/市/县 行政区划并入库
- 按**缩放层级**分层聚合展示：全球视角→每国家几张代表图；国家视角→每省/州几张；
  省视角→每市几张；市视角→县/照片点。**点击气泡即 flyTo 下钻**，镜头丝滑
- 代表图随机采样（每次换一批），气泡卡片化（圆角照片 + 数量徽标 + 入场动画）
- 定位：展示用，好看炫酷优先；只读不写照片

## 现状整理

| 现状 | 结论 |
|------|------|
| `assets.gps_lat/gps_lon REAL`（db/migrations.rs:243-244，exif_lite.rs:162-173 提取） | GPS 数据就绪，无需新采集 |
| 缩略图链路：`assetThumbGet` IPC + `convertFileSrc`（AssetThumb.tsx） | 气泡代表图直接复用 256 档缓存 |
| AI 模型下载链路（features/ai + ipc/api/ai.ts：进度事件 + 入库任务） | 地理数据包分发复用此模式（安装包不膨胀） |
| supervisor 后台任务 + bus 进度事件（`refresh_exif_for_generation` 模式） | 行政区划回填任务照抄此模式 |
| 侧边栏四组（AppShell/Sidebar.tsx），器材统计在「组织」组（Sidebar.tsx:166） | 新模块挂「组织」组 /gear 之后 |
| 动画体系：motion/react + useMotionOn + .no-motion 全局灭 | 气泡动画接入既有契约 |
| 依赖：Cargo 无 geo/rstar；package.json 无 maplibre-gl | 全部新增 |

## 设计

### 数据与许可（关键决策）

- **底图**：OpenFreeMap 暗色矢量瓦片（`https://tiles.openfreemap.org/styles/dark`）——
  免费、无 key、无限量，数据 ODbL、样式 BSD，与 GPLv3 分发兼容。maplibre-gl 本身即
  客户端 WebGL 渲染，满足「本地渲染」。离线 PMTiles 中国包列入未来扩展。
- **行政区划（逆地理编码，全部离线点包含）**：
  - 基础包：geoBoundaries 简化版 **ADM0（国家）+ ADM1（省/州）**，CC-BY 4.0（署名可再分发）
  - 中国增强：阿里 DataV GeoJSON **省/市/县三级**（精度与中文名最好）
  - 可选增强包：geoBoundaries **ADM2（全球市级行右）**，默认不下载
  - 层级深度定案（2026-09-29）：**最深到市/县（level 3）**，不做更深层数据包
    开关，**街道级明确不做**（避免地图数据膨胀）；气泡副文案显示到县 + 坐标
- **分发**：不进安装包。首次进入地图页检测数据缺失 → 引导下载到 `dbDir/geo/`
  （基础包 ~60-100MB，下载任务入库 + 进度事件，复用 AI 模型下载 UI 模式）

### 后端（Rust）

```
src-tauri/src/geo/
├── mod.rs        # 行政区数据加载（dbDir/geo/ 下缓存格式）、层级模型
├── contains.rs   # R-tree（rstar）预筛 + geo::Contains 精判；分层解析：
│                 #   国家→省→市→县 沿树逐级 narrowing（先命中国家再只查其子节点）
├── backfill.rs   # supervisor 后台任务：gps 非空资产 → 树上解析 → 写 asset_regions
│                 #   （gen 标记文件防重入，模式照抄 refresh_exif_for_generation；
│                 #   增量数据包到达后可按 source 只刷对应子树）
└── download.rs   # 数据包下载（URL 清单 + 校验 + 进度事件）
```

- **树缓存 JSON（2026-09-29 用户定案：前端快速加载）**：数据包加载/更新后把
  regions 树一次性导出 `dbDir/geo/regions-cache.json`（嵌套 children 结构：
  id/parent/level/name/code/lat/lon，几千节点仅几 MB；缓存指纹 = 各数据包
  `source` 版本串，指纹不符即重建）。前端进地图页直接 `fetch`（convertFileSrc）
  读缓存拿全树做层级 UI 与下钻，**不走 IPC 递归查询**；聚合计数仍走 IPC
  （动态真值，随资产增删变化）。

- **库表**（新 migration，开发期直接改 schema 重排索引——现库可重建）。
  **树形地区索引（2026-09-29 用户定案：基于 GPS 解析结果的独立索引，地图功能只消费索引）**：
  ```sql
  -- 地区树：一节点一行，全部元数据单份存储（中心/层级/编码/数据包来源）
  CREATE TABLE regions (
    id INTEGER PRIMARY KEY,
    parent_id INTEGER REFERENCES regions(id), -- 根节点（世界）为 NULL
    level INTEGER NOT NULL,                   -- 0 国家 /1 省·州 /2 市 /3 县·区
    name TEXT NOT NULL,                       -- 本地化名（zh 优先，回退 shapeName）
    code TEXT,                                -- 稳定编码（DataV adcode / geoBoundaries shapeISO）
    lat REAL NOT NULL, lon REAL NOT NULL,     -- 气泡锚点（多边形 centroid，加载时算一次）
    source TEXT NOT NULL                      -- 数据包来源+版本（geoboundaries-v1 / datav-v1）
  );

  -- 资产 ↔ 地区挂接：**每资产每层一行**（10 万资产 ×4 层 = 40 万行，SQLite 无压力）
  CREATE TABLE asset_regions (
    asset_id INTEGER NOT NULL REFERENCES assets(id) ON DELETE CASCADE,
    region_id INTEGER NOT NULL REFERENCES regions(id),
    level INTEGER NOT NULL,
    PRIMARY KEY (asset_id, level)
  );
  CREATE INDEX idx_asset_regions_region ON asset_regions(region_id, level);
  ```
  设计动机：
  - **规范不冗余**：地区中心/名称只在树节点存一份（原扁平方案的 8 个浮点列
    ×10 万行冗余取消）；气泡锚点来自 regions 表
  - **聚合/下钻查询形态自然**：任意层级聚合 = `GROUP BY region_id WHERE level=N`；
    下钻带范围 = `AND region_id IN (SELECT id FROM regions WHERE parent_id=?)`
    ——都走 idx_asset_regions_region，无递归 CTE
  - **增量数据包**：基础包（国家+省）先到先回填两级，中国包到达后树上补挂
    市/县节点再增量回填——层级加深不动已有结构
  - **资产→完整地址串**：按 level JOIN 树拼「国家 省 市 县」，供气泡副文案与
    查看器信息面板复用
- **IPC（src-tauri/src/ipc/map.rs）**：
  - `map_geo_status()` → 数据包就绪状态 / 回填进度 / 树缓存指纹
  - `map_geo_cache_url()` → 树缓存文件路径（前端 convertFileSrc 直读）
  - `map_geo_download_start()` → 启动下载任务
  - `map_clusters(level, parent_region_id?)` → `[{ region_id, name, lat, lon,
     count, samples: [{ id, path, kind }] }]`——level 选层、parent 限定下钻范围；
     每组随机 N 张（SQL `ORDER BY random() LIMIT n`；组行数 >5000 时先 COUNT
     再随机 offset）
  - **编辑联动重索引（2026-09-29 用户定案）**：经纬度元数据被编辑修改
    （edit/metadata 保存链路，含置空清除）时，同事务/同任务**重解析该单资产**
    的树上归属并重写其 asset_regions 行（单点 narrowing 毫秒级），广播
    `map://regions-updated`——地图与地址串不出现编辑后滞留旧地区
  - 事件：`map://geo-progress`（下载/回填进度）、`map://regions-updated`（回填完成/单资产重索引刷新）

### 前端（React）

```
src/features/map/
├── pages/MapPage.tsx           # 页面骨架：数据缺失引导 / 回填进度 / 地图画布
├── components/
│   ├── MapCanvas.tsx           # maplibre Map 封装（style JSON、flyTo、zoom 监听）
│   ├── PhotoBubble.tsx         # 自定义 Marker DOM：照片卡（256 档 thumb）+ 数量徽标
│   │                           #   hover 放大、点击 → flyTo 下钻 + 层级刷新
│   └── GeoDataGate.tsx         # 首次下载引导 + 进度条（复用 AI 下载交互模式）
├── lib/
│   ├── hierarchy.ts            # zoom→level 映射（<3.5 国家 / <6.5 省 / <9.5 市 / ≥9.5 县）
│   │                           #   + 防抖（zoom 变化 250ms 稳定后才切层，避免飞行途中抖动）
│   ├── regionTree.ts           # 树缓存 JSON 加载（map_geo_cache_url → fetch 一次，
│   │                           #   内存建 Map<id,node>；下钻链/层级名/地址串都查内存树）
│   └── useMapClusters.ts       # map_clusters 拉取 + 会话内样本缓存（刷新按钮换一批）
└── MapPage.test.tsx 等         # maplibre-gl 全模块 mock（jsdom 无 WebGL）
```

- **路由**：`src/app/routes.tsx` 主壳 children 加 `{ path: "map", element: <MapPage /> }`
- **侧边栏**：Sidebar「组织」组 /gear 后插 `{ to: "/map", labelKey: "nav.map" }`（地球线稿图标）
- **交互细节**：
  - 点击气泡：`map.flyTo({ center, zoom: 目标层级中值, duration: 1.2s, curve: 1.42 })`
  - 下钻后旧层气泡退场（scale+fade）、新层 stagger 入场（motion/react；
    animations 关闭时全部瞬时，遵守 .no-motion 契约）
  - 顶层国家气泡在多国分布时自动错开（简单经纬网格 snap 防重叠，v1 不做碰撞物理）
  - 深层（照片点级）气泡直接展示单张照片卡，点击进查看器（复用 ViewerOverlay 路由）
- **i18n**：zh/en 补 `nav.map`、`map.*`（标题/空态/下载引导/层级名）

### 依赖

- 前端：`maplibre-gl`（BSD-2-Clause，GPLv3 兼容）
- Rust：`geo`（多边形 contains/centroid）、`rstar`（R-tree 预筛）

## 验收标准

1. 首次进入「拍摄地图」：提示下载地理数据（基础包），下载完成后地图出现且自动开始
   回填；回填期间已有结果 progressively 出现，进度可见
2. 全球视角（打开默认）：每个有照片的国家一枚照片气泡（随机代表图 + 数量）；
   点击中国气泡 → 丝滑 flyTo 且切换为省级气泡（每省随机几张）
3. 逐级下钻 省→市→县/照片点 连续点击均飞行切换；层级名（国家/省/市/县）随缩放
   正确切换，飞行途中不闪烁切层
4. 气泡照片为随机样本：点「换一批」按钮后同区域样本变化；hover 有放大反馈；
   动画开关关闭时（settings.appearance.animations=false）无任何过渡
5. 无 GPS 照片的库：页面显示空态文案（不白屏不报错）
6. 编辑某图经纬度（查看器元数据编辑）保存后：地图气泡与地址串即时迁到新地区；
   清除 GPS 后该图从地图索引消失
7. 10 万资产库（沙盒造数）：任一层级聚合查询 <300ms，地图交互不掉帧（缩放/拖动 60fps 档）

## 实施步骤

### Phase 1: 后端地理基础（可独立验收）

1. 新 migration：`regions` 树 + `asset_regions` 挂接 + 索引；`geo` 模块骨架
   （`geo/mod.rs` 数据目录约定 + 树加载器：数据包 → regions 节点 + centroid）
2. 数据包下载：`geo/download.rs` + `map_geo_status`/`map_geo_download_start` IPC +
   `map://geo-progress` 事件（判据：手动触发下载后 dbDir/geo/ 出现校验通过的包文件）
3. 点包含引擎：`geo/contains.rs` R-tree + 分层 narrowing；单测覆盖
   （判据：`cargo test geo` 通过——含北京→中国/北京市/海淀区、纽约→美国/纽约州、
   海上坐标→国家 None、边界点容差等用例）
4. 回填任务：`geo/backfill.rs` supervisor 任务 + gen 文件防重入 +
   `map://regions-updated` 事件（判据：沙盒库跑完后 asset_regions 行数 = GPS
   非空资产数 × 已装层级数）
5. 树缓存导出：包加载/更新后写 `regions-cache.json`（指纹 = source 版本串；
   判据：删缓存文件后重进地图自动重建，指纹一致时不重写）

### Phase 2: 聚合查询（依赖 Phase 1）

1. `map_clusters(level, parent_region_id?)` IPC：按层分组（`WHERE level=N
   [AND region_id IN parent 子节点]`）+ 计数 + 随机样本（含大组随机 offset 优化）
2. 事件刷新挂接（导入收尾 / tags-indexed 同款时机重拉）
3. 编辑联动：edit/metadata 保存 GPS 变化（含清除）→ 单资产树上重解析 →
   重写 asset_regions 行 + `map://regions-updated`（判据：改坐标后气泡/地址串
   即时迁移，清 GPS 后该图从地图索引消失）
4. 单测：分组正确性、样本数上限、随机性（两次调用样本集合不同）、
   parent 下钻范围过滤（子节点外不串组）、编辑联动前后挂接行断言

### Phase 3: 前端地图壳（依赖 Phase 1 数据包就绪，可与 Phase 2 并行）

1. 依赖接入 maplibre-gl；路由 /map + 侧边栏项 + i18n；MapPage 骨架与 GeoDataGate
   下载引导（判据：npm test 通过 + 真机 CDP 9223 进页面见暗色世界地图）
2. MapCanvas：style 加载、暗色调参（背景 #0E0F12 融入）、缩放/拖动性能确认
3. maplibre-gl 测试 mock 层（jsdom 无 WebGL，全模块 mock）+ MapPage 空态/引导单测

### Phase 4: 分层气泡交互（依赖 Phase 2+3）

1. PhotoBubble 组件（thumb 加载复用 AssetThumb 管线、数量徽标、hover 态）
2. zoom→level 层级状态机 + 250ms 防抖 + flyTo 下钻（点击气泡把该节点
   region_id 作为 parent 传入，center+zoom 飞行）
3. stagger 入场/退场动画（motion/react；.no-motion 契约测试）
4. 「换一批」样本刷新 + 会话缓存；照片点级气泡点击进查看器

### Phase 5: 打磨与压力验收

1. 沙盒 10 万资产（含 GPS 比例 60%）压测：聚合耗时、marker 数量上限
   （同屏 >200 气泡时聚合抽稀）
2. 多国/跨洲数据集视觉验收（CDP 截图）；空态、数据包损坏重下路径
3. 文档：geoBoundaries CC-BY 4.0 + OSM ODbL 署名进「关于/设置」页脚

## 验收标准追溯

| 验收标准 | 实现它的任务 |
|----------|--------------|
| 1 首次下载引导 + 回填进度 | Phase 1-2/1-4、Phase 3-1 |
| 2 国家→省 flyTo 下钻 | Phase 4-2/4-3 |
| 3 连续下钻不闪烁 | Phase 4-2（防抖）、Phase 5-2 |
| 4 随机样本 + 换一批 + 动画契约 | Phase 2-1、Phase 4-3/4-4 |
| 5 无 GPS 空态 | Phase 3-3 |
| 6 编辑 GPS 联动重索引 | Phase 2-3 |
| 7 10 万资产性能 | Phase 2-1（offset 优化）、Phase 5-1 |

## 并行策略

- Phase 1 与 Phase 3-1/3-3（前端壳 + mock，不依赖真实数据）可双 lane 并行
- Phase 2 依赖 Phase 1 的表结构；Phase 4 依赖 2+3 汇合
- geoBoundaries 数据包脚本（下载/校验/量化）与代码开发可并行（一次准备，多处使用）

## 验证方法

- `cargo test geo` / `cargo test map`：点包含矩阵、聚合、随机采样（exit 0）
- `npm test`：MapPage/hierarchy/mock 单测（exit 0，maplibre-gl 全 mock）
- 真机：`npm run tauri dev` 后 WebView2 CDP 9223 —— 打开 /map，脚本断言
  `map-page` 容器与 `photo-bubble` 数量随 flyTo 变化；`settings.animations=false`
  时 marker 无 transition
- 沙盒压测库放 `I:\SmartPhoto` 测试库（**绝不指向 Y:\照片**）

## 未来扩展

- 离线底图：PMTiles 中国区域包（protomaps basemap 裁剪），长途外出离线可用
- 照片密度热力图层（heatmap 表达式）与气泡双模式
- 时间轴联动（按拍摄年份过滤气泡——memories 选题联动）
- 按国家/省统计的「足迹报告」卡片（去过 N 国 M 省、最北/最南拍摄点）
