/** Lexical UI helpers, not filesystem identity or access-control checks. */
export function isWindowsPath(path: string): boolean {
  return /^[a-z]:/i.test(path) || path.startsWith("\\\\") || path.startsWith("//");
}

export function trimDirectoryEnd(path: string): string {
  if (!isWindowsPath(path)) return path.replace(/\/+$/, "") || (path.startsWith("/") ? "/" : "");
  const trimmed = path.replace(/[\\/]+$/, "");
  // C: means a drive-relative path; never turn C:\ into C:.
  return /^[a-z]:$/i.test(trimmed) ? `${trimmed}\\` : trimmed;
}

export function directoryComparisonKey(path: string): string {
  return isWindowsPath(path)
    ? trimDirectoryEnd(path.replace(/\//g, "\\")).toLowerCase()
    : trimDirectoryEnd(path);
}

export function directoryPreview(root: string, ...parts: string[]): string {
  const separator = isWindowsPath(root) ? "\\" : "/";
  const base = trimDirectoryEnd(isWindowsPath(root) ? root.replace(/\//g, "\\") : root);
  const suffix = parts.filter(Boolean).join(separator);
  const prefix = base && !base.endsWith(separator) ? base + separator : base;
  return `${prefix}${suffix}${separator}`;
}
