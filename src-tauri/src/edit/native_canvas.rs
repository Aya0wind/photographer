//! Experimental native canvas. Only parameters cross IPC; compositor output stays
//! on the GPU and is copied to a native swapchain. Other platforms retain JPEG.
use serde::Deserialize;
use tauri::{AppHandle, WebviewWindow};

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub rect: [i32; 4],
    pub uv: [f32; 4],
}
impl View {
    fn validate(&self) -> Result<(), String> {
        if self.rect[2] <= 0
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
pub fn edit_native_hide(session_id: String) {
    #[cfg(windows)]
    windows_canvas::hide(&session_id);
    #[cfg(not(windows))]
    let _ = session_id;
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
        rect: Mutex<Option<[i32; 4]>>,
    }
    impl Worker {
        fn stop(&self) {
            self.stopped.store(true, Ordering::Release);
            self.visible.store(false, Ordering::Release);
            self.shown.store(false, Ordering::Release);
            self.changed.notify_all();
            let _ = self.window.hide();
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
    pub fn hide(session: &str) {
        if let Ok(w) = worker(session) {
            w.visible.store(false, Ordering::Release);
            w.shown.store(false, Ordering::Release);
            let _ = w.window.hide();
        }
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
        parent
            .set_background_color(Some(tauri::utils::config::Color(0, 0, 0, 0)))
            .map_err(|e| e.to_string())?;
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
        let shade = if matches!(parent.theme(), Ok(tauri::Theme::Light)) {
            tauri::utils::config::Color(245, 245, 247, 255)
        } else {
            tauri::utils::config::Color(14, 15, 18, 255)
        };
        owner
            .set_background_color(Some(shade))
            .map_err(|e| e.to_string())?;
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
                let hwnd = child.hwnd().map_err(|e| e.to_string())?;
                unsafe {
                    SetParent(hwnd, Some(owner.hwnd().map_err(|e| e.to_string())?))
                        .map_err(|e| e.to_string())?;
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_STYLE,
                        (style & !(WS_POPUP.0 as isize)) | WS_CHILD.0 as isize,
                    );
                    SetWindowPos(
                        hwnd,
                        Some(HWND_BOTTOM),
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
            rect: Mutex::new(None),
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
                    Ok(gpu)=>{let _=ready.send(Ok(true));gpu},
                    Err(error)=>{let _=ready.send(Err(error));return;}
                };
                let initial=match preview::canvas_source(&w.session){Ok(base)=>base,Err(_)=>return};
                let mut renderer=Renderer::new(&initial);
                let mut previous:Option<(photocraft_engine::doc::DocId,EditRecipe,bool,photocraft_engine::doc::Document)>=None;
                loop {
                    let next={let Ok(mut pending)=w.latest.lock() else {break};
                        while pending.is_none() && !w.stopped.load(Ordering::Acquire) {
                            pending=match w.changed.wait(pending){Ok(p)=>p,Err(_)=>return};
                        }
                        if w.stopped.load(Ordering::Acquire){break;} pending.take()};
                    let Some(frame)=next else {continue};
                    if !w.visible.load(Ordering::Acquire) || frame.revision<w.revision.load(Ordering::Acquire){continue;}
                    let start=Instant::now();
                    let result=(|| {
                        let base=preview::canvas_source(&w.session)?;
                        let changed=!previous.as_ref().is_some_and(|(id,recipe,original,_)| *id==base.id && *recipe==frame.recipe && *original==frame.original);
                        if changed {
                            let id=base.id;
                            let doc=if frame.original {base} else {renderer.canvas_document(&base,&frame.recipe)?};
                            previous=Some((id,frame.recipe.clone(),frame.original,doc));
                        }
                        gpu.draw(&previous.as_ref().ok_or("无绘制文档")?.3,&frame.view,changed)
                    })();
                    match result {
                        Ok(uploads)=>{
                            if w.visible.load(Ordering::Acquire) && !w.shown.swap(true,Ordering::AcqRel) {
                                let child=w.clone();
                                let _=w.window.app_handle().run_on_main_thread(move || {
                                    if child.visible.load(Ordering::Acquire) {if let Ok(hwnd)=child.window.hwnd() {unsafe {
                                        let _=SetWindowPos(hwnd,Some(HWND_BOTTOM),0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE|SWP_SHOWWINDOW);
                                    }}}
                                });
                            }
                            let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",serde_json::json!({
                                "sessionId":w.session,"revision":frame.revision,"ok":true,
                                "submitMs":start.elapsed().as_secs_f64()*1000.0,"uploadedBytes":uploads,"readbackBytes":0}));
                        },
                        Err(error)=>{let _=w.window.hide();let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",
                            serde_json::json!({"sessionId":w.session,"revision":frame.revision,"ok":false,"error":error}));w.stop();break;}
                    }
                }
            }));
            if result.is_err() {let _=w.window.app_handle().emit_to(&w.parent,"editor-native-frame",
                serde_json::json!({"sessionId":w.session,"ok":false,"error":"原生 GPU 画布故障，已恢复普通预览"}));}
            let _=w.window.hide();let _=w.window.destroy();
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
        if revision < w.revision.fetch_max(revision, Ordering::AcqRel) {
            return Ok(());
        }
        let changed = {
            let mut old = w.rect.lock().map_err(|_| "画布锁不可用")?;
            let changed = *old != Some(view.rect);
            *old = Some(view.rect);
            changed
        };
        if changed {
            let (send, receive) = tokio::sync::oneshot::channel();
            let child = w.window.clone();
            let rect = view.rect;
            app.run_on_main_thread(move || {
                let result = child
                    .hwnd()
                    .map_err(|e| e.to_string())
                    .and_then(|hwnd| unsafe {
                        SetWindowPos(
                            hwnd,
                            Some(HWND_BOTTOM),
                            rect[0],
                            rect[1],
                            rect[2],
                            rect[3],
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        )
                        .map_err(|e| e.to_string())
                    });
                let _ = send.send(result);
            })
            .map_err(|e| e.to_string())?;
            receive.await.map_err(|e| e.to_string())??;
        }
        if revision < w.revision.load(Ordering::Acquire) {
            return Ok(());
        }
        w.visible.store(true, Ordering::Release);
        *w.latest.lock().map_err(|_| "画布锁不可用")? = Some(Frame {
            recipe,
            view,
            revision,
            original,
        });
        w.changed.notify_one();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::View;
    #[test]
    fn invalid_view_cannot_reach_surface_configuration() {
        assert!(View {
            rect: [0, 0, 1920, 1080],
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_ok());
        assert!(View {
            rect: [0, 0, 0, 1080],
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 20000, 1080],
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 1920, 1080],
            uv: [f32::NAN, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
    }
}
