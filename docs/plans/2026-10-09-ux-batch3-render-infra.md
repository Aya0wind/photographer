# UX 批次三:渲染体系件(预解码/缓存/调度器/Worker 池)

> 状态:计划定稿(2026-10-09)。来源:AfterFrame 对比探索(体验/性能专项),参考仓库 `I:/projects/AfterFrame/apps/desktop/src/`(只读,下称 AF)。
> 定位:较大工作量,六项按依赖与收益排序;每项独立可验收,可拆给不同执行者。

## 前置纪律(同批次一,见 `2026-10-09-ux-batch1-anti-flicker.md`)

补充:本批次涉及全局架构件(调度器/Worker 池),新增模块放既有目录惯例位置(`src/features/gallery/lib/`、`src/features/editor/lib/`、`src/lib/`),命名与注释风格跟随现有代码(中文注释、决策写注释)。

---

## 3.1 AssetThumb 图片 IntersectionObserver 预解码

**现状**:全库 grep 无一处 IntersectionObserver;缩略图 `<img>` 挂载即设 src,快速滚动时视口外图片也竞争解码;只有个别 `loading="lazy"`。

**方案**:`src/features/gallery/components/AssetThumb.tsx` 加 IO 懒挂 src:
- `root` 传滚动容器 ref(**不是**默认 viewport——网格是内滚容器;AssetGrid 的滚动容器需透传或 context);
- `rootMargin: "600px 0px 600px 0px"`(视口上下各提前 600px;比虚拟化 overscan 略小——IO 管图片解码、虚拟化管 DOM,两者独立,AF 注释明说);
- `threshold: 0.01`(露出 1% 即触发);
- 命中即 `observer.disconnect()`(一次性);
- 缓存图竞态兜底:blob:/热缓存图可能在 React 挂 onLoad 前同步完成导致事件丢失——用 ref 回调检查 `img.complete`(AF:`AF:components/PreviewImage.jsx:57-59`);
- 顺带 `decoding="async"`。

**AF 参考**:`AF:components/PreviewImage.jsx:25-42, 57-59, 63`。

**验收**:滚动时图片在进入视口前 ~600px 开始加载(可用加载顺序断言);已滚过的图片不再加载;虚拟化卸载的行重挂时若已加载直接显示。

**测试**:AssetThumb 用例(mock IO 或 jsdom 手动触发):视口外不设 src、进入 rootMargin 设 src、complete 兜底。

---

## 3.2 页面级 stale-while-revalidate 缓存(相册/回忆页)

**现状**:相册首页、回忆页(MemoriesPage)等页面数据无页面级缓存,换库/刷新/返回时整页重拉,期间闪空或骨架(批次一 1.2 解决了"旧数据不清",但页面卸载重挂后旧数据也没了)。

**方案**:做一个小型页面缓存工具 `src/lib/pageCache.ts` + 各页数据 hook 接入:
- 缓存键 = `数据库id#revision`(revision 用现有的 catalogRevision 等价物——我们的事件体系 photoLibrariesChanged/databasesChanged/资产变更事件可汇聚成一个自增 revision,见现有 settingsStore/aiStore 的事件计数模式);
- **LRU 容量 2**(只留最近两个 revision,`AF:DiscoverView.jsx:86`);
- **latestByCatalog**:每数据库存最后一份完整页;revision miss 时**先渲染旧页**再后台拉新页(不闪空);
- 新页到达后原子替换;同键命中同步返回(首帧完整渲染)。

**AF 参考**:`AF:components/DiscoverView.jsx:36-44(pageCache/latestByCatalog 结构), 69-88(LRU2 + 命中逻辑), 187-206(revision miss 先渲染旧页)`;预热点 `AF:App.jsx:256-262`(目录就绪后 300ms prefetch,revision 变化重排)——我们可暂不做预热,缓存先行。

