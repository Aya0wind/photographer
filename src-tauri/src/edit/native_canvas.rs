//! Experimental native canvas. Only parameters cross IPC; compositor output stays
//! on the GPU and is copied to a native swapchain. Other platforms retain JPEG.
use serde::Deserialize;
use tauri::{AppHandle, WebviewWindow};
#[derive(Default)]
struct RecipeCache {value:Option<(u64,std::sync::Arc<super::recipe::EditRecipe>)>}
impl RecipeCache {
    fn resolve(&mut self,recipe:Option<super::recipe::EditRecipe>,version:u64)->Result<std::sync::Arc<super::recipe::EditRecipe>,String>{
        if let Some(recipe)=recipe {
            if self.value.as_ref().is_some_and(|(old,stored)|version==*old&&**stored!=recipe){return Err("原生画布配方版本重复".into());}
            if self.value.as_ref().is_none_or(|(old,_)|version>*old){self.value=Some((version,std::sync::Arc::new(recipe)));}
        }
        self.value.as_ref().filter(|(stored,_)|*stored==version).map(|(_,recipe)|recipe.clone()).ok_or_else(||"原生画布配方版本不匹配".into())
    }
}

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
    #[serde(default)]
    pub straighten_guide:Option<[f32;4]>,
    #[serde(default)]
    pub compare_split:Option<f32>,
    #[serde(default)]
    pub clipping:u8,
    #[serde(default)]
    pub mask_guide:Option<[f32;8]>,
    #[serde(default)]
    pub mask_id:Option<String>,
    #[serde(default)]
    pub mask_preview:Option<[f64;4]>,
    #[serde(default)]
    pub mask_source_space:bool,
    #[serde(default)]
    pub mask_stroke:Option<super::recipe::MaskStroke>,
    #[serde(default="default_scale")]
    pub scale:f32,
}
fn identity_mapping()->[f32;8] {[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0]}
fn default_scale()->f32 {1.0}
fn mask_preview_recipe(recipe:&super::recipe::EditRecipe,view:&View)->Result<super::recipe::EditRecipe,String>{
    let mut preview=recipe.clone();
    if let Some(p)=view.mask_preview {
        let id=view.mask_id.as_ref().ok_or("临时蒙版未指定区域")?;
        let mask=preview.masks.iter_mut().find(|m|&m.id==id).ok_or("所选蒙版不存在")?;
        if mask.kind==super::recipe::MaskKind::Brush{return Err("画笔不支持渐变临时坐标".into());}
        mask.from=super::recipe::NormPoint{x:p[0],y:p[1]};mask.to=super::recipe::NormPoint{x:p[2],y:p[3]};
    }
    if let Some(stroke)=&view.mask_stroke {
        let count:usize=preview.masks.iter().flat_map(|m|&m.strokes).map(|s|s.points.len()).sum();
        if count.saturating_add(stroke.points.len())>100000{return Err("蒙版笔迹过长".into());}
        let id=view.mask_id.as_ref().ok_or("临时画笔未指定蒙版")?;
        let mask=preview.masks.iter_mut().find(|m|&m.id==id).ok_or("所选蒙版不存在")?;
        if mask.kind!=super::recipe::MaskKind::Brush{return Err("渐变不支持临时画笔".into());}
        mask.strokes.push(stroke.clone());
    }Ok(preview)
}
fn full_image() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}
impl View {
    fn validate(&self) -> Result<(), String> {
        if let Some(s)=&self.mask_stroke {
            if s.points.is_empty()||s.points.len()>100000||s.points.iter().any(|p|![p.x,p.y].iter().all(|v|v.is_finite()&&(0.0..=1.0).contains(v)))||![s.width_rel,s.hardness,s.opacity,s.flow].iter().all(|v|v.is_finite())||!(0.0001..=1.0).contains(&s.width_rel)||![s.hardness,s.opacity,s.flow].iter().all(|v|(0.0..=1.0).contains(v)){return Err("临时画笔参数无效".into());}
        }
        if self.mask_preview.is_some_and(|p|p.iter().any(|v|!v.is_finite()||!(0.0..=1.0).contains(v))){return Err("临时蒙版坐标无效".into());}
        if self.mask_id.as_ref().is_some_and(|id|id.is_empty()||id.len()>128)||self.mask_guide.is_some_and(|g|g.iter().any(|v|!v.is_finite()||v.abs()>1.0e7)||g[4]<0.0||g[5]<0.0||![0.0,1.0,2.0,3.0].contains(&g[6])||![0.0,1.0].contains(&g[7])){return Err("蒙版控制线无效".into());}
        if self.clipping>3{return Err("曝光提示参数无效".into());}
        if self.compare_split.is_some_and(|v|!v.is_finite()||!(0.0..=1.0).contains(&v)){return Err("对比分割位置无效".into());}
        if self.straighten_guide.is_some_and(|g|g.iter().any(|v|!v.is_finite()||*v<0.0||*v>1.0)){return Err("拉直辅助线无效".into());}
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
    recipe: Option<serde_json::Value>,
    recipe_revision:Option<u64>,
    view: View,
    revision: u64,
    original: bool,
) -> Result<bool, String> {
    view.validate()?;
    if window.label() != "advanced-editor" {
        return Err("原生画布窗口无效".into());
    }
    let recipe = recipe.as_ref().map(super::recipe::parse_recipe).transpose()?;
    #[cfg(windows)]
    {
        windows_canvas::frame(app, &session_id, recipe,recipe_revision.unwrap_or(revision), view, revision, original).await?;
        Ok(true)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, session_id, recipe, recipe_revision,revision, original);
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
        recipe: Arc<EditRecipe>,
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
        recipe:Mutex<RecipeCache>,
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
                    // parent() means an owned top-level window on Windows.
                    // Create WS_CHILD through the runtime so focus/style updates
                    // retain child semantics instead of restoring window chrome.
                    .parent_raw(owner.hwnd().map_err(|e| e.to_string())?)
                    .decorations(false)
                    .shadow(false)
                    .resizable(false)
                    .minimizable(false)
                    .maximizable(false)
                    .closable(false)
                    .title("")
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
                            & !((WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_BORDER | WS_DLGFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0
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
            recipe:Mutex::new(RecipeCache::default()),
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
                let mut previous_overlay:Option<(photocraft_engine::doc::DocId,Arc<EditRecipe>,bool)>=None;
                let mut previous_input:Option<(Arc<EditRecipe>,Option<[f64;4]>,Option<crate::edit::recipe::MaskStroke>)>=None;
                let mut previous_mask:Option<(photocraft_engine::doc::DocId,serde_json::Value)>=None;
                let mut gradient_overlay:Option<photocraft_engine::doc::Document>=None;
                let warm_start=Instant::now();
                let warm_view=View{rect:[0,0,1,1],uv:[0.0,0.0,1.0,1.0],image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0};
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
                        if previous.as_ref().is_none_or(|(id,_,_,_)|*id!=base.id){
                            let size=preview::canvas_original_size(&w.session)?;
                            renderer.set_source_size(size,&base);overlay_renderer.set_source_size(size,&base);
                        }
                        let input_changed=previous_input.as_ref().is_none_or(|(recipe,gradient,stroke)|!Arc::ptr_eq(recipe,&frame.recipe)||*gradient!=frame.view.mask_preview||*stroke!=frame.view.mask_stroke);
                        let mask_recipe=if frame.view.mask_preview.is_some()||frame.view.mask_stroke.is_some(){std::borrow::Cow::Owned(crate::edit::native_canvas::mask_preview_recipe(&frame.recipe,&frame.view)?)}else{std::borrow::Cow::Borrowed(frame.recipe.as_ref())};
                        let mut mask_uploads=0;
                        if let Some(id)=frame.view.mask_id.as_ref().filter(|_|frame.view.mask_guide.is_some_and(|g|g[7]>0.5)) {
                            let mask=mask_recipe.masks.iter().find(|m|&m.id==id).ok_or("所选蒙版不存在")?;
                            frame.view.mask_source_space=mask.components.is_empty()&&mask.feather==0.0&&mask.kind!=crate::edit::recipe::MaskKind::Brush;
                            let unchanged= !input_changed&&previous_mask.as_ref().is_some_and(|(old_id,old)|*old_id==base.id&&old["id"].as_str()==Some(id.as_str()));
                            if !unchanged {
                            let key=serde_json::json!({"id":id,"kind":mask.kind,"from":mask.from,"to":mask.to,"strokes":mask.strokes,"components":mask.components,"overlayColor":mask.overlay_color,"inverted":mask.inverted,"feather":mask.feather,"quarter":frame.recipe.rotate_quarter,"geometry":frame.recipe.geometry,"crop":frame.recipe.crop});
                            if !previous_mask.as_ref().is_some_and(|(old_id,old)|*old_id==base.id&&*old==key){
                                let doc=if frame.view.mask_source_space {let doc=crate::edit::masks::gradient_overlay_source(&renderer.neutral_document(&base,&frame.recipe)?,mask,gradient_overlay.as_ref())?;gradient_overlay=Some(doc.clone());doc}else{overlay_renderer.mask_overlay(&renderer.neutral_document(&base,&frame.recipe)?,&mask_recipe,id)?};
                                mask_uploads=gpu.prepare_mask_overlay(&doc)?;previous_mask=Some((base.id,key));
                            }
                            }
                        }
                        if frame.view.mask_id.is_none()||!frame.view.mask_guide.is_some_and(|g|g[7]>0.5){previous_mask=None;}
                        frame.view.mapping=crate::edit::native_geometry::mapping(&frame.recipe,base.size);
                        let overlay_changed=previous_overlay.as_ref().is_none_or(|(id,old,original)|*id!=base.id || *original!=frame.original || !Arc::ptr_eq(old,&frame.recipe)&&(old.text_layers!=frame.recipe.text_layers || old.brush_strokes!=frame.recipe.brush_strokes || old.rotate_quarter!=frame.recipe.rotate_quarter || old.geometry!=frame.recipe.geometry || old.crop!=frame.recipe.crop));
                        if overlay_changed {
                            annotation_doc=if frame.original {None}else{overlay_renderer.canvas_annotations(&renderer.neutral_document(&base,&frame.recipe)?,&frame.recipe)?};
                            previous_overlay=Some((base.id,frame.recipe.clone(),frame.original));
                        }
                        let candidate=if input_changed {let mut value=mask_recipe.as_ref().clone();value.rotate_quarter=0;value.geometry=None;value.crop=None;value.text_layers.clear();value.brush_strokes.clear();Some(value)}else{None};
                        let changed=previous.as_ref().is_none_or(|(id,recipe,original,_)|*id!=base.id||*original!=frame.original||candidate.as_ref().is_some_and(|value|value!=recipe));
                        if changed {
                            let render_recipe=candidate.or_else(||previous.as_ref().map(|(_,recipe,_,_)|recipe.clone())).ok_or("无绘制配方")?;
                            let id=base.id;
                            let doc=if frame.original {renderer.neutral_document(&base,&frame.recipe)?} else {renderer.canvas_document(&base,&render_recipe)?};
                            previous=Some((id,render_recipe,frame.original,doc));
                        }
                        previous_input=Some((frame.recipe.clone(),frame.view.mask_preview,frame.view.mask_stroke.clone()));
                        let comparison_uploads=if frame.view.compare_split.is_some(){gpu.prepare_comparison(&renderer.neutral_document(&base,&frame.recipe)?)?}else{0};
                        gpu.draw_with_annotations(&previous.as_ref().ok_or("无绘制文档")?.3,&frame.view,changed,annotation_doc.as_ref(),overlay_changed).map(|uploads|uploads+comparison_uploads+mask_uploads)
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
        recipe: Option<EditRecipe>,
        recipe_revision:u64,
        view: View,
        revision: u64,
        original: bool,
    ) -> Result<(), String> {
        let w = worker(session)?;
        let recipe=w.recipe.lock().map_err(|_|"配方缓存锁不可用")?.resolve(recipe,recipe_revision)?;
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
    fn cached_recipe_views_require_exact_version_and_cannot_roll_back_newer_data(){
        let mut cache=super::RecipeCache::default();let recipe=crate::edit::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft"})).unwrap();
        assert!(cache.resolve(None,1).is_err());assert_eq!(cache.resolve(Some(recipe.clone()),1).unwrap().as_ref(),&recipe);assert_eq!(cache.resolve(None,1).unwrap().as_ref(),&recipe);
        let mut changed=recipe.clone();changed.rotate_quarter=1;assert!(cache.resolve(Some(changed.clone()),1).is_err());assert_eq!(cache.resolve(None,1).unwrap().as_ref(),&recipe);
        assert_eq!(cache.resolve(Some(changed.clone()),3).unwrap().as_ref(),&changed);assert!(cache.resolve(Some(recipe),2).is_err());assert_eq!(cache.resolve(None,3).unwrap().as_ref(),&changed);assert!(cache.resolve(None,4).is_err());
    }
    #[test]
    fn recipe_cache_reuses_immutable_allocation_and_preserves_old_snapshots(){
        let mut cache=super::RecipeCache::default();let recipe=crate::edit::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft"})).unwrap();
        let first=cache.resolve(Some(recipe.clone()),1).unwrap();let view=cache.resolve(None,1).unwrap();assert!(std::sync::Arc::ptr_eq(&first,&view));
        let repeated=cache.resolve(Some(recipe.clone()),1).unwrap();assert!(std::sync::Arc::ptr_eq(&first,&repeated));
        let mut changed=recipe.clone();changed.rotate_quarter=1;let second=cache.resolve(Some(changed),2).unwrap();assert!(!std::sync::Arc::ptr_eq(&first,&second));assert_eq!(first.rotate_quarter,0);assert_eq!(second.rotate_quarter,1);
    }
    #[test]
    #[ignore="large recipe reference timing; run explicitly"]
    fn large_recipe_reference_timing(){
        let mut recipe=crate::edit::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","masks":[{"id":"m","name":"Brush","kind":"brush","enabled":true,"inverted":false,"density":100,"feather":0,"from":{"x":0,"y":0},"to":{"x":1,"y":1}}]})).unwrap();
        recipe.masks[0].strokes.push(crate::edit::recipe::MaskStroke{points:(0..100000).map(|n|crate::edit::recipe::NormPoint{x:n as f64/100000.0,y:0.5}).collect(),width_rel:0.1,hardness:0.5,opacity:0.5,flow:0.5,erase:false});
        let mut cache=super::RecipeCache::default();let first=cache.resolve(Some(recipe),1).unwrap();let start=std::time::Instant::now();for _ in 0..1000{std::hint::black_box(cache.resolve(None,1).unwrap());}let shared=start.elapsed().as_secs_f64();
        let start=std::time::Instant::now();for _ in 0..1000{std::hint::black_box(first.as_ref().clone());}let copied=start.elapsed().as_secs_f64();eprintln!("[recipe references] 100k points, shared {:.4} ms; deep clone {:.4} ms",shared,copied);
    }
    #[test]
    fn temporary_brush_and_eraser_update_pixels_without_committing_a_stroke(){
        use photocraft_engine::doc::{Document,Size,Color,ColorMode,SampleType};
        let recipe=crate::edit::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","masks":[{"id":"mask","name":"Brush","kind":"brush","enabled":true,"inverted":false,"density":100,"feather":0,"from":{"x":0,"y":0},"to":{"x":1,"y":1},"advanced":{"exposure":1}}]})).unwrap();
        let stroke=serde_json::from_value(serde_json::json!({"points":[{"x":0.3,"y":0.5},{"x":0.7,"y":0.5}],"widthRel":0.4,"hardness":0.8,"opacity":0.7,"flow":0.5,"erase":false})).unwrap();
        let mut view=View{rect:[0,0,640,400],uv:full_image(),image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:Some([0.3,0.5,0.7,0.5,0.2,0.2,3.0,1.0]),mask_id:Some("mask".into()),mask_preview:None,mask_source_space:false,mask_stroke:Some(stroke),scale:1.0};
        assert!(view.validate().is_ok());let painting=super::mask_preview_recipe(&recipe,&view).unwrap();assert!(recipe.masks[0].strokes.is_empty());assert_eq!(painting.masks[0].strokes.len(),1);
        let base=Document::with_background("Photo",Size::new(32,24),ColorMode::Rgb,SampleType::U8,Color::rgba(0.3,0.3,0.3,1.0));
        let before=photocraft_compose::flatten(&crate::edit::photocraft::render_document(&base,&recipe).unwrap());let painted=photocraft_compose::flatten(&crate::edit::photocraft::render_document(&base,&painting).unwrap());assert_ne!(before.px,painted.px);
        view.mask_stroke.as_mut().unwrap().erase=true;view.mask_stroke.as_mut().unwrap().opacity=1.0;view.mask_stroke.as_mut().unwrap().flow=1.0;
        let erasing=super::mask_preview_recipe(&painting,&view).unwrap();let erased=photocraft_compose::flatten(&crate::edit::photocraft::render_document(&base,&erasing).unwrap());assert_ne!(painted.px,erased.px);
        view.mask_stroke=None;assert_eq!(super::mask_preview_recipe(&recipe,&view).unwrap(),recipe);
    }
    #[test]
    fn temporary_gradient_moves_local_effect_without_changing_saved_recipe(){
        use photocraft_engine::doc::{Document,Size,Color,ColorMode,SampleType};
        let recipe=crate::edit::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","masks":[{"id":"mask","name":"Gradient","kind":"linear","enabled":true,"inverted":false,"density":100,"feather":0,"from":{"x":0.1,"y":0.5},"to":{"x":0.5,"y":0.5},"advanced":{"exposure":2}}]})).unwrap();
        let mut view=View{rect:[0,0,640,400],uv:full_image(),image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:Some("mask".into()),mask_preview:Some([0.5,0.5,0.9,0.5]),mask_source_space:false,mask_stroke:None,scale:1.0};
        let preview=super::mask_preview_recipe(&recipe,&view).unwrap();assert_eq!(recipe.masks[0].from.x,0.1);assert_eq!(preview.masks[0].from.x,0.5);
        let base=Document::with_background("Photo",Size::new(16,8),ColorMode::Rgb,SampleType::U8,Color::rgba(0.3,0.3,0.3,1.0));
        let before=photocraft_compose::flatten(&crate::edit::photocraft::render_document(&base,&recipe).unwrap());
        let moving=photocraft_compose::flatten(&crate::edit::photocraft::render_document(&base,&preview).unwrap());assert_ne!(before.px,moving.px);
        view.mask_preview=None;assert_eq!(super::mask_preview_recipe(&recipe,&view).unwrap(),recipe);
    }
    #[test]
    fn mask_guides_accept_offscreen_handles_and_reject_invalid_payloads(){
        let mut view=View{rect:[0,0,640,400],uv:full_image(),image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:Some([-1.0,0.5,2.0,0.5,1.5,1.5,2.0,1.0]),mask_id:Some("mask".into()),mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0};
        assert!(view.validate().is_ok());view.mask_guide.as_mut().unwrap()[6]=4.0;assert!(view.validate().is_err());
        view.mask_guide.as_mut().unwrap()[6]=1.0;view.mask_guide.as_mut().unwrap()[7]=0.0;assert!(view.validate().is_ok());
        view.mask_guide.as_mut().unwrap()[0]=f32::NAN;assert!(view.validate().is_err());
    }
    #[test]
    fn comparison_split_accepts_endpoints_and_rejects_invalid_positions(){
        let mut view=View{rect:[0,0,640,400],uv:full_image(),image:full_image(),overlays:vec![],mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0};
        for value in [0.0,0.5,1.0]{view.compare_split=Some(value);assert!(view.validate().is_ok());}
        for value in [-0.1,1.1,f32::NAN,f32::INFINITY]{view.compare_split=Some(value);assert!(view.validate().is_err());}
        view.compare_split=None;
        for warnings in 0..=3{view.clipping=warnings;assert!(view.validate().is_ok());}
        view.clipping=4;assert!(view.validate().is_err());
    }
    #[test]
    fn invalid_view_cannot_reach_surface_configuration() {
        assert!(View {
            rect: [0, 0, 1920, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_ok());
        assert!(View {
            rect: [0, 0, 0, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 20000, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0,
            uv: [0.0, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
        assert!(View {
            rect: [0, 0, 1920, 1080],
            image: full_image(),
            overlays:vec![],
            mapping:identity_mapping(),crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0,
            uv: [f32::NAN, 0.0, 1.0, 1.0]
        }
        .validate()
        .is_err());
    }
}
