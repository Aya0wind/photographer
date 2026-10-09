# UX 批次一:防闪烁四件(加载态与失败恢复)

> 状态:计划定稿(2026-10-09)。来源:AfterFrame 对比探索(体验/性能专项),参考仓库 `I:/projects/AfterFrame/apps/desktop/src/`(只读参考,下称 AF)。
> 定位:小改动、立竿见影。四项互不依赖,可单独实施与验收。

## 前置纪律(全批次通用,违反即返工)

- **动画体系红线**(既有定案):全部动画受设置开关控制(motion 三件套 `src/lib/motion.ts` + `.no-motion`);查看器切图无动画;路由仅入场;严禁布局属性动画(width/height/top/left),过渡只走 transform/opacity。
- **测试**:轻量、快;只跑受影响模块(`node node_modules/vitest/vitest.mjs run <files>` + `node node_modules/typescript/bin/tsc --noEmit`);不写性能测试;`data-testid` 跟随现有规范。
- **环境**:前端命令先 `export PATH="$HOME/.local/nodejs:$HOME/.cargo/bin:$PATH"`。
- 不 git commit/push(由主会话统一处理)。

---

## 1.1 加载指示延迟显示(400ms 档)

**现状**:我们的骨架/加载指示在数据请求发出时立即挂载。快加载(<400ms)会闪一下骨架再出内容,形成"闪一下"的生硬感。落点:`src/features/gallery/components/AssetThumb.tsx`(骨架挂载,当前约 :124-127)与 `src/features/gallery/components/ViewerOverlay.tsx`(大图加载 spinner)。

**方案**:加载指示组件加"延迟出现"包装——请求开始后启动 400ms 定时器,期间内容已就绪则取消定时器、全程不显示任何指示;超时才显示骨架/spinner。实现为一个可复用 hook(如 `src/lib/useDelayedFlag.ts`):`const visible = useDelayedFlag(pending, 400)`。

**AF 参考**:`AF:components/Lightbox.jsx:354-359`——"Loading large file"文字 400ms 后才显示;另一档 `AF:components/editor/components/EditorLoading.jsx:13,20-22`——慢任务的原因解释文案 1200ms 后才出现(HINT_AFTER_MS,只有真的慢才解释)。两档思想一致:**把加载指示本身当成可能闪烁的东西来防**。

**验收**:
- 快加载(<400ms,可用测试假数据/mock delay)全程无骨架出现;
- 慢加载 400ms 后骨架出现且内容到达即消失;
- ViewerOverlay 大图 spinner 同语义。

**测试**:AssetThumb/ViewerOverlay 各补 1-2 个用例(fake timers 断言 399ms 无指示、401ms 有指示)。

---

## 1.2 只在真空时显示加载骨架

**现状**:`src/features/gallery/pages/GalleryPage.tsx`(约 :382-385)筛选态/换库时 setStatus("loading") 整个网格骨架化——旧数据还在却先清空显示骨架,产生"网格→转圈→网格"双闪。

**方案**:保留旧数据静默换新。骨架仅在 `assets.length === 0 && loading` 时出现;有旧数据时照常渲染旧网格,新数据到达后原子替换(现有 store 更新即是),可加一个轻度的"更新中"暗示(如顶部细进度条,受 motion 开关控制)但不阻塞旧内容。

**AF 参考**:`AF:components/Gallery.jsx:993-1004`——中心 spinner 只在 `!items.length && (loading || !browserReady)` 时出现,注释明说防双闪;`AF:components/DiscoverView.jsx:191`——revision 未命中时旧页留在屏上等新页。

**验收**:切换筛选/照片库过滤时旧网格不消失、无骨架闪现,数据到达后内容更新。

**测试**:GalleryPage 补用例:有旧 assets 时切筛选不出现 loading testid;真空时出现。

---

## 1.3 骨架脉冲只在 pending 时挂载(含 CPU 注释规约)

**现状**:审计 `src/features/gallery/components/AssetThumb.tsx`(骨架类 `sp-skeleton`,约 :124-127)与全局 `grep sp-skeleton`:确认失败态/空图态/已完成态是否残留无限脉冲动画。

**背景教训**(AF 实测,须抄进我们代码注释):无限脉冲动画会让 Chromium 合成器逐帧合成,仅画廊静置就吃掉 GPU 进程约 35% CPU——**加载完成/失败后必须摘掉动画类**,动画只在 `pending` 状态存在。

**AF 参考**:`AF:components/PreviewImage.jsx:44-51`——`loading = src && !loaded && !errored` 时才挂 `animate-pulse`,原注释:"Pulse only while a load is pending. An infinite animation left under a loaded (or failed) image makes Chromium composite every frame forever: ~35% CPU in the GPU process with the gallery just sitting there."

**方案**:骨架动画类条件化(仅 pending);把上述 CPU 教训写成中文注释规约放进 `src/styles/app.css` 的 sp-skeleton 定义处或 AssetThumb 顶部;顺带审计其他无限动画(ai-shimmer 类)是否同样只在运行中挂载。

**验收**:加载完成后的卡片 DOM 上不再有 animate-pulse/sp-skeleton 动画类;失败态用静态占位(图标或暗块)不用脉冲。

**测试**:AssetThumb 断言 settled 后 className 不含动画类。

---

## 1.4 缩略图失败缓存改 TTL 重试(替代永久 failed)

**现状**:`src/features/gallery/lib/thumbPipeline.ts:152` 附近——失败结果进 `failedCache` **永久缓存**,只能等 `thumbnailReady` 事件整体失效;瞬时失败(解码线程超时/磁盘忙)后该资产缩略图永不恢复,直到重启。

**方案**:
- `failedCache` 的值从结果改为 `{ result, failedAt }`;查询命中失败缓存时,若 `Date.now() - failedAt > 15_000` 视为未缓存(允许重试)并删除条目;
- **再成为主图立即重试**:查看器当前资产(或画廊 hover 选中)请求失败档时无视 TTL 直接清失败记录重排(用户显式关注优先于防风暴);
- 成功后正常写 thumbCache。

**AF 参考**:`AF:hooks/hdPreviewStore.js:13`(`FAILED_RETRY_MS = 15_000`)、`:56-70`(失败条目带 failedAt,过期或资产成为 current 时 forgetFailure 重排)。

**验收**:注入一次失败(可 mock thumb_file 返回 None/坏文件)后:15s 内重试仍被拦截;15s 后(或该资产在查看器打开)重试成功并显示。

**测试**:thumbPipeline 单测(fake timers):失败→TTL 内命中拦截→过期→重试路径;主图强制重试路径。

---

## 分期与验证

四项可并行,建议顺序 1.3(审计+注释)→ 1.1 → 1.2 → 1.4。每项完成后跑:

```
export PATH="$HOME/.local/nodejs:$HOME/.cargo/bin:$PATH"
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vitest/vitest.mjs run src/features/gallery src/lib
```

## 明确不做

- 不引入骨架交错/FLIP 等新动画(AF 也没有);
- 不改 motion 三件套开关语义;
- 不做性能基准(以行为断言为准)。
