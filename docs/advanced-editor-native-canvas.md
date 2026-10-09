# Native advanced-editor canvas prototype

Windows adjustment/view/output modes use a native child window beneath a
transparent WebView. React/Konva remain the controls, pointer-input and overlay
layer. PhotoCraft's compositor writes its chunks to a GPU display texture;
the upstream ICC display LUT maps to sRGB, then a swapchain presents it. There
is no pixel readback, JPEG encoding or image payload over IPC in this path.

The existing 1600-pixel proxy/session, recipes, undo history, native background
preparation and full-original export are reused. Native frame requests carry
only the recipe and clipped viewport/UV coordinates. A one-slot latest-request
queue and revisions prevent backlog. Panning/resizing reuses the composite.

Exposure/temperature/tint are channel-separable for the exposed Camera Raw
controls. The upstream `camera_raw::develop` function samples 256 display levels
into per-channel PhotoCraft Curves adjustment layers. This is preview-only;
export retains the original native algorithms. A parity test compares the
sampled preview with the upstream full filter.

Prototype limits: Windows only; uncropped, unrotated adjustment/view/output
mode. Crop/text/brush/picker editing, other platforms, unsupported color modes
and GPU failures use the existing preview. GPU resources close with the session
or editor window. Hardware support and embedded-window appearance require
platform-specific validation before extending this to macOS.

Native windows use Tauri's `unstable` WindowBuilder API, pinned by Cargo.lock.
The new UI checks `editor-native-frame` success before hiding the ordinary
image and pauses ordinary frame rendering only after native presentation works.

Validation: `cargo run --manifest-path src-tauri/Cargo.toml --example
native_canvas_probe` opens a hidden, disposable native test window, changes no
library/settings/photos, checks actual hardware presentation and prints submit
timings. Examples embed a Common Controls v6 manifest, as the main app does.
The 640×400 cached-composite result on this machine was about 1.17 ms per
submit (RTX 5070 Ti); this is not full-editor latency or an FPS guarantee.

Frontend diagnostics are in `performance.getEntriesByName("editor.native.frame")`,
including CPU submit time, uploaded bytes and `readbackBytes: 0`.
