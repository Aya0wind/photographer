//! 卷设备源（读卡器/U 盘）：M1 T3 由设备 lane 实现。
//! 要求：list 递归遍历（忽略 System Volume Information / $RECYCLE.BIN）、
//! FILE_FLAG_SEQUENTIAL_SCAN 读取、2-4 并发流由引擎侧控制。
