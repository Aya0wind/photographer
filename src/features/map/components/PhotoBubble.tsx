/**
 * 照片气泡（命令式 DOM 工厂，非 React 组件）：
 * maplibre Marker 需要原生 element；每气泡一个 react root 代价大且无必要。
 * 结构 = 叠放照片卡（主图 + 最多两张副图错位）+ 数量徽标 + 名称标签。
 * 入场 stagger 纯 CSS（--stagger 变量），受全局 .no-motion 契约约束。
 * 缩略图由 MapCanvas 懒加载补 src（data-asset 契约）。
 *
 * 【定位纪律】maplibre v6 把 marker 定位 transform 写在自定义元素的内联
 * style 上——**任何以 fill 模式跑 transform 的 CSS 动画都会永久压过内联值**
 * （层叠规则：animation > inline），气泡就全瘫在画布左上角。因此入场动画
 * 和 hover 缩放一律放在内层 .map-bubble-in，marker 元素绝不碰 transform。
 *
 * 【点击语义】气泡本体点击 = 下钻/县层放大（onDrill）；hover 露出的
 * 「N 张 →」角标（可聚焦、aria-label）与样图缩略图点击 = 打开该地区照片
 * 子页（onOpenPhotos），两者均 stopPropagation 不冒泡到下钻。
 */

import type { MapCluster } from "@/ipc/api/map";

export function createBubbleElement(
  cluster: MapCluster,
  index: number,
  onDrill: () => void,
  options: {
    /** 角标/样图点击 → 打开该地区照片（MapPage 导航 /map/photos） */
    onOpenPhotos: (regionId: number) => void;
    /** 角标文案（含数量，由调用方 i18n 插值，如「12 张 →」） */
    photosLabel: string;
  },
): HTMLElement {
  const root = document.createElement("button");
  root.type = "button";
  root.className = "map-bubble";
  root.dataset.region = String(cluster.regionId);
  root.title = `${cluster.name} · ${cluster.count}`;
  root.setAttribute("aria-label", `${cluster.name}, ${cluster.count} photos`);

  // 内层动画载体（外层是 maplibre 定位元素，transform 禁触）
  const inner = document.createElement("span");
  inner.className = "map-bubble-in";
  inner.style.setProperty("--stagger", `${Math.min(index, 20) * 35}ms`);

  const stack = document.createElement("div");
  stack.className = "map-bubble-stack";

  /** 样图点击 = 打开照片子页（不冒泡到气泡本体的下钻） */
  const openPhotos = (e: Event) => {
    e.stopPropagation();
    options.onOpenPhotos(cluster.regionId);
  };

  // 副图（后两张，错位垫底；正序反向插入保证 samples[0] 在最上层）
  const extras = cluster.samples.slice(1, 3);
  for (let i = extras.length - 1; i >= 0; i -= 1) {
    const extra = document.createElement("div");
    extra.className = `map-bubble-photo map-bubble-extra-${i + 1}`;
    extra.dataset.asset = String(extras[i].id);
    extra.addEventListener("click", openPhotos);
    stack.appendChild(extra);
  }

  // 主图（img 由懒加载器补 src；初始占位底色 + 渐变骨架）
  const main = document.createElement("img");
  main.className = "map-bubble-photo map-bubble-main";
  main.alt = "";
  main.draggable = false;
  main.dataset.asset = String(cluster.samples[0]?.id ?? "");
  main.addEventListener("click", openPhotos);
  stack.appendChild(main);

  const badge = document.createElement("div");
  badge.className = "map-bubble-badge";
  badge.textContent = cluster.count > 999 ? `${Math.floor(cluster.count / 1000)}k` : String(cluster.count);
  stack.appendChild(badge);

  // 「N 张 →」角标（hover 露出；可聚焦、Enter/Space 触发；span role=button——
  // 根元素已是 button，嵌套交互元素用 span 承载避免非法 HTML）
  const photosBtn = document.createElement("span");
  photosBtn.className = "map-bubble-photos";
  photosBtn.setAttribute("role", "button");
  photosBtn.setAttribute("tabindex", "0");
  photosBtn.setAttribute("aria-label", options.photosLabel);
  photosBtn.textContent = options.photosLabel;
  photosBtn.addEventListener("click", openPhotos);
  photosBtn.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      options.onOpenPhotos(cluster.regionId);
    }
  });
  stack.appendChild(photosBtn);

  const name = document.createElement("div");
  name.className = "map-bubble-name";
  name.textContent = cluster.name;

  inner.appendChild(stack);
  inner.appendChild(name);
  root.appendChild(inner);
  root.addEventListener("click", (e) => {
    e.stopPropagation();
    onDrill();
  });
  return root;
}
