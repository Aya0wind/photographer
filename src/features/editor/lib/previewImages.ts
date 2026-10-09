/** 已解码帧供 Konva 直接复用，避免发布 URL 后再次创建/等待图片解码。 */
const images = new Map<string, HTMLImageElement>();

export function getPreviewImage(url: string): HTMLImageElement | undefined {
  return images.get(url);
}

export async function preparePreviewImage(url: string): Promise<void> {
  if (images.has(url)) return;
  const image = new Image();
  image.decoding = "async";
  try {
    if (typeof image.decode === "function") {
      image.src = url;
      await image.decode();
    } else {
      await new Promise<void>((resolve, reject) => {
        image.onload = () => resolve();
        image.onerror = () => reject(new Error("Preview image decode failed"));
        image.src = url;
      });
    }
    images.set(url, image);
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
}

export function releasePreviewImage(url: string): void {
  images.delete(url);
  URL.revokeObjectURL(url);
}
