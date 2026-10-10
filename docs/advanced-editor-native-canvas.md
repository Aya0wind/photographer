# Native advanced-editor canvas prototype

Windows adjustment/view/output modes use a pointer-transparent foreground
native child window. React/Konva remain the controls and pointer-input layer.
The WebView stays opaque; no CSS transparency hole is used. PhotoCraft's compositor writes its chunks to a GPU display texture;
the upstream ICC display LUT maps to sRGB, then a swapchain presents it. There
is no pixel readback, JPEG encoding or image payload over IPC in this path.

The existing 1600-pixel proxy/session, recipes, undo history, persistent proxy caching
and full-original export are reused. Native frame requests carry
only the recipe, fixed viewport bounds and image-placement coordinates. A one-slot latest-request
queue and revisions prevent backlog. Zoom/pan change GPU image coordinates without moving/resizing the visible
native window. Modal dialogs suspend the native layer; its hide epoch rejects
late frames. The initial window remains transparent until presentation has
completed, avoiding an empty window/border flash.

Exposure/temperature/tint are channel-separable for the exposed Camera Raw
controls. The upstream `camera_raw::develop` function samples 256 display levels
into per-channel PhotoCraft Curves adjustment layers. This is preview-only;
export retains the original native algorithms. A parity test compares the
sampled preview with the upstream full filter.

Prototype limits: Windows only; adjustment/view/output/crop support GPU geometry and post-geometry annotations. Text/brush editing, other platforms, unsupported color modes
and GPU failures use the existing preview. GPU resources close with the session
or editor window. Hardware support and embedded-window appearance require
platform-specific validation before extending this to macOS.

Native windows use Tauri's `unstable` WindowBuilder API, pinned by Cargo.lock.
The new UI checks `editor-native-frame` success before activating the native
viewport and pauses ordinary frame rendering only after native presentation works.

Validation: `cargo run --manifest-path src-tauri/Cargo.toml --example
native_canvas_probe` opens a hidden, disposable native test window, changes no
library/settings/photos, checks actual hardware presentation and prints submit
timings. Examples embed a Common Controls v6 manifest, as the main app does.
The 640×400 cached-composite result on this machine was about 1.17 ms per
submit (RTX 5070 Ti); this is not full-editor latency or an FPS guarantee.

Frontend diagnostics are in `performance.getEntriesByName("editor.native.frame")`,
including CPU submit time, uploaded bytes and `readbackBytes: 0`.

### Modal visibility and first adjustment

Native visibility uses the actual reparented HWND: alpha is cleared and SWP_HIDEWINDOW runs on the UI thread before the hide command completes. Frame and hide requests share a monotonically increasing revision; both reveal steps reject hide epochs (recipe-only revisions may present while a newer edit is pending). A stopped worker can still be hidden while awaiting destruction. The hardware probe checks that WS_VISIBLE is cleared after reparenting.

Opening the canvas prewarms camera curves, basic adjustments, HSL, vibrance, levels and luminosity curves while hidden, using the same stable layer IDs as user edits. Source upgrades retain the warmed renderer and adjustment layer identities. Warmup recipes are temporary and never saved; the first visible frame always uses the current user recipe. A regression test checks that returning to a neutral recipe preserves the original pixels.

An unedited recipe reuses the already decoded source preview, skipping the duplicate initial compositor/JPEG request that would otherwise compete with native canvas startup. Resetting to neutral also reuses that source. Edited recipes still use the normal latest-only renderer when native presentation is disabled.

### Continuous dragging and large RAW startup

The ordinary renderer now throttles to one request per 16 ms scheduling window instead of resetting a 40 ms debounce on every input. One render remains in flight; completed frames are displayed during a gesture and the latest pending parameters run next. Returning to an unedited source still rejects older adjusted results.

The native swapchain requests one queued frame and uses Mailbox where supported. ICC profiles are parsed only when the source profile changes. Revealing a hidden surface is blocked by modal hide epochs, not by newer recipe revisions, so continuous input cannot starve the initial presentation.

RAW embedded JPEGs use scaled TurboJPEG decode for the fixed 1600 px proxy. Background native RAW preparation uses upstream MHC demosaicing and resizes its contiguous 16-bit RGB before building a tiled document; source dimensions remain the original dimensions. Full-size export retains upstream default AHD processing. This removes full-resolution document construction/resampling from interactive preparation, but decoding the sensor for the native upgrade still has a cost; the embedded proxy remains editable meanwhile.

### Proxy-only interactive editing (2026-10-10)

Interactive opening no longer automatically invokes edit_preview_prepare or upgrades the displayed proxy to a developed RAW. Fixed 1600 px sRGB JPEG proxies are stored in the active database's editor-proxies/ directory. Keys include format version, source path, actual modification time, size and proxy dimensions. A valid proxy is reused, corrupted entries regenerate, and changed originals use a new key. Writes are atomic and affect only derived cache files; GPU frames use identical decoded pixels on first and subsequent opens. The explicit preparation endpoint remains available internally, and original-file export remains separate.

The hardware regression probe now starts at 1x1, matching production warmup: surface configuration must run on the first draw even when dimensions match the default. It also changes curves repeatedly on a 1600 px U16 document and asserts zero source texture reuploads. The earlier larger-window probe did not cover this initialization failure.

### Floating controls and direct panning

The zoom HUD is again centered over the bottom of the photo and hides three seconds after zoom activity. It does not occupy layout space or wake on pan/hover. Native frame metadata contains bounded overlay rectangles; Windows subtracts their rounded regions from the foreground HWND so the opaque DOM controls remain visible and clickable. Clearing the HUD restores the full region. The hardware probe asserts that a point inside the HUD is excluded and a point inside the photo remains included.

EditorCanvas allows panning in adjustment/filter/output/metadata modes whenever the image overflows; sampling, brush, text and crop gestures take precedence. The separate view navigation entry has been removed, while internal view mode remains available for read-only/compare states.

### GPU geometry and post-geometry annotations

PhotoCraft still executes full geometry for ordinary previews and export. The native canvas keeps the photo's color-adjusted texture resident and uses an inverse affine sampler for quarter-turns, flips, arbitrary rotations and exact integer crop bounds. Canvas dimensions come from PhotoCraft mode_cmds::rotated_size. Crop guides, grid and handles are native display graphics; the DOM retains hit testing. Annotation documents are built at the post-geometry canvas dimensions using the original PhotoCraft annotation commands, then composited as a second resident texture before the display ICC conversion. Color changes do not rebuild annotations, and pure geometry changes do not recompose or upload photo pixels.
