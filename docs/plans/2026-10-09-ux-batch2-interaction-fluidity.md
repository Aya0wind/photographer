# UX 批次二:交互流畅三件(预取/查看器/轮询)

> 状态:计划定稿(2026-10-09)。来源:AfterFrame 对比探索(体验/性能专项),参考仓库 `I:/projects/AfterFrame/apps/desktop/src/`(只读,下称 AF)。
> 定位:中等工作量,三项互相独立。依赖批次一不强(1.4 的失败 TTL 与 2.1 的调度器有概念重叠,先做 1.4 更顺)。

## 前置纪律(同批次一,见 `2026-10-09-ux-batch1-anti-flicker.md`)

动画红线:查看器**切图无动画**(既有定案,本批次 2.2 的"揭示"是图层 visibility 切换而非动画);交互路径只走 transform/opacity;全部改动纳入 motion 开关不影响(本批次三项均为行为策略非动画)。

---

## 2.1 邻张预取"停稳才发"(settle 300ms)

**现状**:`src/features/gallery/lib/useViewerImage.ts`(约 :146-164)查看器打开/切图时**立即**预取 ±1 邻张;快速连按方向键会给每个路过的资产排解码任务(6000 档),既浪费又挤占当前张带宽(仅靠信号量 high 插队缓解)。

**方案**:
- 预取包一层 settle 定时器:index/assetId 变化即清旧定时器,新位置**停稳 300ms** 后才发起 ±1 预取;期间再变则重置——快速翻页全程零预取请求;
- 当前张(主图)不设延迟,立即请求且优先级最高(沿用现有信号量插队);
- `since` 锚点语义:定时器从位置最后变化时刻起算,即使预取队列后到也要等完剩余时间(AF `hdPreviewStore.js:100-132` 的 since 实现可参考)。

**AF 参考**:`AF:hooks/hdPreviewStore.js:14`(`HD_PREFETCH_SETTLE_MS = 300`)与 `:100-132`(位置变化记录 since,泵循环里计算剩余等待);注释思想:"用户已经翻过去的照片不再被想要,所以永远不发送"。

**验收**:快速连按 5 张只产生首尾两次预取(可用请求计数断言);停稳 300ms 后 ±1 预取发生;当前张始终立即。

**测试**:useViewerImage 单测(fake timers + mock 请求计数器):连按只排当前张,停稳后预取 2 张。

---

## 2.2 查看器高清层"交互期隐藏 + 停稳 180ms 揭示"

**现状**:`src/features/gallery/components/ViewerOverlay.tsx` + `src/features/gallery/lib/useViewerTransform.ts`——双图层(内嵌 JPEG 先显 / 后台 2048 高清替换,`useViewerImage.ts:95-128`)已比 AF 单层强,但拖动/缩放期间高清全尺寸位图参与逐帧合成重采样,交互期掉帧风险在高分辨率屏放大。

**方案**:
- 给高清层(2048/原图层)加交互态协议:拖拽/滚轮缩放**进行中**时高清层 `visibility: hidden`(或 opacity 0,无过渡——这不是动画是状态切换),底层(内嵌 JPEG 档)继续跟手(它尺寸小,重采样便宜);
- 交互停止 **180ms** 后(settle)揭示高清层,揭示时 transform 已同步(同一 view 状态派生),无需过渡动画;
- 新交互开始立即重置 settle 定时器并再隐藏;
- 与"切图无动画"定案完全兼容:切图本来就是同步换层,此协议只管交互期间的层可见性。

**AF 参考**:`AF:components/Lightbox.jsx:13`(`DETAIL_SETTLE_DELAY_MS = 180`)、`:230-290`(`beginDetailInteraction`/`settleDetailInteraction`);关键注释:"detail 层只在静止时跟 transform——全尺寸位图绝不在滚轮/拖拽的每一帧被重采样"。

**验收**:放大后拖动画面流畅(高清层不参与合成);停止操作 ~200ms 后高清细节出现;连续操作期间不闪烁揭示。

**测试**:ViewerOverlay/useViewerTransform 用例:模拟拖动中高清层 hidden、停 180ms 后可见(fake timers)。

---

## 2.3 TaskDrawer 轮询三件套(diff 跳过 + 自停 + poke 唤醒)

**现状**:`src/features/tasks/TaskDrawer.tsx:41`(`INDEX_POLL_MS = 1500`)与 `:73-80`——`setInterval` 常驻轮询索引状态,**每个 tick 无论数据变没变都 setState**,任务存活期内整棵订阅树每 1.5s 重渲一次;空闲时也持续 IPC;后端瞬时失败时任务卡片可能凭空消失(无退避语义)。

**方案**(三件一起,对照 `AF:hooks/useJobs.js` 整文件):
1. **diff 跳过**(`AF:useJobs.js:99-105`):轮询结果先 `JSON.stringify` 与 ref 中上次值比对,不变直接 return 不 setState。落点:TaskDrawer 的轮询回调 + `src/stores/aiStore.ts` 的 refreshIndexStatus(或在其消费侧);
2. **自停 + 唤醒**(`AF:useJobs.js:130-135, 153-160`):无活跃任务时清掉定时器睡觉;任何任务启动事件(导入开始/扫描启动/索引 kick,我们已有 EventBus→前端转发)调 `poke()` 以 **250ms** 延迟踢醒轮询——250ms 是给刚启动的任务一点落地时间;**seed 机制**:poke 可携带刚注册的 jobId,防"任务在第一次 poll 前就结束"丢完成副作用(我们的任务完成 toast/总结依赖轮询发现);
3. **失败退避**(`AF:useJobs.js:84-97`):轮询请求失败 ≠ 没有任务——保留已知活跃集合,3s 退避重试,绝不清空任务列表(防后端卡死时 UI 任务全部消失)。

**验收**:空闲 5 分钟零 IPC(可断言 mock invoke 调用次数);任务启动后 ≤1s 内开始轮询;数据无变化的 tick 不触发组件重渲(渲染计数断言);模拟一次轮询失败任务卡不消失。

**测试**:TaskDrawer/aiStore 单测:diff 跳过(渲染计数)、自停/唤醒、失败退避三用例。

---

## 分期与验证

三项独立,建议顺序 2.3(收益面最大)→ 2.1 → 2.2。每项完成后:

```
export PATH="$HOME/.local/nodejs:$HOME/.cargo/bin:$PATH"
node node_modules/typescript/bin/tsc --noEmit
node node_modules/vitest/vitest.mjs run src/features/tasks src/features/gallery src/stores
```

2.3 涉及后端事件转发时(若需新增任务启动事件),改 Rust 侧后补跑受影响 cargo 目标。

## 明确不做

- 不做全局 HD 调度器(那是批次三 3.3,2.1 只改查看器局部预取时机);
- 不给 settle 揭示加过渡动画(visibility 直切);
- 不动后端轮询接口契约。
