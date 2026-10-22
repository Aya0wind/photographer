# 基础编辑器：品牌边框

基础编辑器的“品牌边框”提供 26 个预制模板、8 种信息排版与 7 种画布比例。模板包含手机影像底栏、相纸、画廊、杂志、胶片、横版明信片、全景与毛玻璃。横竖照片都完整放入画布；改变画布比例不裁剪或拉伸前景照片。

## 自定义与保存

- 纯色或照片毛玻璃背景；可调整虚化、蒙层颜色与浓度。
- 可独立调整 Logo 大小（50%–200%），设置留白、底栏高度、文字大小与字体，选择单线、双线、四角装饰。
- 官方 Logo、相机图标、自定义图片或隐藏标识。自定义 PNG/JPEG/WebP 上限 512 KB、4096 × 4096 像素，可用透明 Logo、签名或相机缩略图。
- 相机、镜头、焦距、光圈、快门、ISO、拍摄时间来自照片信息；缺失不编造。各项可独立显示、隐藏或修改。
- “我的方案”在本机保存最多 40 项。方案包含样式与 Logo，不复制源照片的相机、参数、时间或文案；使用方案时保留当前照片信息。
- 配方保存当前照片的边框；撤销、重做、重置与原有编辑流程一致。

## 预览与输出

边框工具及导出设置中显示完整成片，调色、裁剪、标注时仍操作照片本身。边框在 JPEG、PNG、TIFF 输出编码前合成，输出长边是包含边框的最终尺寸。原文件不会被改写。毛玻璃仅模糊背景副本，前景照片与 16 位像素保持原来的精度。品牌标识作为矢量图嵌入，不用系统字体仿制；字体仅用于参数和文案。内置 Inter、Manrope、Lora，默认使用 Inter；预览和原生导出均加载同一份字体，离线可用。仍可选择系统字体，未安装的系统字体由系统回退。Logo 或字号变大时自动增大底栏，避免信息重叠。

## 设计参考

以下参考用于确定交互与版式方向，未复制产品界面或模板素材：

- [小米：Leica 水印和拍摄时间](https://www.mi.com/uk/support/faq/details/KA-508091/)
- [OPPO：签名、经典相机、胶片及参数边框](https://community.oppo.com/thread/1756872330790830087)
- [vivo：色彩自适应边框](https://bbs.vivo.com/in/thread/139162)
- [Snapseed：边框样式、纹理与比例](https://support.google.com/snapseed/answer/3114255?hl=en)
- [Adobe Express：边框颜色、粗细和样式](https://helpx.adobe.com/express/web/image-creation-and-editing/edit-images/add-a-border-to-an-image.html)
- [Canva：背景虚化与照片编辑](https://www.canva.com/features/blur-backgrounds/)

## 官方 Logo 来源

获取日期：2026-10-22。文件位于 `src/features/editor/assets/brands/`，保留官方 SVG 的矢量路径与颜色，没有用字体重绘。官网仅提供白色标识时，使用深色衬底保证辨识度。Logo 权利归各品牌所有；此处仅用于设备识别，不表示品牌合作。

| 品牌 | 文件 | 来源 |
| --- | --- | --- |
| SONY | `sony.svg` | [官网资源](https://www.sony.co.jp/en/) |
| Canon | `canon.svg` | [官网资源](https://global.canon/01cmn/img/common/logo-sp.svg) |
| Nikon | `nikon.svg` | [官网资源](https://www.nikon.com/etc.clientlibs/nikoncore/clientlibs/clientlib-site/resources/img/logo.svg) |
| FUJIFILM | `fujifilm.svg` | [官网资源](https://global.fujifilm.com/themes/custom/fujifilm_com_g2/common/img/fujifilm_corporate_logo.svg) |
| Leica | `leica.svg` | [官网资源](https://leica-camera.com/themes/custom/leica_redesign_theme/logo.svg) |
| Hasselblad | `hasselblad.svg` | [官网资源](https://www.hasselblad.com/assets/icons/logo-white.svg) |
| DJI | `dji.svg` | [官网资源](https://www-cdn.djiits.com/assets/styles/build/www.header-v4.min-b9cead8c66063dd15abdec6a75eaca25.css) |
| OM SYSTEM | `omsystem.svg` | [官网资源](https://nala.explore.omsystem.com/static/version1792396534/frontend/OMDigital/Hyva/en_US/images/logo.svg) |
| RICOH | `ricoh.svg` | [官网资源](https://www.ricoh.com/-/Media/Ricoh/Common/cmn_g_header_footer/img/logo/logo.svg) |
| vivo | `vivo.svg` | [官网资源](https://www.vivo.com/en) |
| Xiaomi | `xiaomi.svg` | [官网资源](https://www.mi.com/global/) |
| HUAWEI | `huawei.svg` | [官网资源](https://consumer.huawei.com/.resources/huawei-cbg-site-lm-basic/webresources/mkt/etc/designs/huawei-cbg-site/clientlib-campaign-v4/common-v4/images/logo.svg) |
| ZEISS | `zeiss.svg` | [官网资源](https://www.zeiss.com/etc.clientlibs/zeiss-aem-app/jupiter/clientlibs/jupiter-components.app/resources/assets/icons/symbols/zeiss-logo.svg) |

## 内置字体

来自 [Google Fonts 官方字体仓库](https://github.com/google/fonts)，原始文件未改名内部字体名称，随应用分发 SIL Open Font License：

- [Inter](https://github.com/google/fonts/tree/main/ofl/inter)：现代无衬线，默认参数字体。
- [Manrope](https://github.com/google/fonts/tree/main/ofl/manrope)：几何无衬线。
- [Lora](https://github.com/google/fonts/tree/main/ofl/lora)：人文衬线，画廊和相纸模板默认字体。

字体文件和各自许可证位于 `src/features/editor/assets/fonts/`；分发产物包含 `public/licenses/fonts/` 中的许可证副本。

为统一浏览器与原生 SVG 的字重，Inter 和 Manrope 从原始可变字体固定生成 400/700 两个实例，保留家族名称与轮廓；Lora 使用原始字体的 400 字重。原始可变字体一并保留以便复现。