**验收**:相册页→画廊→返回相册:内容即刻显示(缓存命中),若数据已变则随后静默更新;换数据库后不显示旧库内容(键隔离)。

**测试**:pageCache 单测(命中/LRU 淘汰/旧页先渲染);接入页 1-2 个行为用例。

---

## 3.3 HD/原图按需调度器(全局单飞 + 优先级)

**现状**:查看器 2048 生成、选片(culling)、编辑器高清源各自直接请求后端,无全局排队;多个消费方并发时互相挤占,当前查看照片可能排在路过预取后面。

**方案**:新建 `src/features/gallery/lib/hdRequestStore.ts`(纯 TS 外置 store + `useSyncExternalStore`):
- **全局单飞**:同时只有 1 个 HD 生成请求在途(后端缩略图服务侧也有信号量,这里是前端请求侧的全局仲裁);
- **优先级**:`当前查看的资产 > 显式批次(未来拼图/批量导出)> 邻居预取`;预取请求在排队期间失去关注(用户翻走了)即丢弃——"用户已经翻过去的照片不再被想要"(`AF:hdPreviewStore.js:177-194` nextWork 语义);
- **want/release 声明式接口**:消费方(查看器/选片/编辑器)声明 `want(assetId, {kind: "current"|"prefetch"})`,卸载/失焦 `release`;
- **settle 300ms** 与 **失败 15s 冷却** 复用批次二 2.1 与批次一 1.4 的实现(此处是它们的全局化);
- **generation 隔离**:换数据库时 generation+1,在途过期结果落地即丢弃(`AF:hdPreviewStore.js:76-92` setCatalog);
- 消费端订阅粒度:**每个视图只订阅自己那张图的结果**,store 其他活动不触发本视图重渲(`AF:hooks/useOnDemandHdPreviews.js:45-49`);快照用原始值(状态枚举/版本号)而非对象,规避引用相等陷阱。

**AF 参考**:`AF:hooks/hdPreviewStore.js` 整文件(266 行,头注释即架构说明,可作实现蓝本)。

**验收**:查看器与选片同时请求时当前查看者优先;翻走的预取被丢弃;换库后在途结果不落地;单飞期间其余请求排队不报错。

**测试**:hdRequestStore 单测:优先级排序、丢弃、generation、单飞串行(用假请求)。

---

## 3.4 像素处理 Worker 池(滤镜/导出/批量缩略)

**现状**:全部像素处理(PhotoCraft 渲染、导出合成、批量调色)在主线程;大图导出/批量滤镜会冻结 UI 数秒。`grep "new Worker"` 全库无使用。

**方案**:新建 `src/features/editor/lib/pixelPool.ts`(参考 `AF:components/editor/lut/lutPool.js` 276 行):
- **池大小** `min(4, 硬件并发-1)`;
- **大图按行分带**:全尺寸处理拆为行带,一带一 worker 并行(100MP 图不再长任务冻结;主线程只保留 canvas 合成最终一份拷贝);
- **transferable 零拷贝**:像素 buffer `postMessage(data, [buffer])` 转移所有权;**错误路径必须把 buffer 原样转回** caller 重试(否则像素凭空丢失——AF 教训注释 `AF:lutWorker.js:31-36`);
- **worker 失败降级**:`file://` 或环境不支持时置 workersBroken,降级主线程按 512 行小带循环(避免单条长任务);
- **显式释放**:编辑器/导出器关闭时 `releaseWorkers()` terminate 全部并清缓存(像素表是 MB 级,不留僵尸);
- 首个接入方建议:PhotoCraft 导出合成(收益最大且边界清晰),后续滤镜预览/批量再接。

**AF 参考**:`AF:components/editor/lut/lutPool.js:18(池), 97-144(请求/缓存被挤归还 buffer), 218-266(分带+释放)`,worker 侧 `AF:lutWorker.js`(worker 内 LRU + transferable)。

**验收**:大图导出期间 UI 可交互(滚动不卡);结果与主线程处理逐像素一致(同输入对比);worker 不可用时功能不坏(降级路径)。

