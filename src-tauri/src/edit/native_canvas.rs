//! Experimental native canvas. Only parameters cross IPC; compositor output stays
//! on the GPU and is copied to a native swapchain. Other platforms retain JPEG.
use serde::Deserialize;
use tauri::{AppHandle, WebviewWindow};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub rect: [i32; 4],
    pub uv: [f32; 4],
    #[serde(default = "full_image")]
    pub image: [f32; 4],
    #[serde(default)]
    pub overlays: Vec<[i32;5]>,
    #[serde(default="identity_mapping")]
    pub mapping:[f32;8],
    #[serde(default)]
    pub crop_guide:Option<[f32;4]>,
    #[serde(default="default_scale")]
    pub scale:f32,
}
fn identity_mapping()->[f32;8] {[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0]}
fn default_scale()->f32 {1.0}
fn full_image() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}
impl View {
    fn validate(&self) -> Result<(), String> {
        if !self.scale.is_finite() || self.scale<=0.0 || self.scale>8.0 || self.mapping.iter().any(|v|!v.is_finite()) || self.crop_guide.is_some_and(|g|g.iter().any(|v|!v.is_finite()||*v<0.0||*v>1.001)||g[2]<=0.0||g[3]<=0.0) {return Err("原生画布变换参数无效".into());}
        if self.overlays.len()>8 || self.overlays.iter().any(|r|r.iter().any(|v|v.unsigned_abs()>32768)||r[2]<=0||r[3]<=0||r[4]<0) {return Err("原生画布浮动控件尺寸无效".into());}
        if self.image.iter().any(|v| !v.is_finite())
            || self.image[2] <= 0.0
            || self.image[3] <= 0.0
            || self.rect[2] <= 0
            || self.rect[3] <= 0
            || self.rect[2] > 16384
            || self.rect[3] > 16384
            || self
                .uv
                .iter()
                .any(|n| !n.is_finite() || *n < 0.0 || *n > 1.001)
        {
            return Err("原生画布尺寸无效".into());
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn edit_native_open(
    app: AppHandle,
    window: WebviewWindow,
    session_id: String,
) -> Result<bool, String> {
    if window.label() != "advanced-editor" {
        return Err("原生画布仅用于高级编辑窗口".into());
    }
    #[cfg(windows)]
    {
        return windows_canvas::open(app, window, session_id).await;
    }
    #[cfg(not(windows))]
    {
        let _ = (app, session_id);
        Ok(false)
    }
}

#[tauri::command]
pub async fn edit_native_frame(
    app: AppHandle,
    window: WebviewWindow,
    session_id: String,
    recipe: serde_json::Value,
    view: View,
    revision: u64,
    original: bool,
) -> Result<(), String> {
    view.validate()?;
    if window.label() != "advanced-editor" {
        return Err("原生画布窗口无效".into());
    }
    let recipe = super::recipe::parse_recipe(&recipe)?;
    #[cfg(windows)]
    {
        windows_canvas::frame(app, &session_id, recipe, view, revision, original).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, session_id, recipe, revision, original);
        Err("此平台尚未启用原生画布".into())
    }
}

#[tauri::command]
pub async fn edit_native_hide(session_id: String, revision: Option<u64>) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_canvas::hide(&session_id, revision).await
    }
    #[cfg(not(windows))]
    {
        let _ = (session_id, revision);
        Ok(())
    }
}
#[tauri::command]
pub fn edit_native_close(session_id: String) {
    #[cfg(windows)]
    windows_canvas::close(&session_id);
    #[cfg(not(windows))]
    let _ = session_id;
}

