import { useSettingsStore } from "@/stores/settingsStore";

/**
 * 动画体系（M4.5）：统一开关与过渡预设。
 * - useMotionOn()：读 settings.appearance.animations（默认开）；关闭时 AppShell 根
 *   节点挂 .no-motion（CSS 全局灭动画）+ motion 组件 initial=false（不进过渡）
 * - 预设只允许 transform / opacity（GPU 合成层）；严禁 width/height/top/left 等
 *   布局属性动画——布局型收合改为 transform 位移 + 固定尺寸
 */

/** 过渡预设（motion 组件 transition 用；均为合成层属性） */
export const TRANS = {
  /** 快速淡入/滑出（卡片、提示条） */
  quick: { duration: 0.15, ease: "easeOut" } as const,
  /** 抽屉/面板滑出 */
  slide: { duration: 0.18, ease: "easeOut" } as const,
  /** 交叉淡入（图片层提交） */
  fade: { duration: 0.15, ease: "easeOut" } as const,
};

/** 界面动画是否开启（settings.appearance.animations；默认 true） */
export function useMotionOn(): boolean {
  return useSettingsStore((s) => s.settings.appearance?.animations ?? true);
}

/** initial 值：动画开=给定初始态；关=false（直接渲染到 animate 态，无过渡） */
export function motionInitial<T extends object>(enabled: boolean, initial: T): T | false {
  return enabled ? initial : false;
}
