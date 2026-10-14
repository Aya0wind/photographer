# RawLab / LibRaw 移植实施记录

关联：[专项色彩合同](2026-10-12-libraw-smart-preview-film.md)、[阶段实施顺序](platform-evolution/implementation.md)、[roadmap](platform-evolution/roadmap.md)。

## 2026-10-14：第一批 M0 原型

移植前先将 dev 合入 main 并推送，基线合并提交 `fcf3257`。main 上最新编辑器改动保留，dev 同步该合并后开展移植。RawLab 参考源码固定为 `dcecbf82189360288085ee2d8017ed44f1eef919`。

### 本批代码

路线由用户确认：**直接复用固定版本 RawLab core，通过薄 Rust 封装接入**。不自行移植数学，不把 RAW 显影扩展为重写算法。

| 位置 | 已落地内容 |
| --- | --- |
| `native/rawlab/upstream` | 未修改的 RawLab core 源码快照，包含显影/胶片/平台 GPU 和必要依赖声明/补丁，不含客户端、LUT、RAW 样片 |
| `native/rawlab/CMakeLists.txt` | 固定 LibRaw 0.22.2 与官方 CMake 适配；配置上游 core 能力，原样应用其补丁 |
| `native/rawlab/src/photographer_raw.cpp` | float C ABI；直接调用 RAWProcessor、core 色彩/LUT 和 Metal/D3D11/GLES photo renderer；无 GPU 的 Linux 使用 CPU |
| `crates/media/src/rawlab.rs` | 可信库加载、版本/参数映射、编码标记、原生拥有的 RGB 借用与同库释放；请求 GPU 与实际成功分开 |
| `src-tauri/src/edit/photocraft.rs::linear_rgb_document` | float linear sRGB/D65 到 F32 PhotoCraft tiles，匹配 ICC，不提前裁掉负值/超白 |
| `src-tauri/examples/rawlab_probe.rs` | RAW → 线性文档交接 → core 中性/胶片 CPU/GPU → PhotoCraft PNG；原片只读，禁止覆盖输出 |
| `.github/workflows/media-core.yml` | Linux/macOS/Windows 原生构建、C ABI 与 Rust 跨边界用例 |
| Windows/macOS 安装包 CI | 原型示例和线性文档交接测试的编译检查 |

选择先交付 M0 验证入口；不跳过 M0–M4 把 Log LUT 直接套到旧显示 RGB 上。crate 暂用仓内 path 依赖，M1 再统一根 workspace/锁文件。主应用加载源、配方、预览与导出仍沿用既有后端；尚未开放新后端开关。上游已有 C ABI 只有 RGB8/RGBA8，所以浮点桥是必要的表示/生命周期适配，不是显影或胶片算法重写。

不捆绑第三方 LUT。使用 core 识别 Gamma/Gamut 合同的用户 CUBE，未知输入明确失败，DCP 待后续。归属/具体边界见 [原生说明](../../native/rawlab/README.md)。

### 实际验证证据

- 两份上游源码包下载和 SHA256 已核对，原样 RawLab 补丁对临时 LibRaw 0.22.2 源码完成静态应用检查。源码快照来源/更新方式记录在 [UPSTREAM.md](../../native/rawlab/UPSTREAM.md)。
- Rust 格式/语法解析、锁定 Cargo metadata、自有代码的 diff 空白检查执行；上游 `gles_lut.cpp:244` 原有行尾空白随未修改快照保留，不将清理空白混入上游算法快照；遵循编译交 CI 的约定，未本地构建/运行应用、样片或测试。
- 已写 C ABI 失败/取消/中性范围、Rust 合成 Log LUT 强度/CPU-GPU Auto 对照、GPU fallback/Force 和 PhotoCraft 浮点/ICC 交接测试。真实 RAW 生命周期用例需私有样片，显式 ignored。CI 结果待发布，不能计作已通过。
- 未验证实机 GPU 成功、全部目标原生库、代表相机画质、首帧延迟、峰值 RAM/VRAM 或 30 fps。M0 尚未达到完整出口，M5 未完成。

### 接下来实施顺序

