# 联机拍摄相机 SDK 获取与构建（仓库外模式）

四家相机厂商的联机 SDK 均为**许可禁止再分发**（头文件/import 库不可入仓库、
不可随源码分发），本项目采用「仓库外 SDK + 一键构建脚本 + 缺席降级」模式：
仓库只含自研桥代码，SDK 由使用者自行申请下载后交给脚本消费。

## 各家获取

| 厂商 | SDK | 入口 | 备注 |
|---|---|---|---|
| Sony | Camera Remote SDK（RemoteCli 目录） | https://support.d-imaging.sony.co.jp/app/sdk/en/index.html （表单申请，免费，个人可申） | 对齐 ≥1.13（2026-09 逆向观察：像素蛋糕量产用 1.13.00，A7R V 实机 libusbK 形态即其 USB 传输层） |
| Canon | EDSDK | https://developercommunity.usa.canon.com/ （同意协议后下载） | 运行时 DLL 多数版本允许随应用分发（发布前核对当期协议） |
| Nikon | Nikon SDK | 尼康开发者计划表单申请 | 未接入；WPD 通道 0x90C0 厂商码为优先验证路线 |
| Fujifilm | FTLPTP | 富士开发者计划 | 未接入；其官方库为 WPD 传输层（佐证 WPD 后端对富士可行） |

## 构建（拿到 SDK 后）

```text
scripts\build-sony-bridge.cmd <RemoteCli 目录路径>
```

产物落 `%LOCALAPPDATA%\PhotoHub\sony-sdk\bridge-build\Release\photo-hub-sony.exe`
（含随构建拷贝的 crsdk 运行时 DLL）。后端每次连接时按路径探测，**应用无需重启**。
SDK 也可解压到仓库 `vendor/sony-sdk/`（已 gitignore）或任意位置，路径传给脚本即可。

## CI 与缺席语义

- CI 不获取任何厂商 SDK：桥不参与 CI 构建；主程序全部测试在无 SDK 环境绿。
- 运行时缺席降级：`helper_path()` 探测不到桥 → 相机清单不出现该厂 →
  UI 空态提示（联拍弹窗已含 Sony SDK 前置提示）。

## 发布（安装包）纪律

- 允许随应用分发的运行时 DLL（按当期协议核对后）由构建脚本拷至产物旁，
  打包脚本原样收取。
- 头文件 / import 库（.h/.lib）永不进安装包与仓库。
- 不使用从其他产品安装目录提取的 SDK 文件构建或分发（授权链条不覆盖）。
