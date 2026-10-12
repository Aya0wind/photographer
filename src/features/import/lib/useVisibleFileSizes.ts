import { useEffect, useRef, useState } from "react";
import { deviceFileSizes } from "@/ipc/api";
import type { SourceFile } from "@/stores/importStore";

// Sizes never participate in discovery. Only virtualized rows request them.
export function useVisibleFileSizes(deviceId: string | undefined, files: SourceFile[]) {
  const [sizes, setSizes] = useState<Record<string, number>>({});
  const [supported, setSupported] = useState<boolean | null>(null);
  const [revision, setRevision] = useState(0);
  const inFlight = useRef(false);
  const alive = useRef(true);
  const activeDevice = useRef(deviceId);
  activeDevice.current = deviceId;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const ids = files.filter(file => file.size === 0 && file.objectId && sizes[file.objectId] === undefined)
    .map(file => file.objectId!).slice(0, 64);
  const key = JSON.stringify(ids);
  useEffect(() => { setSizes({}); setSupported(null); }, [deviceId]);
  useEffect(() => {
    if (!deviceId || supported === false || ids.length === 0) return;
    // Scroll changes cancel queued work before it touches the camera.
    const timer = window.setTimeout(() => {
      if (inFlight.current) return;
      inFlight.current = true;
      void deviceFileSizes(deviceId, ids).then(values => {
        if (!alive.current || activeDevice.current !== deviceId) return;
        if (values === null) { setSupported(false); return; }
        setSupported(true);
        setSizes(previous => ({ ...previous, ...Object.fromEntries(ids.map(id => [id, 0])), ...Object.fromEntries(values) }));
      }).catch(() => {
        if (alive.current && activeDevice.current === deviceId) setSupported(false);
      }).finally(() => {
        inFlight.current = false;
        if (alive.current) setRevision(value => value + 1);
      });
    }, 150);
    return () => { window.clearTimeout(timer); };
  }, [deviceId, key, supported, revision]);
  return { sizes, supported };
}