#[cfg(windows)]
mod windows_canvas {
    use super::super::{preview, preview_renderer::Renderer, recipe::EditRecipe};
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Condvar, Mutex, OnceLock,
    };
    use std::time::Instant;
    use tauri::{Emitter, Manager, Window};
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Frame {
        recipe: EditRecipe,
        view: View,
        revision: u64,
        original: bool,
        epoch: u64,
    }
    struct Worker {
        session: String,
        parent: String,
        window: Arc<Window>,
        latest: Mutex<Option<Frame>>,
        changed: Condvar,
        stopped: AtomicBool,
        visible: AtomicBool,
        shown: AtomicBool,
        revision: AtomicU64,
        hide_epoch: AtomicU64,
        rect: Mutex<Option<[i32; 4]>>,
        overlays:Mutex<Vec<[i32;5]>>,
    }
    impl Worker {
        fn stop(&self) {
            self.stopped.store(true, Ordering::Release);
            self.visible.store(false, Ordering::Release);
            self.shown.store(false, Ordering::Release);
            self.changed.notify_all();
            let child = self.window.clone();
            let _ = self.window.app_handle().run_on_main_thread(move || {
                let _ = crate::edit::native_presenter::hide_window(&child);
            });
        }
    }
    fn slot() -> &'static Mutex<Option<Arc<Worker>>> {
        static SLOT: OnceLock<Mutex<Option<Arc<Worker>>>> = OnceLock::new();
        SLOT.get_or_init(|| Mutex::new(None))
    }
    fn worker(session: &str) -> Result<Arc<Worker>, String> {
        slot()
            .lock()
            .map_err(|_| "画布锁不可用")?
            .as_ref()
            .filter(|w| w.session == session && !w.stopped.load(Ordering::Acquire))
            .cloned()
            .ok_or("原生画布已关闭".into())
    }
    pub async fn hide(session: &str, revision: Option<u64>) -> Result<(), String> {
        // A stopped renderer may still own a visible HWND until its thread exits.
        let current = slot()
            .lock()
            .map_err(|_| "Canvas lock unavailable")?
            .as_ref()
            .filter(|w| w.session == session)
            .cloned();
        let Some(w) = current else {
            return Ok(());
        };
        if let Some(revision) = revision {
            if revision < w.revision.fetch_max(revision, Ordering::AcqRel) {
                return Ok(());
            }
        }
        w.hide_epoch.fetch_add(1, Ordering::AcqRel);
        w.visible.store(false, Ordering::Release);
        w.shown.store(false, Ordering::Release);
        let (send, receive) = tokio::sync::oneshot::channel();
        let child = w.clone();
        w.window
            .app_handle()
            .run_on_main_thread(move || {
                let result = if child.visible.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    crate::edit::native_presenter::hide_window(&child.window)
                };
                let _ = send.send(result);
            })
            .map_err(|e| e.to_string())?;
        receive.await.map_err(|e| e.to_string())?
    }
    pub fn close(session: &str) {
        if let Ok(mut current) = slot().lock() {
            if current.as_ref().is_some_and(|w| w.session == session) {
                if let Some(w) = current.take() {
                    w.stop();
                }
            }
        }
    }

    pub async fn open(
        app: AppHandle,
        parent: WebviewWindow,
        session: String,
    ) -> Result<bool, String> {
        static LIFECYCLE: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let _lifecycle = LIFECYCLE
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        preview::canvas_source(&session)?;
        if worker(&session).is_ok() {
            return Ok(true);
        }
        if let Ok(mut current) = slot().lock() {
            if let Some(old) = current.take() {
                old.stop();
            }
        }
        let (send, receive) = tokio::sync::oneshot::channel();
        let owner = app.get_window(parent.label()).ok_or("编辑窗口已关闭")?;
        let app_clone = app.clone();
        let label = format!("editor-native-{}", uuid::Uuid::new_v4());
        app.run_on_main_thread(move || {
            let result = (|| -> Result<Arc<Window>, String> {
                let child = tauri::WindowBuilder::new(&app_clone, label)
                    .parent(&owner)
                    .map_err(|e| e.to_string())?
                    .decorations(false)
                    .shadow(false)
                    .resizable(false)
                    .skip_taskbar(true)
                    .focusable(false)
                    .focused(false)
                    .visible(false)
                    .inner_size(1.0, 1.0)
                    .build()
                    .map_err(|e| e.to_string())?;
                child
                    .set_ignore_cursor_events(true)
                    .map_err(|e| e.to_string())?;
                let hwnd = child.hwnd().map_err(|e| e.to_string())?;
                unsafe {
                    SetParent(hwnd, Some(owner.hwnd().map_err(|e| e.to_string())?))
                        .map_err(|e| e.to_string())?;
                    SetLayeredWindowAttributes(
                        hwnd,
                        windows::Win32::Foundation::COLORREF(0),
                        0,
                        LWA_ALPHA,
                    )
                    .map_err(|e| e.to_string())?;
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        ex & !((WS_EX_WINDOWEDGE
                            | WS_EX_CLIENTEDGE
                            | WS_EX_STATICEDGE
                            | WS_EX_DLGMODALFRAME)
                            .0 as isize),
                    );
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_STYLE,
                        (style
                            & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME).0
                                as isize))
                            | WS_CHILD.0 as isize,
                    );
                    SetWindowPos(
                        hwnd,
                        Some(HWND_TOP),
                        0,
                        0,
                        1,
                        1,
                        SWP_NOACTIVATE | SWP_FRAMECHANGED,
                    )
                    .map_err(|e| e.to_string())?;
                }
                Ok(Arc::new(child))
            })();
            let _ = send.send(result);
        })
        .map_err(|e| e.to_string())?;
        let child = receive.await.map_err(|e| e.to_string())??;
        let w = Arc::new(Worker {
            session: session.clone(),
            parent: parent.label().into(),
            window: child,
            latest: Mutex::new(None),
            changed: Condvar::new(),
            stopped: AtomicBool::new(false),
            visible: AtomicBool::new(false),
            shown: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            hide_epoch: AtomicU64::new(0),
            rect: Mutex::new(None),
            overlays:Mutex::new(Vec::new()),
        });
        let weak = Arc::downgrade(&w);
        parent.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                if let Some(w) = weak.upgrade() {
                    w.stop();
                }
            }
        });
        *slot().lock().map_err(|_| "画布锁不可用")? = Some(w.clone());
        let (ready, opened) = tokio::sync::oneshot::channel();
        std::thread::Builder::new().name("editor-native-canvas".into()).spawn(move || {
            // Surface is dropped before the native child window is destroyed.
            let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut gpu=match crate::edit::native_presenter::Presenter::new(w.window.clone()) {
                    Ok(gpu)=>gpu,
                    Err(error)=>{let _=ready.send(Err(error));return;}
                };
                let initial=match preview::canvas_source(&w.session){Ok(base)=>base,Err(_)=>return};
                let original_size=match preview::canvas_original_size(&w.session){Ok(size)=>size,Err(_)=>return};
                let mut renderer=Renderer::new(&initial).with_source_size(original_size,&initial);
                let mut overlay_renderer=Renderer::new(&initial);
                let mut annotation_doc:Option<photocraft_engine::doc::Document>=None;
                let mut previous_overlay:Option<(photocraft_engine::doc::DocId,EditRecipe,bool)>=None;
                let warm_start=Instant::now();
                let warm_view=View{rect:[0,0,1,1],uv:[0.0,0.0,1.0,1.0],image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,scale:1.0};
                let warmed=renderer.warm_canvas_document(&initial)
                    .and_then(|doc| gpu.draw(&doc,&warm_view,true))
                    .and_then(|_| gpu.wait_ready());
                if let Err(error)=warmed {crate::devices::diagnostics::record(format!("editor native warmup failed: {error}"));let _=ready.send(Err(error));return;}
                crate::devices::diagnostics::record(format!("editor native warmup {:.2} ms, depth={:?}, size={}x{}",warm_start.elapsed().as_secs_f64()*1000.0,initial.depth,initial.size.width,initial.size.height));
                let _=ready.send(Ok(true));
                let mut previous:Option<(photocraft_engine::doc::DocId,EditRecipe,bool,photocraft_engine::doc::Document)>=None;
                let mut completed=0;
                loop {
                    let next={let Ok(mut pending)=w.latest.lock() else {break};
                        while pending.is_none() && !w.stopped.load(Ordering::Acquire) {
                            pending=match w.changed.wait(pending){Ok(p)=>p,Err(_)=>return};
                        }
                        if w.stopped.load(Ordering::Acquire){break;} pending.take()};
                    let Some(mut frame)=next else {continue};
                    if !w.visible.load(Ordering::Acquire) || frame.epoch!=w.hide_epoch.load(Ordering::Acquire) || frame.revision<w.revision.load(Ordering::Acquire){continue;}
                    let start=Instant::now();
                    let result=(|| {
                        let base=preview::canvas_source(&w.session)?;
                        frame.view.mapping=crate::edit::native_geometry::mapping(&frame.recipe,base.size);
                        let mut render_recipe=frame.recipe.clone();render_recipe.rotate_quarter=0;render_recipe.geometry=None;render_recipe.crop=None;render_recipe.text_layers.clear();render_recipe.brush_strokes.clear();
                        let overlay_changed=previous_overlay.as_ref().is_none_or(|(id,old,original)|*id!=base.id || *original!=frame.original || old.text_layers!=frame.recipe.text_layers || old.brush_strokes!=frame.recipe.brush_strokes || old.rotate_quarter!=frame.recipe.rotate_quarter || old.geometry!=frame.recipe.geometry || old.crop!=frame.recipe.crop);
                        if overlay_changed {
                            annotation_doc=if frame.original {None}else{overlay_renderer.canvas_annotations(&base,&frame.recipe)?};
                            previous_overlay=Some((base.id,frame.recipe.clone(),frame.original));
                        }
                        let changed=!previous.as_ref().is_some_and(|(id,recipe,original,_)| *id==base.id && *recipe==render_recipe && *original==frame.original);
                        if changed {
                            let id=base.id;
                            let doc=if frame.original {base} else {renderer.canvas_document(&base,&render_recipe)?};
                            previous=Some((id,render_recipe,frame.original,doc));
                        }
                        gpu.draw_with_annotations(&previous.as_ref().ok_or("无绘制文档")?.3,&frame.view,changed,annotation_doc.as_ref(),overlay_changed)
                    })();
                    match result {
                        Ok(uploads)=>{
                            completed+=1;
                            if completed<=8 || uploads>0 {
                                if let Some((_,_,_,doc))=&previous {
                                    crate::devices::diagnostics::record(format!("editor native frame={} revision={} depth={:?} size={}x{} submit={:.2}ms uploaded={} readback=0",completed,frame.revision,doc.depth,doc.size.width,doc.size.height,start.elapsed().as_secs_f64()*1000.0,uploads));
                                }
                            }
                            // First make a transparent, already-rendered surface visible.
                            // Re-present after ShowWindow/WM_PAINT, then reveal it atomically.
                            if !w.shown.load(Ordering::Acquire) {
                                let (send,receive)=std::sync::mpsc::channel();let child=w.clone();
                                let posted=w.window.app_handle().run_on_main_thread(move || {
                                    let result=(|| -> Result<bool,String> {
                                        if !child.visible.load(Ordering::Acquire) || child.hide_epoch.load(Ordering::Acquire)!=frame.epoch {return Ok(false);}
                                        let hwnd=child.window.hwnd().map_err(|e|e.to_string())?;
                                        unsafe {
                                            SetLayeredWindowAttributes(hwnd,windows::Win32::Foundation::COLORREF(0),0,LWA_ALPHA).map_err(|e|e.to_string())?;
                                            SetWindowPos(hwnd,Some(HWND_TOP),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE|SWP_SHOWWINDOW).map_err(|e|e.to_string())?;
                                        }
                                        Ok(true)
                                    })();let _=send.send(result);
                                });
                                if posted.is_err() || !matches!(receive.recv(),Ok(Ok(true))){continue;}
                                let doc=&previous.as_ref().unwrap().3;
                                if gpu.draw_with_annotations(doc,&frame.view,false,annotation_doc.as_ref(),false).and_then(|_|gpu.wait_ready()).is_err(){w.stop();break;}
                                let (send,receive)=std::sync::mpsc::channel();let child=w.clone();
                                let _=w.window.app_handle().run_on_main_thread(move || {
                                    let visible=child.visible.load(Ordering::Acquire) && child.hide_epoch.load(Ordering::Acquire)==frame.epoch;
                                    if visible {if let Ok(hwnd)=child.window.hwnd(){unsafe {
                                        let _=SetLayeredWindowAttributes(hwnd,windows::Win32::Foundation::COLORREF(0),255,LWA_ALPHA);
                                    }}}
                                    let _=send.send(visible);
                                });
                                if !matches!(receive.recv(),Ok(true)){continue;}
                                w.shown.store(true,Ordering::Release);
                            }
                            let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",serde_json::json!({
                                "sessionId":w.session,"revision":frame.revision,"ok":true,
                                "submitMs":start.elapsed().as_secs_f64()*1000.0,"uploadedBytes":uploads,"readbackBytes":0}));
                        },
                        Err(error)=>{let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",
                            serde_json::json!({"sessionId":w.session,"revision":frame.revision,"ok":false,"error":error}));w.stop();break;}
                    }
                }
            }));
            if result.is_err() {let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",
                serde_json::json!({"sessionId":w.session,"ok":false,"error":"原生 GPU 画布故障，已恢复普通预览"}));}
            w.stop();let _=w.window.destroy();
        }).map_err(|e|e.to_string())?;
        opened.await.map_err(|e| e.to_string())?
    }

    pub async fn frame(
        app: AppHandle,
        session: &str,
        recipe: EditRecipe,
        view: View,
        revision: u64,
        original: bool,
    ) -> Result<(), String> {
        let w = worker(session)?;
        let epoch = w.hide_epoch.load(Ordering::Acquire);
        if revision < w.revision.fetch_max(revision, Ordering::AcqRel) {
            return Ok(());
        }
        let changed = {
            let mut old = w.rect.lock().map_err(|_| "画布锁不可用")?;
            let mut old_overlays=w.overlays.lock().map_err(|_|"画布锁不可用")?;
            let changed = *old != Some(view.rect) || *old_overlays!=view.overlays || !w.visible.load(Ordering::Acquire);
            *old = Some(view.rect);*old_overlays=view.overlays.clone();
            changed
        };
        if changed {
            let (send, receive) = tokio::sync::oneshot::channel();
            let child = w.window.clone();
            let rect = view.rect;
            let overlays=view.overlays.clone();
            app.run_on_main_thread(move || {
                let result = child
                    .hwnd()
                    .map_err(|e| e.to_string())
                    .and_then(|hwnd| unsafe {
                        SetWindowPos(
                            hwnd,
                            Some(HWND_TOP),
                            rect[0],
                            rect[1],
                            rect[2],
                            rect[3],
                            SWP_NOACTIVATE,
                        )
                        .map_err(|e| e.to_string())
                    });
                let result=result.and_then(|_|crate::edit::native_presenter::clip_overlays(&child,&overlays,rect[2],rect[3]));
                let _ = send.send(result);
            })
            .map_err(|e| e.to_string())?;
            receive.await.map_err(|e| e.to_string())??;
        }
        if epoch != w.hide_epoch.load(Ordering::Acquire)
            || revision < w.revision.load(Ordering::Acquire)
        {
            return Ok(());
        }
        w.visible.store(true, Ordering::Release);
        *w.latest.lock().map_err(|_| "画布锁不可用")? = Some(Frame {
            recipe,
            view,
            revision,
            original,
            epoch,
        });
        w.changed.notify_one();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{full_image,identity_mapping, View};
    #[test]
    fn invalid_view_cannot_reach_surface_configuration() {
        assert!(View {
            rect: [0, 0, 1920, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_ok());
        assert!(View {
            rect: [0, 0, 0, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 20000, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 1920, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,scale:1.0,
            uv: [f32::NAN, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
    }
}
