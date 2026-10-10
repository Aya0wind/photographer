import { ipc } from "../index";

/** 在系统文件管理器中打开目录，或用默认程序打开照片文件（open_with_system）。
 *  不 catch：失败文案透传给调用方提示 */
export async function openWithSystem(path: string): Promise<void> {
  await ipc<void>("open_with_system", { path });
}

/** 复制文件到系统剪贴板（clipboard_copy_files：Explorer 可直接粘贴的文件对象；
 *  后端在途契约——命令未注册时 ipc() 抛错由调用方兜底提示）。不 catch：
 *  失败文案透传给调用方。 */
export async function clipboardCopyFiles(paths: string[]): Promise<void> {
  await ipc<void>("clipboard_copy_files", { paths });
}

/** 在资源管理器中批量定位选中文件（reveal_in_explorer：同目录多文件单窗
 *  多选；返回成功定位的文件数）。不 catch：失败文案透传给调用方。 */
export async function revealInExplorer(paths: string[]): Promise<number> {
  return ipc<number>("reveal_in_explorer", { paths });
}
