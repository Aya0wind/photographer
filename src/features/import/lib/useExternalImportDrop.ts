import {useEffect, useRef, useState} from "react";
import {isTauri} from "@tauri-apps/api/core";
import {getCurrentWebview} from "@tauri-apps/api/webview";

/** Native file drops retain absolute paths, including directories and RAW files. */
export function useExternalImportDrop(onDrop: (paths: string[]) => void) {
  const [dragging, setDragging] = useState(false);
  const callback = useRef(onDrop);
  callback.current = onDrop;
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let release: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent(({payload}) => {
      if (disposed) return;
      if (payload.type === "enter") setDragging(payload.paths.length > 0);
      if (payload.type === "leave" || payload.type === "drop") setDragging(false);
      if (payload.type === "drop" && payload.paths.length > 0) callback.current(payload.paths);
    }).then(off => { if (disposed) off(); else release = off; }).catch(() => {
      if (!disposed) setDragging(false);
    });
    return () => { disposed = true; release?.(); };
  }, []);
  return dragging;
}
