import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { assetTrashMove, type AssetDto } from "@/ipc/api";
import AddToAlbumDialog from "@/features/albums/components/AddToAlbumDialog";

import { AssetContextMenu } from "./ContextMenu";

/**
 * 照片右键操作与后续弹窗：批量标记、全选/反选、加入相册和回收站。
 * 始终挂载，让单张照片的右键菜单关闭后，操作弹窗也能继续显示。
 */

interface Props {
  contextMenu?: { x: number; y: number; assets: AssetDto[] } | null;
  onCloseContextMenu?: () => void;
  /** 页面当前展示的全部资产（数据窗口；全选/反选的口径） */
  assets: AssetDto[];
  selectedIds: number[];
  /** 全选/反选/取消全选落点（替换式） */
  onSelectIds: (ids: number[]) => void;
  /** 退出多选模式 */
  onDone: () => void;
  /** 选中资产本地乐观补丁（星级/色标/拒绝的显示同步） */
  onPatched?: (ids: number[], patch: Partial<AssetDto>) => void;
  /** 移入回收站后的本地剔除（页面过滤列表） */
  onRemoved?: (ids: number[]) => void;
}

export default function AssetActionsHost({
  assets,
  selectedIds,
  onSelectIds,
  onDone,
  onPatched,
  onRemoved,
  contextMenu,
  onCloseContextMenu,
}: Props) {
  const { t } = useTranslation();
  const windowIds = useMemo(() => assets.map((a) => a.id), [assets]);
  const [addToAlbumTargets, setAddToAlbumTargets] = useState<AssetDto[] | null>(null);
  /** 「移入回收站」确认目标（null=弹窗关闭） */
  const [trashConfirm, setTrashConfirm] = useState<number[] | null>(null);

  async function confirmTrash(): Promise<void> {
    if (trashConfirm === null) return;
    const ids = trashConfirm;
    setTrashConfirm(null);
    await assetTrashMove(ids); // 后端软删；本地剔除交给回调（失败靠重进页面重拉兜底）
    onRemoved?.(ids);
    onDone();
  }

  return (
    <>
      {contextMenu && <AssetContextMenu
        at={{ x: contextMenu.x, y: contextMenu.y }}
        assets={contextMenu.assets}
        onClose={() => onCloseContextMenu?.()}
        onFavoritesChanged={(items, favorite) =>
          onPatched?.(items.map((a) => a.id), { rating: favorite ? 5 : 0 })
        }
        onFlagged={(items, flagged) => onPatched?.(items.map((asset) => asset.id), { flagged })}
        onAddToAlbum={(targets) => setAddToAlbumTargets(targets)}
        onColorLabeled={(items, label) => onPatched?.(items.map((a) => a.id), { colorLabel: label })}
        onRejected={(items, rejected) => onPatched?.(items.map((a) => a.id), { rejected })}
        onTrashRequest={(items) => setTrashConfirm(items.map((a) => a.id))}
        windowIds={windowIds}
        selectedIds={selectedIds}
        onSelectIds={onSelectIds}
      />}

      {/* 「加入相册」选择弹窗 */}
      {addToAlbumTargets !== null && (
        <AddToAlbumDialog assets={addToAlbumTargets} onClose={() => setAddToAlbumTargets(null)} />
      )}

      {/* 「移入回收站」确认一步 */}
      {trashConfirm !== null && (
        <div
          className="fixed inset-0 z-40 flex items-center justify-center bg-black/55 p-6"
          data-testid="trash-move-dialog"
          role="dialog"
          aria-modal="true"
          aria-label={t("trash.moveTitle")}
          onClick={(e) => {
            if (e.target === e.currentTarget) setTrashConfirm(null);
          }}
        >
          <div className="w-full max-w-sm rounded-xl border border-edge bg-surface p-4 shadow-2xl">
            <h2 className="text-sm font-semibold text-text-primary" data-testid="trash-move-title">
              {t("trash.moveTitle")}
            </h2>
            <p className="mt-2 text-xs leading-relaxed text-text-secondary">
              {t("trash.moveDesc", { count: trashConfirm.length })}
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setTrashConfirm(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="trash-move-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void confirmTrash()}
                className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-red-500/85"
                data-testid="trash-move-accept"
              >
                {t("trash.moveConfirm")}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