1. **收口 M0**：以新 CI 修复跨平台编译问题；用 Bayer、X-Trans、DNG/压缩 RAW 样片记录浮点颜色、高光、方向、特殊几何/半尺寸、耗时与内存。旧 PhotoCraft、LibRaw/RawLab 的对比要使用同原图、明确曝光基准。不能以单张 DNG 测试代表全部机型。
2. **组织与共享边界 M1–M4**：按现有总计划迁根 workspace、稳定模型、来源与预算，提取现有处理链。现有工作代码优先搬迁/适配，不在这次原型里重写它们。
3. **LibRaw 新来源**：显式配方/后端版本；线性基础代理保留浮点范围，固定白平衡/曝光基准。所有后台 RAW 任务纳入宿主预算；源变化/切图/关闭丢弃过期结果。不可中断段仍持有许可。
4. **Smart Preview**：先 JPEG 可编辑，异步代理缓存、损坏重建、最新配方候选帧及原子切换。专项列出的视口、工具、历史和蒙版坐标必须保留。
5. **胶片编辑**：在正确色彩阶段增加资源身份/内容版本及面板；直接复用 core 的色域/Log/LUT 与平台 GPU，接通持久纹理/会话及 CPU 回退；是否再迁 wgpu 以数据交接和实测瓶颈决定，不重复实现算法。普通创意 LUT 位于后续，胶片/强度不能重新显影。
6. **导出/兼容**：完整 RAW 与代理共享色彩/配方合同，冻结资源版本，基准/WB/exposure 不重复应用。旧配方明确保持旧像素语义。再验收 CPU/GPU、导出/代理和真实交互性能。

### 本批边界与性能债务

OpenMP 暂关闭，宿主预算接入后再选择 C++ 并行。RawLab loadFile 已经 unpack，当前像素检查不代表解码前内存保证；完整字节预算和源租约仍待应用层接入。上游未公开中途取消，桥只在前后丢弃过期结果，库调用结束前不得释放许可。

LibRaw 内部仍有16位处理，不宣称全浮点去马赛克。float film 入口有输入复制/输出读回，Windows/Android renderer 目前一次性创建；尚未接持久缓存、原生呈现和编辑配方，所以不是最终实时帧循环。full 源尺寸为最终方向化尺寸，half-size 精确坐标未验证、返回 None；不可用于既有蒙版坐标。详细限制与复现入口见 [native/rawlab](../../native/rawlab/README.md) 和 [crates/media](../../crates/media/README.md)。


## 2026-10-14：上游更新管理与持久色彩会话

最新集成分支刷新为 `origin/development` 的 `89cdc5b14402ee9265468f54152c8fb55d661248`，167 个文件原样快照。此前的第一批版本保留在 git 历史。更新脚本只读取独立 RawLab checkout 的 git 对象，完整替换所选源码树，删除过时文件；有本地修改时拒绝覆盖。锁文件记录 commit、逐文件 SHA256、整体摘要和 LibRaw/CMake 下载地址校验值；CI 离线校验源码完整性后构建，不在每次构建时自动跟随上游分支。

新版 core 的 JPEG XL DNG 依赖使用上游 DNG SDK/libjxl 构建配置，默认开启；libtiff 同样使用上游固定依赖，Lensfun 暂关闭。未修改上游算法。后续更新先运行候选检查，再审核 C++/CMake/色彩合同变化并更新快照，接口适配始终留在快照外。

桥 ABI 升为 2，Rust 同步检查版本并映射 options 布局。新增不跨线程的持久色彩会话：复制 immutable float 源一次，Windows/Android 保留 renderer，通过 revision 复用源上传；曝光、相对 RGB WB、胶片切换/强度复用该来源。CPU 从同一原始线性数据计算，Auto 在 GPU 报失败或抛异常时回退，Force 明确失败。相对 RGB WB 不等同于 RAW 去马赛克前传感器 WB。中性 CPU 路径减少一个全图复制；使用胶片时仍保留中性对照图。输出独立拥有像素，释放会话不会提前释放已返回的帧。

已补充 CI 回归：重复会话渲染、曝光/WB 等价输入、取消/非法参数后恢复、源图不被修改、会话释放后输出仍有效、原始 CUBE 色彩合同查询。静态检查与上游完整性核对在本地执行，应用/原生构建和测试交 CI；未验证实时帧率、GPU 实机表现或画质出口。

本批仍未替换编辑器主链。下一批接宿主预算与工作线程、版本化线性代理缓存，然后接最新配方候选帧/原子切换、胶片资源面板与导出。旧配方继续沿用已有后端，M0 的代表 RAW 样片验收和各阶段出口仍须完成。


## 2026-10-14：改为 Git submodule 管理上游

按用户指定，将 `native/rawlab/upstream` 改为 GitHub RawLab 仓库的 submodule，固定同一 commit `89cdc5b14402ee9265468f54152c8fb55d661248`。主仓库不再跟踪上游的逐文件源码副本；之前快照管理的描述属于历史实现，由本节取代。完整上游树存在于子模块 checkout，构建/安装包仍只选择 core 能力，不自动引入上游应用或素材。

