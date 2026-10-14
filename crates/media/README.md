# photographer-media：固定 RawLab core 的 Rust 接口

无 Tauri、窗口、数据库或脚本运行时依赖。**直接复用 C++ core，不重复实现数学/显影/胶片算法**。`rawlab` 模块负责版本、参数、编码描述、错误、借用和原生资源释放；构建/来源/合同详见 [native/rawlab](../../native/rawlab/README.md)。

`Backend::develop` 得到浮点线性 sRGB/D65；`Backend::render` 调用 core CPU 或平台 photo GPU，得到显示 sRGB。GPU 模式与实际 gpu_used 分开；Auto 可回退、Force 不伪装 CPU 成功。`Backend::color_session` 创建可复用源/设备会话，`ColorSession::render` 支持线性曝光、相对 RGB WB 和胶片强度；会话 `!Send/!Sync`，必须在同一工作线程管理，输出可在会话释放后继续借用。`Backend::lut_info` 保留原始 CUBE 的上游色彩合同。桥 ABI 2 与 Rust 封装一起更新，加载旧 ABI 明确失败。主编辑器通过 `src-tauri/src/edit/rawlab.rs` 装配：独立色彩配方、持久工作线程、float 代理缓存、胶片资源与完整 RAW 导出；旧配方沿用原链。主应用集成代码已接通，跨平台构建与实机交互性能仍需 CI/样片验收。

跨模块/FFI 集成测试放在本 crate 的 tests；职责内单测应相邻放入 src。tests/rawlab_bridge.rs 覆盖失败/取消、资源绕过、编码标记、浮点范围、合成 Log LUT 的强度/CPU-GPU Auto 对照；真实 RAW 生命周期用例显式 ignored。未提供桥时跳过 ABI 用例，不表示通过。三平台 CI 会提供刚构建的桥。

```sh
cargo test --manifest-path crates/media/Cargo.toml --locked
PHOTOGRAPHER_RAW_BRIDGE=/absolute/path/libphotographer_raw.so \
PHOTOGRAPHER_RAW_FIXTURE=/photos/input.DNG \
cargo test --manifest-path crates/media/Cargo.toml --locked --test rawlab_bridge -- --ignored
```

M0 暂用仓内 path 依赖，保留 src-tauri/Cargo.lock 和现有桌面入口。轻量 CI 的独立锁文件在 M1 根 workspace/应用布局迁移时合并，不维护两套引擎版本。
