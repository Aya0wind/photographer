const paths = {
  undo: "M9 5 4 10l5 5M4 10h9a6 6 0 0 1 0 12",
  redo: "m15 5 5 5-5 5m5-5h-9a6 6 0 0 0 0 12",
  left: "M3 4h18v16H3zM9 4v16M5.5 8h1M5.5 12h1",
  right: "M3 4h18v16H3zM15 4v16m2.5-12h1m-1 4h1",
  compare: "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 0v18",
  split: "M3 5h18v14H3zM12 3v18",
  auto: "m12 3 2.4 6.6L21 12l-6.6 2.4L12 21l-2.4-6.6L3 12l6.6-2.4L12 3Z",
  save: "M5 3h13l3 3v15H3V3h2Zm2 0v6h10V3M7 21v-8h10v8",
  export: "M12 3v12m-4-4 4 4 4-4M4 16v5h16v-5",
} as const;

export default function EditorIcon({name}:{name:keyof typeof paths}) {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]}/></svg>;
}
