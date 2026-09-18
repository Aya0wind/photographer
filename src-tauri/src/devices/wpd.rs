//! WPD/MTP 相机源（Windows Portable Devices COM）：M1 T4 由设备 lane 实现。
//! 要求：每线程 CoInitializeEx guard；属性批量枚举；IStream 流读取；
//! AccessDenied/Disconnected 错误语义（相机未切 PC 模式 / 传输中拔线）。