**测试**:pixelPool 单测(worker 可用性 mock):分带正确合并、降级路径、释放后无泄漏引用。像素一致性用一个中小图 fixture 断言。

---

## 3.5 memo 稳定转发器(防内联回调废掉 AssetGrid memo)

**现状**:`src/features/gallery/components/AssetGrid.tsx` 的卡片/行组件已有 memo,但父层(GalleryPage/AssetGrid 自身)传给 cell 的回调若是内联闭包,**memo 永远不 bail**——每次轮询 tick/无关 state 变化都重渲全部可见卡片(AF 同类坑实测注释原话:"otherwise the memo never bails and every poll tick re-renders every visible card")。

**方案**:
- 审计 AssetGrid 传给每格的全部 props,找出身份不稳定的回调与对象;
- 建 `handlersRef` 模式:`const handlersRef = useRef({...}); handlersRef.current = { onSelect, onOpen, ... }`(每渲染直写最新闭包),对外暴露 `useCallback(..., [])` 的稳定转发器转发到 ref.current;
- 行/列尺寸等派生对象用 useMemo;
- 把 AF 的教训注释翻成中文写在转发器上方。

**AF 参考**:`AF:components/Gallery.jsx:574-583`(handlersRef + 4 个稳定转发器)。

**验收**:渲染计数测试——无关 state 变化(如任务徽标 tick)时可见卡片组件不重渲(可用测试计数 hook 断言)。

**测试**:AssetGrid 用例:父重渲时 cell 渲染计数不增。

---

## 3.6 平滑滚动例外分支(跳转/时间线)

**现状**:PhotoTimeline 跳转与画廊 reveal 使用 smooth 滚动;跨大距离(数千行/跨筛选)平滑滚会**一路触发沿途每张缩略图加载**,几秒内产生请求风暴;虚拟化行也要一路实例化。

**方案**:`src/features/gallery/components/PhotoTimeline.tsx` 与画廊 scrollIntoView 调用点:
- 同屏/短距离(如 < 2 屏)保持 `behavior: "smooth"`;
- 跨大距离/跨筛选/外部 reveal 请求用 `behavior: "auto"`(瞬时),配合 3.1 的 IO 预解码后沿途请求自然消失;
- 判定阈值抽成常量并写注释。

**AF 参考**:`AF:components/Gallery.jsx:875-878`(注释:"平滑滚过几千行会一路加载缩略图");reveal 居中分支 `AF:Gallery.jsx:839-879`。

**验收**:时间线跳到很远月份时瞬时到位、无沿途图片请求风暴(请求计数断言);短距离跳转仍有平滑。

**测试**:PhotoTimeline/AssetGrid 行为用例(mock scrollTo 记录 behavior 参数)。

---

## 分期与验证

依赖关系:3.1 独立;3.2 独立;3.3 依赖/收编 2.1+1.4(先做批次一二);3.4 独立(建议导出场景先行);3.5 独立(半小时级);3.6 建议在 3.1 之后。

建议顺序:**3.5 → 3.1 → 3.6 → 3.2 → 3.4 → 3.3**(先把便宜的防重渲与预解码做掉,重型调度器殿后)。

每项完成后:

```
export PATH="$HOME/.local/nodejs:$HOME/.cargo/bin:$PATH"
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vitest/vitest.mjs run <受影响目录>
```

涉及 Rust 侧(3.4 若后端配合分带、3.3 若调整信号量)时补跑受影响 cargo 目标(NASM/PATH 见仓库记忆)。

## 明确不做

- 不换虚拟化方案(@tanstack/react-virtual 保留,AF 自研方案不引入);
- 不做 prefetchDiscover 式启动预热(3.2 预热项后置,等页面缓存稳定);
- 不做 Performance 基准测试与 FPS 采集(行为断言为准);
- 不动后端缩略图三档/LRU 语义。
