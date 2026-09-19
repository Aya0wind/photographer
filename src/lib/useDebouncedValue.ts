import { useEffect, useState } from "react";

/**
 * 防抖值（搜索条件即时查询用，300ms）：value 变化后延迟 delayMs 才同步到返回值，
 * 期间再次变化则重新计时。value 为原始值（字符串/数字），对象请传序列化后的键。
 */
export function useDebouncedValue<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);

  return debounced;
}