`.gitmodules` 声明 GitHub URL 与 development 更新分支，实际构建版本以主仓库 gitlink 为准。锁文件改为 version 2，只保存 commit、上游身份和原生依赖下载校验值。更新工具默认检查子模块，拒绝本地修改，候选审核后使用 `--write` 同步 detached checkout 和锁文件；gitlink 与锁文件一起提交。三平台 CI 递归检出并验证版本，CMake 拒绝未初始化或与锁不一致的 core。操作示例见 [UPSTREAM.md](../../native/rawlab/UPSTREAM.md)。

本批只调整上游引用方式，色彩会话 ABI 与编辑器接入顺序不变。已执行子模块状态/来源/版本与工具静态检查；未本地编译应用或执行应用测试。


## 2026-10-14：主编辑器接入批次

本批把已选固定 core 接入实际编辑路径，仍由用户显式启用胶片显影；历史配方不添加新字段或改变后端。

| 部分 | 本批实现 |
| --- | --- |
| 配方/兼容 | 独立 colorPipeline v1，固定 coreRevision；未知或不同版本明确拒绝，胶片不进入局部蒙版参数 |
| 资源/前端 | 内容寻址的原始 CUBE 和目录，保留 Gamma/Gamut；胶片选择、启停、强度、导入；复用原有手势历史；不捆绑第三方 LUT |
| 色彩工作线程 | 同线程创建/渲染/销毁 native 会话，预览队列有界；两个源/上传估算 128 MiB、30 秒闲置回收；后续曲线/HSL/LUT 不使色彩源失效 |
| 来源/Smart Preview | 先用已有 JPEG RGB，后台 LibRaw 生成 float linear 代理；prepare 暂存、activate 按当前配方生成完整候选帧后换源；前端候选解码并追上期间的新编辑后发布 |
| 缓存/生命周期 | 代理身份/版本/尺寸、像素 checksum/有限值/边界检查、损坏重建、512 MiB 磁盘裁剪；关闭/切图丢弃过期结果；源变化拒绝激活 |
| 显示/读数 | JPEG 和 Windows 原生预览共用色彩阶段，原图对比用中性显示；取色、直方图、曝光提示一致；蒙版不重复执行全局胶片链 |
| 导出 | 完整 RAW decode 与色彩阶段共持资源许可，消费曝光/WB 后交 PhotoCraft；完整色彩会话不占预览工作队列，仍保留现有 ICC/元数据/几何/格式链 |
| 便携方案 | 打包原始胶片 CUBE，按内容 ID 校验；新色彩方案格式 v2，旧方案 v1 继续接收，旧应用不会把新方案静默当旧配方加载 |
| 构建/分发 | Windows/macOS CI 构建固定子模块 core 并装入资源，macOS 签名 native 库；许可随应用，对应源码作为 artifact/release 资产；没有脚本应用运行时 |

新增原生色彩阶段回归验证“曝光只消费一次、下游改动复用色彩图、原生预览/导出一致、旧配方不进入新链、core 版本拒绝”；float 缓存覆盖负值/超白和损坏/尺寸拒绝；前端覆盖显式启用、准备期间编辑、关闭后丢弃。CI 提供实际构建的桥运行 host/filter 用例。本地执行 TypeScript noEmit、Rust 语法/格式、JSON/YAML/Python 静态解析、子模块版本和 diff 检查；未本地编译或运行应用/回归测试。

仍未完成 M0 代表相机样片、CPU/GPU 色差、代理/完整导出误差、峰值 RAM/VRAM 与交互帧率验收。JPEG 快速代理与 RAW 色彩不是同一物理来源；相对 RGB WB 不等于去马赛克前绝对 Kelvin；LibRaw/DNG SDK 内部额外内存和线程不能仅凭宿主许可视为严格受限。M1–M4 全平台目录/来源/预算统一、DCP/HDR 与更细的任务优先级仍按 roadmap 推进，本批不宣称这些阶段已完成。

补充收尾：无可用内嵌/JPEG 代理时，显式新后端可以直接准备 LibRaw 来源；后台准备失败作为提示保留临时 RGB 编辑。线性源批量读取瓦片，避免进入显示合成的范围夹取；完整 RAW 导出直接 decode → native 色彩会话 → U16 显示瓦片，绕过浮点文档往返。缓存校验涵盖尺寸头和像素。跨 native/source/renderer 的集成用例位于 edit/tests，单测与源模块相邻。
