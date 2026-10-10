import { useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { photoLibraryCreate, type PhotoLibrary } from "@/ipc/api";

/**
 * 新建照片库对话框（M3 存储页，2026-10-10 单一入口定案）：
 * 对话框内模式二选一（不再由外部两个按钮分别进入）：
 * - 「新建」：登记空文件夹，之后 copy/move 导入落盘目标 = 该库 root
 *   （纯时间布局由后端导入引擎负责）。
 * - 「从已有文件夹建立」（reference）：只登记不搬文件；登记即触发后端递归
 *   扫描批量登记，进度/取消/完成通知由存储页卡片扫描状态行呈现（M4f 闭环）。
 * 业务错误（路径非法/与其他库或数据库目录重叠等）透传后端 Err 文案内联展示；
 * invoke 不可用（error=null）提示后端未连接。
 */

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 从绝对路径取末段作为缺省库名（Windows/Unix 分隔符都认；取不到回退全路径） */
function baseName(path: string): string {
  const parts = path.split(/[\\/]+/).filter((s) => s.length > 0);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

export default function CreateLibraryDialog({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  /** 登记成功回调（mode = 实际使用的模式；存储页据此区分 toast 文案） */
  onCreated: (library: PhotoLibrary, mode: "new" | "reference") => void;
}) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<"new" | "reference">("new");
  const [name, setName] = useState("");
  const [rootPath, setRootPath] = useState("");
  /** 名称未被手改过时，选完文件夹自动带出末段作缺省名 */
  const [nameTouched, setNameTouched] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready = name.trim().length > 0 && rootPath.trim().length > 0 && !creating;

  function patchMode(next: "new" | "reference"): void {
    if (next === mode) return;
    setMode(next);
    // 切模式清路径；名称未手改时回空（选完文件夹自动带出）
    setRootPath("");
    if (!nameTouched) setName("");
  }

  async function pickDirectory(): Promise<void> {
    try {
      const dir = await openDialog({
        directory: true,
        defaultPath: rootPath.trim() || undefined,
      });
      if (typeof dir === "string" && dir.length > 0) {
        setRootPath(dir);
        if (!nameTouched && name.trim().length === 0) setName(baseName(dir));
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function create(): Promise<void> {
    if (!ready) return;
    setCreating(true);
    setError(null);
    const result = await photoLibraryCreate(
      name.trim(),
      rootPath.trim(),
      mode === "reference",
    );
    setCreating(false);
    if (result.ok) {
      onCreated(result.library, mode);
      return;
    }
    // null=invoke 不可用（后端未连接）；字符串=后端业务 Err 文案透传
    setError(
      result.error === null ? t("storage.createDialog.unavailable") : result.error,
    );
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      data-testid="storage-create-dialog"
      data-mode={mode}
    >
      <div
        className="w-[460px] rounded-xl border border-edge bg-surface p-5"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-sm font-semibold text-text-primary">
          {t("storage.createDialog.title")}
        </h2>

        {/* 模式二选一：新建空文件夹 / 从已有文件夹建立（reference） */}
        <div className="mt-2 grid grid-cols-2 gap-2" data-testid="storage-create-modes">
          {(["new", "reference"] as const).map((key) => (
            <button
              key={key}
              type="button"
              onClick={() => patchMode(key)}
              aria-pressed={mode === key}
              className={`rounded-lg border p-2.5 text-left transition-colors ${
                mode === key
                  ? "border-accent bg-accent/10"
                  : "border-edge hover:border-text-muted"
              }`}
              data-testid={`storage-create-mode-${key}`}
              data-selected={mode === key}
            >
              <span
                className={`text-xs font-medium ${mode === key ? "text-accent" : "text-text-primary"}`}
              >
                {t(`storage.createDialog.mode.${key}`)}
              </span>
              <p className="mt-0.5 text-[11px] leading-relaxed text-text-secondary">
                {t(`storage.createDialog.mode.${key}Desc`)}
              </p>
            </button>
          ))}
        </div>

        <p className="mt-2 text-xs leading-relaxed text-text-secondary">
          {t(mode === "new" ? "storage.createDialog.new.hint" : "storage.createDialog.reference.hint")}
        </p>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("storage.createDialog.name")}
          <input
            type="text"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setNameTouched(true);
            }}
            className={FIELD_CLASS}
            autoFocus
            data-testid="storage-create-name"
          />
        </label>

        <label className="mt-3 flex flex-col gap-1 text-xs text-text-secondary">
          {t("storage.createDialog.root")}
          <div className="flex gap-2">
            <input
              type="text"
              value={rootPath}
              onChange={(e) => setRootPath(e.target.value)}
              className={FIELD_CLASS}
              data-testid="storage-create-root"
            />
            <button
              type="button"
              onClick={() => void pickDirectory()}
              className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
              data-testid="storage-create-browse"
            >
              {t("storage.createDialog.browse")}
            </button>
          </div>
        </label>

        {error !== null && (
          <p className="mt-2 text-[11px] text-red-400" role="alert" data-testid="storage-create-error">
            {error}
          </p>
        )}

        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
            data-testid="storage-create-cancel"
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={() => void create()}
            className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="storage-create-confirm"
          >
            {creating ? t("storage.createDialog.creating") : t("storage.createDialog.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
