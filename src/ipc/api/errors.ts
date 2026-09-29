

/** invoke 不可用类错误（非 Tauri 环境/命令未注册）：error=null，调用方显示通用文案 */
export const INVOKE_UNAVAILABLE_PATTERN = /__TAURI_INTERNALS__|invoke is not available|command [^\s]+ not found/i;
