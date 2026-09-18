//! 热插拔检测（WM_DEVICECHANGE）：M1 T3 由设备 lane 实现。
//! 要求：message-only 窗口 + RegisterDeviceNotification（卷接口 + WPD 接口），
//! 独立线程泵消息 → EventBus.publish(DeviceArrived/Removed)。
