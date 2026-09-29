import { useEffect, useMemo, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { AssetDto } from "@/ipc/api";
import type { AssetGroup } from "./assetGroups";
import { fetchAssetThumb, useAssetThumbUrl, warmImageDecode } from "./thumbPipeline";

const VIEWER_MID_SIZE = 2048; // 中间档（后端加 2048 档后生效）：原图渲染失败时的清晰回退
/** RAW 内嵌全幅直出档（后端语义：RAW && size > 2048 = 提取最大内嵌 JPEG 原样直出） */
const VIEWER_RAW_EMBED_SIZE = 6000;
const VIEWER_THUMB_SIZE = 1280; // 名义边长；后端 snap 512 档就近

function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** ISO 时间 → "YYYY-MM-DD HH:mm:ss"（本地时区）；空值 "—" */
/** 大图来源、后台解码图层与相邻预取。 */
export function useViewerImage(asset: AssetDto, group: AssetGroup, index: number) {
  const originalUrl = useMemo(
    () => (asset.kind === "photo" ? safeConvert(asset.path) : null),
    [asset.kind, asset.path],
  );
  const [stage, setStage] = useState<"original" | "mid" | "thumb">("original");
  useEffect(() => {
    setStage("original");
  }, [asset.id, originalUrl]);
  const wantsThumb = asset.kind === "photo" || asset.kind === "raw";
  const thumb = useAssetThumbUrl(asset.id, VIEWER_THUMB_SIZE, wantsThumb, "high");
  // 内嵌全幅直出档（>2048 为后端语义标记）：RAW 主显示路径
  const rawEmbed = useAssetThumbUrl(
    asset.id,
    VIEWER_RAW_EMBED_SIZE,
    asset.kind === "raw",
    "high",
  );
  // rawler 2048 显影：仅在「无内嵌预览」（embed 结算为 null）时才启用兜底
  const rawFull = useAssetThumbUrl(
    asset.id,
    VIEWER_MID_SIZE,
    asset.kind === "raw" && rawEmbed.status === "failed",
    "high",
  );
  // 中间档仅 photo 且原图已失败时才请求（2048 档后端就位后生效；未就位时 settled null → 继续降 512）
  const mid = useAssetThumbUrl(
    asset.id,
    VIEWER_MID_SIZE,
    asset.kind === "photo" && stage !== "original",
    "high",
  );

  let mainSrc: string | null;
  let mainFailed = false;
  if (asset.kind === "photo" && stage === "original" && originalUrl !== null) {
    mainSrc = originalUrl; // img onError → 降档；渲染失败前不作无图判定
  } else if (asset.kind === "photo" && stage === "mid") {
    mainSrc = mid.url; // 在途 null → 走加载提示；确定无图由下方自动降档
  } else if (asset.kind === "raw") {
    // 512 秒出 → 内嵌全幅替换变清晰；无内嵌预览 → 2048 显影兜底
    mainSrc = rawEmbed.url ?? rawFull.url ?? thumb.url;
    mainFailed =
      rawEmbed.status === "failed" &&
      rawFull.status === "failed" &&
      thumb.status === "failed";
  } else {
    mainSrc = thumb.url;
    mainFailed = thumb.status === "failed";
  }
  // 源缺失（源文件被移动/删除，管线终态）：512 档在任何回退档位都请求，其 missing
  // 结算即整链缺源信号——有历史缓存时 mainSrc 仍指向缓存 URL（尽力展示），无缓存
  // 时 mainSrc 为 null，靠下方占位兜底（绝不让舞台空白/无限转圈）。
  const mainMissing = thumb.status === "missing";
  const showMissingPlaceholder = mainMissing && mainSrc === null;
  // 中间档确定无图（后端未加 2048 档 / 提取失败）→ 自动降到 512 档；
  // 中间档缺源且无缓存（missing+null，不会再有图）同样降档交给 512 档结算
  useEffect(() => {
    if (
      asset.kind === "photo" &&
      stage === "mid" &&
      (mid.status === "failed" || (mid.status === "missing" && mid.url === null))
    ) {
      setStage("thumb");
    }
  }, [asset.kind, stage, mid.status, mid.url]);
  // photo 无原图可用（非 Tauri 环境 convertFileSrc 抛错）→ 直达 512 档
  useEffect(() => {
    if (asset.kind === "photo" && stage === "original" && originalUrl === null) {
      setStage("thumb");
    }
  }, [asset.kind, stage, originalUrl]);

  // --- 无空窗单层切换 ---------------------------------------------------------------
  // 用稳定 key 保留真正已解码的 DOM 图片节点：新图先在不可见层解码，onLoad 后把同一个
  // 节点提升为可见层并移除旧层。不能只把新 src 写回旧 <img>，否则浏览器仍可能在重新
  // 绘制该节点时短暂清空画面，表现为切图黑闪。
  type ImageLayer = { src: string; phase: "active" | "loading" | "retiring" };
  const [imageLayers, setImageLayers] = useState<ImageLayer[]>([]);
  useEffect(() => {
    if (mainSrc === null) return; // 源在途（等 2048/512 URL）——旧图层继续显示
    setImageLayers((previous) => {
      if (previous.some((layer) => layer.src === mainSrc)) return previous;
      const active = previous.find((layer) => layer.phase === "active");
      return active
        ? [active, { src: mainSrc, phase: "loading" }]
        : [{ src: mainSrc, phase: "loading" }];
    });
  }, [mainSrc]);
  const hasRetiringLayer = imageLayers.some((layer) => layer.phase === "retiring");
  useEffect(() => {
    if (!hasRetiringLayer) return;
    // 新图至少完整绘制一帧后才移除旧图。这样即使 WebView 合成线程比 React 提交稍慢，
    // 也始终有上一张作为后备，不会在两个纹理之间露出黑色舞台背景。
    const frame = requestAnimationFrame(() => {
      setImageLayers((previous) =>
        previous.filter((layer) => layer.phase !== "retiring"),
      );
    });
    return () => cancelAnimationFrame(frame);
  }, [hasRetiringLayer]);
  // 确定无图：清掉残留图层，显示占位
  useEffect(() => {
    if (mainFailed || showMissingPlaceholder) {
      setImageLayers([]);
    }
  }, [mainFailed, showMissingPlaceholder]);

  // 大图加载提示：源在途超过 300ms 才转圈（几十 MB 原图加载慢，避免黑屏误判失败）；
  // 切换期间旧图层兜底显示，仅新图 300ms 仍未 onLoad 才叠加 spinner（快速连按不闪）。
  const [slowLoading, setSlowLoading] = useState(false);
  const awaitingImage =
    imageLayers.some((layer) => layer.phase === "loading") ||
    (mainSrc === null && !mainFailed && !showMissingPlaceholder);
  useEffect(() => {
    setSlowLoading(false);
    if (!awaitingImage) return;
    const timer = setTimeout(() => setSlowLoading(true), 300);
    return () => clearTimeout(timer);
  }, [awaitingImage]);

  // 相邻预取（#7 预加载强化）：前后各 1 张——
  // a) 512 回退档 URL 高优先预取；b) new Image() 解码预热（photo 的原图 asset URL 也预热），
  // 让箭头切换时下一张大概率已在解码器缓存里。
  useEffect(() => {
    const neighbors = [
      index > 0 ? group.assets[index - 1] : null,
      index < group.assets.length - 1 ? group.assets[index + 1] : null,
    ];
    for (const neighbor of neighbors) {
      if (!neighbor) continue;
      if (neighbor.kind === "photo") warmImageDecode(safeConvert(neighbor.path));
      void fetchAssetThumb(neighbor.id, VIEWER_THUMB_SIZE, "high").then((r) => {
        if (r.kind === "url") warmImageDecode(r.url);
      });
      if (neighbor.kind === "raw") {
        // RAW 相邻预热内嵌全幅直出档（毫秒级 IO，取最大内嵌 JPEG）
        void fetchAssetThumb(neighbor.id, VIEWER_RAW_EMBED_SIZE, "high").then((r) => {
          if (r.kind === "url") warmImageDecode(r.url);
        });
      }
    }
  }, [index, group.assets]);

  function sourceKind(src: string): string {
    if (src === originalUrl) return "original";
    if (src === rawEmbed.url && asset.kind === "raw") return "raw-embed";
    if (src === rawFull.url && asset.kind === "raw") return "raw-full";
    return src === mid.url ? "mid" : "thumb";
  }
  return { stage, setStage, mainSrc, mainFailed, mainMissing, showMissingPlaceholder, sourceKind,
    imageLayers, setImageLayers, slowLoading };
}
