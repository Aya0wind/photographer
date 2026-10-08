import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useMotionOn } from "@/lib/motion";
const MIN_SCALE = 1;
const MAX_SCALE = 4;

/** 缩放发生后显示；拖动结束再开始计时，切换照片时清理。 */
function useZoomIndicator(assetId: number, scale: number) {
  const [zoomVisible, setZoomVisible] = useState(false);
  const [zoomInteracting, setZoomInteracting] = useState(false);
  const [zoomFocused, setZoomFocused] = useState(false);
  const previousZoom = useRef({ assetId, scale: 1 });
  useEffect(() => {
    const previous = previousZoom.current;
    previousZoom.current = { assetId, scale };
    if (previous.assetId !== assetId) {
      previousZoom.current.scale = MIN_SCALE;
      setZoomVisible(false);
      setZoomInteracting(false);
      setZoomFocused(false);
    } else if (previous.scale !== scale) {
      setZoomVisible(true);
    }
  }, [assetId, scale]);
  useEffect(() => {
    if (!zoomVisible || zoomInteracting || zoomFocused) return;
    const timer = window.setTimeout(() => setZoomVisible(false), 3000);
    return () => window.clearTimeout(timer);
  }, [zoomVisible, zoomInteracting, zoomFocused, scale]);
  return { zoomVisible, setZoomVisible, setZoomInteracting, setZoomFocused };
}

/** 舞台手势与临时变换；切换资产时复位。 */
export function useViewerTransform(assetId: number) {
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [stageSize, setStageSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const element = stageRef.current;
    if (!element) return;
    const update = () => {
      const rect = element.getBoundingClientRect();
      setStageSize((previous) => previous.width === rect.width && previous.height === rect.height ? previous : { width: rect.width, height: rect.height });
    };
    update();
    window.addEventListener("resize", update);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(update);
    observer?.observe(element);
    return () => { observer?.disconnect(); window.removeEventListener("resize", update); };
  }, []);
  // --- 缩放/平移/旋转状态（资产切换时复位；旋转不持久化） ----------------------------
  const [view, setView] = useState({ scale: 1, x: 0, y: 0, rotation: 0 });
  const zoomIndicator = useZoomIndicator(assetId, view.scale);
  const [renderedView, setRenderedView] = useState(view);
  const renderedRef = useRef(view);
  const animationFrame = useRef<number | null>(null);
  const [animating, setAnimating] = useState(false);
  const motionOn = useMotionOn();
  // All image dimensions and offsets use one interpolated frame, never separate CSS transitions.
  const [dragging, setDragging] = useState(false);
  const stopAnimation = () => {
    if (animationFrame.current !== null) cancelAnimationFrame(animationFrame.current);
    animationFrame.current = null;
  };
  const paint = (next: typeof view) => { renderedRef.current = next; setRenderedView(next); };
  useEffect(() => {
    stopAnimation();
    const start = renderedRef.current;
    if (dragging || !motionOn || (start.scale === view.scale && start.x === view.x && start.y === view.y && start.rotation === view.rotation)) {
      paint(view);
      setAnimating(false);
      return;
    }
    setAnimating(true);
    let started: number | null = null;
    const frame = (time: number) => {
      started ??= time;
      const progress = Math.min(1, (time - started) / 120);
      const eased = 1 - Math.pow(1 - progress, 3);
      paint(progress === 1 ? view : {
        scale: start.scale + (view.scale - start.scale) * eased,
        x: start.x + (view.x - start.x) * eased,
        y: start.y + (view.y - start.y) * eased,
        rotation: start.rotation + (view.rotation - start.rotation) * eased,
      });
      if (progress < 1) animationFrame.current = requestAnimationFrame(frame);
      else { animationFrame.current = null; setAnimating(false); }
    };
    animationFrame.current = requestAnimationFrame(frame);
    return stopAnimation;
  }, [view, dragging, motionOn]);
  useEffect(() => {
    stopAnimation();
    const next = { scale: 1, x: 0, y: 0, rotation: 0 };
    paint(next);
    setView(next);
    setAnimating(false);
    setDragging(false);
  }, [assetId]);

  const clampScale = (s: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));
  /** 滑条以舞台中心缩放，并保持当前平移点的相对位置。 */
  function setZoom(scale: number): void {
    const next = clampScale(scale);
    setView((v) => ({ ...v, scale: next,
      x: next === MIN_SCALE ? 0 : v.x * next / v.scale,
      y: next === MIN_SCALE ? 0 : v.y * next / v.scale }));
  }
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
      if (e.deltaY === 0) return;
      const rect = el.getBoundingClientRect();
      const cx = e.clientX - rect.left - rect.width / 2;
      const cy = e.clientY - rect.top - rect.height / 2;
      setView((v) => {
        const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? rect.height : 1;
        const delta = Math.max(-1000, Math.min(1000, e.deltaY * unit));
        const next = clampScale(v.scale * Math.pow(1.2, -delta / 100));
        if (next === v.scale) return v;
        if (next === MIN_SCALE) return { scale: MIN_SCALE, x: 0, y: 0, rotation: v.rotation };
        const displayed = renderedRef.current;
        const ratio = next / displayed.scale;
        // 指针为锚：保持光标下的图像点不动
        return { scale: next, x: cx - (cx - displayed.x) * ratio, y: cy - (cy - displayed.y) * ratio, rotation: v.rotation };
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
    stopAnimation();
    setView({ ...renderedRef.current, rotation: view.rotation });
    setAnimating(false);
    dragRef.current = { x: e.clientX, y: e.clientY };
    setDragging(true);
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
    setDragging(false);
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 静默
    }
  }

  function resetView(): void {
    stopAnimation();
    const next = { scale: 1, x: 0, y: 0, rotation: 0 };
    paint(next);
    setView(next);
    setAnimating(false);
  }
  function toggleZoom(): void {
    setView(v => v.scale > MIN_SCALE
      ? { ...v, scale: MIN_SCALE, x: 0, y: 0 }
      : { ...v, scale: 2, x: 0, y: 0 });
  }
  return { stageRef, stageSize, dragRef, view, renderedView, animating, rotate, resetView, toggleZoom, setZoom, dragging,
    minScale: MIN_SCALE, maxScale: MAX_SCALE, ...zoomIndicator,
    handlePointerDown, handlePointerMove, handlePointerUp };
}
