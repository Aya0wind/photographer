fn main() {
    tauri_build::build();
    // Tauri embeds the GUI manifest in the app binary, but not in diagnostic
    // examples. Native window probes need Common Controls v6 as well.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("examples/windows.manifest");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg-examples=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-examples=/MANIFESTINPUT:{}", manifest.display());
        // 测试也会链接原生文件对话框；缺少 Common Controls v6 会在
        // Windows 加载阶段报 TaskDialogIndirect 入口不存在，尚未进入测试。
        // rustc-link-arg-tests 不覆盖 lib 单元测试，通用链接参数只补控件依赖，
        // 不更改 Tauri 自己生成的应用权限声明。
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'");
        // 应用二进制的 resource.lib 已嵌入 Tauri 清单，不能再生成第二份。
        // 保留通用参数供 lib 测试使用，仅对应用二进制关闭链接器的清单生成。
        println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
    }
}
