import { useEffect, useRef, useState } from "react";
const MIN_SCALE = 1;
const MAX_SCALE = 4;

/** 舞台手势与临时变换；切换资产时复位。 */
export function useViewerTransform(assetId: number) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  // --- 缩放/平移/旋转状态（资产切换时复位；旋转不持久化） ----------------------------
  const [view, setView] = useState({ scale: 1, x: 0, y: 0, rotation: 0 });
  useEffect(() => {
    setView({ scale: 1, x: 0, y: 0, rotation: 0 });
  }, [assetId]);

  const clampScale = (s: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));
  /** 90° 步进旋转（负=逆时针）；触发拖拽/缩放之外的独立维度 */
  const rotate = (delta: number) =>
    // 保留连续角度，确保 -270 -> -360 的过渡继续向左，而不是归零后反向补间。
    setView((v) => ({ ...v, rotation: v.rotation + delta }));

  // 主预览滚轮始终只负责缩放；前后翻页统一由左右箭头/键盘/胶片条承担。
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const cx = e.clientX - rect.left - rect.width / 2;
      const cy = e.clientY - rect.top - rect.height / 2;
      setView((v) => {
        const next = clampScale(v.scale * (e.deltaY < 0 ? 1.2 : 1 / 1.2));
        if (next === v.scale) return v;
        if (next === MIN_SCALE) return { scale: MIN_SCALE, x: 0, y: 0, rotation: v.rotation };
        const ratio = next / v.scale;
        // 指针为锚：保持光标下的图像点不动
        return { scale: next, x: cx - (cx - v.x) * ratio, y: cy - (cy - v.y) * ratio, rotation: v.rotation };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // 拖拽平移（scale>1 时）：pointer capture，jsdom/老 WebView 缺失时静默退化
  const dragRef = useRef<{ x: number; y: number } | null>(null);
  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>): void {
    if (view.scale <= MIN_SCALE) return;
    // 箭头/工具按钮是操作控件，缩放状态下不能被舞台的 pointer capture 抢走点击。
    if (e.target instanceof Element && e.target.closest("button")) return;
    dragRef.current = { x: e.clientX, y: e.clientY };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // 无指针捕获时 move/up 仍在本元素内生效
    }
  }
  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>): void {
    const drag = dragRef.current;
    if (!drag) return;
    setView((v) => ({ ...v, x: v.x + (e.clientX - drag.x), y: v.y + (e.clientY - drag.y) }));
    dragRef.current = { x: e.clientX, y: e.clientY };
  }
  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>): void {
    dragRef.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 静默
    }
  }

  function resetView(): void { setView({ scale: 1, x: 0, y: 0, rotation: 0 }); }
  function toggleZoom(): void {
    setView(v => v.scale > MIN_SCALE
      ? { ...v, scale: MIN_SCALE, x: 0, y: 0 }
      : { ...v, scale: 2, x: 0, y: 0 });
  }
  return { stageRef, dragRef, view, rotate, resetView, toggleZoom,
    handlePointerDown, handlePointerMove, handlePointerUp };
}
