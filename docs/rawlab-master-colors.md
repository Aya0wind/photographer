# 内置 RawLab 大师色彩

编辑器的胶片面板提供 14 款「内置大师色彩」，选择后自动启用 RawLab 色彩管线，无需导入 CUBE。强度调节、实时预览、保存和导出使用同一份原始 LUT。

## 资源位置

- 上游 CUBE：`native/rawlab/upstream/RawLabMac/Resources/MasterRecipes/`。
- 上游名称、风格说明和配方参数：`native/rawlab/upstream/Shared/master-recipes.json`。
- 打包脚本：`scripts/build-rawlab.py` 的 `stage_builtin_films`。
- 打包后的目录：`src-tauri/resources/rawlab/films/`，安装后位于应用资源目录的 `rawlab/films/`。
- 打包后的 `catalog.json` 按原始 CUBE 的 SHA-256 标识资源；对应文件为 `<SHA-256>.cube`。`master-recipes.json` 保留上游名称与原文件名的对应关系。
- 用户自行导入的胶片仍位于应用配置目录的 `films/`，独立于内置资源。

包含滨田英明、保井崇志、酒井贵弘、withluke、Daniel Casson、Billy Dinh、rkrkrk、Samalive、Iwona、CineStill 800T、Wilson Lee、Luiz Clas、Ivan Ivanchenko 和 Jungraphy。

此处内置的是这些风格的色彩 LUT；选择时保留 Photographer 当前的曝光、白平衡、影调和 HSL 调整，不自动覆盖为上游完整配方的调整参数。原始 CUBE 的 sRGB/BT.709 声明与内容保持不变。

CI 产物及 Release 附件仅上传安装包，不附带 `rawlab-source-*.tar.gz`。应用内保留原有许可证和源码获取说明。
