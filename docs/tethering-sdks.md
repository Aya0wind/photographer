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

---

## libgphoto2 后端（进程内，gphoto_backend.rs）

Sony 通路之外的第二条通用后端：运行时 `libloading` 动态加载
`libgphoto2-6.dll` + `libgphoto2_port-12.dll`（无 import 库 / 无 pkg-config /
零子进程——gphoto2-rs crate 卡在 pkg-config + MSVC import 库，vcpkg 无 port，
自持薄 FFI 是定案）。注册表 id `gphoto`，相机 id 形如 `gphoto:usb:001,011`。

### DLL 部署集（MSYS2 ucrt64，objdump 传递闭包实测）

13 个：libgphoto2-6 / libgphoto2_port-12 / libusb-1.0 / libexif-12 /
libintl-8 / libltdl-7 / libwinpthread-1 / libsystre-0 / libiconv-2 /
libtre-5 / libjpeg-8 / libxml2-16 / zlib1。部署目录
`%LOCALAPPDATA%\PhotoHub\gphoto\`（或 exe 旁 `gphoto/`，或
`PHOTO_HUB_GPHOTO_DLL` 指定绝对路径）。全部以
`LOAD_WITH_ALTERED_SEARCH_PATH` 加载并 mem::forget 驻留——camlib
（ptp2.dll）与 iolib（usb1.dll）运行期按模块名解析依赖，预加载句柄一 drop
就会被 FreeLibrary 撤走。

### 运行期三个坑（真机 A7R V 逐一实证，2026-09-29）

1. **iolibs/camlibs 目录**：libgphoto2 按编译期前缀（C:\msys64 树）找驱动，
   官方重定位钩子是 `IOLIBS` / `CAMLIBS` 环境变量。且 Win32
   `SetEnvironmentVariable`（= Rust `env::set_var`）对 ucrt `getenv` 的
   启动快照不可见——必须经共享 ucrtbase 的 `_putenv` 写入 CRT 表。
2. **符号归属**：`gp_port_info_list_*` / `gp_context_*` 由 port 库导出而非
   主库，符号表要在两库间逐个解析。
3. **RADIO/MENU 的 set_value 传 `char*` 本体**（与 get 的 `&char*` 不对称）；
   传 `&ptr` 相机静默忽略。另 Sony 属性写入后立即回读是旧值（异步生效），
   set 后需 ~180ms 再刷新。

### 功能面（真机全通：枚举 / 参数 / 取景帧 33KB / 拍摄+下载 64MB ARW / 入册）

- **全参数面板**：`gp_camera_list_config`（505 项）过滤厂商裸码后动态输出
  （kind = choice / toggle / range / action；A7R V 约 35 项：测光模式、
  P/A/S/M、白平衡、色温、画质、驱动模式 139 档、对焦区域、防抖、快门类型、
  电子手动对焦 -7..+7 等）。`set_single_config` 在 ptp2 上返回 -2，写路径
  走全树 get→find→set→set_config。
- **点击对焦**：`tethering_focus_at(x,y)`（归一化坐标）→ 写
  `spotfocusarea`（PTP_DPC_SONY_AFAreaPosition，live view 640×480 系，
  格式 "x,y"）+ 触发 autofocus toggle。相机需处于 AF 模式且对焦区域支持
  定点，否则相机侧忽略（前端静默降级）。
- **取景帧率** 15/30/60 档（前端轮询间隔，localStorage 持久）。

### 分发结论（MSVC vs MinGW）

**用 MSYS2 的 MinGW (ucrt64) 现成产物，不要为 MSVC 自编译 libgphoto2**：
上游无官方 MSVC 支持，维护成本全在自己；ucrt64 构建链 ucrtbase（与 MSVC
主程序同一 CRT，堆一致），C ABI 完全兼容，闭包 13 个 DLL 可控。camlibs
目录用 IOLIBS/CAMLIBS 环境变量重定位（安装包把 iolibs/camlibs 子目录放
gphoto/ 旁即可，locate_driver_dir 已支持随包布局）。LGPL-2.1 动态链接 +
随包附源码链接即合规。libloading 接缝意味着将来换自编译/换工具链 DLL 不
需要动应用代码（同一组 C 符号）。
