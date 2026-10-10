//! Hidden, disposable hardware validation of the native presentation path.
//! No library/photos/settings are opened or changed.
#[cfg(windows)]
mod native_canvas {
    pub struct View {
        pub rect: [i32; 4],
        pub uv: [f32; 4],
        pub image: [f32; 4],
        // 探针只写不读（presenter 输出对账用）；WIP。
        #[allow(dead_code)]
        pub overlays:Vec<[i32;5]>,
        pub mapping:[f32;8],
        pub crop_guide:Option<[f32;4]>,
        pub scale:f32,
    }
}
#[cfg(windows)]
#[path = "../src/edit/native_presenter.rs"]
mod native_presenter;


#[cfg(windows)]
mod recipe {
    #[derive(Clone,Default)]
    pub struct Geometry {pub angle:f64,pub flip_horizontal:bool,pub flip_vertical:bool}
    #[derive(Clone,Copy)]
    pub struct CropRect {pub x:f64,pub y:f64,pub w:f64,pub h:f64}
    pub struct EditRecipe {pub rotate_quarter:u32,pub geometry:Option<Geometry>,pub crop:Option<CropRect>}
}
#[cfg(windows)]
#[path="../src/edit/native_geometry.rs"]
mod native_geometry;
#[cfg(windows)]
fn main() {
    use std::sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    };
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let status = Arc::new(AtomicI32::new(1));
    let worker_status = status.clone();
    tauri::Builder::default()
        .setup(move |app| {
            let parent = tauri::WindowBuilder::new(app, "native-probe-owner")
                .visible(false)
                .skip_taskbar(true)
                .build()?;
            let window = tauri::WindowBuilder::new(app, "native-canvas-probe")
                .title("Photographer GPU validation")
                .inner_size(640.0, 400.0)
                .visible(false)
                .skip_taskbar(true)
                .decorations(false)
                .build()?;
            window.set_ignore_cursor_events(true)?;
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(
                    window.hwnd()?,
                    windows::Win32::Foundation::COLORREF(0),
                    255,
                    windows::Win32::UI::WindowsAndMessaging::LWA_ALPHA,
                )?;
            }
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::*;
                SetParent(window.hwnd()?, Some(parent.hwnd()?))?;
                let style = GetWindowLongPtrW(window.hwnd()?, GWL_STYLE);
                SetWindowLongPtrW(
                    window.hwnd()?,
                    GWL_STYLE,
                    (style & !(WS_POPUP.0 as isize)) | WS_CHILD.0 as isize,
                );
                SetWindowPos(
                    window.hwnd()?,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
                )?;
                assert_ne!(
                    GetWindowLongPtrW(window.hwnd()?, GWL_STYLE) & WS_VISIBLE.0 as isize,
                    0
                );
                native_presenter::hide_window(&window).map_err(std::io::Error::other)?;
                assert_eq!(
                    GetWindowLongPtrW(window.hwnd()?, GWL_STYLE) & WS_VISIBLE.0 as isize,
                    0
                );
                eprintln!("[native probe] reparented HWND hidden: WS_VISIBLE cleared, alpha=0");
            }
            native_presenter::clip_overlays(&window,&[],640,400).map_err(std::io::Error::other)?;
            native_presenter::clip_overlays(&window,&[[200,330,240,48,16]],640,400).map_err(std::io::Error::other)?;
            unsafe {
                use windows::Win32::Graphics::Gdi::*;
                let region=CreateRectRgn(0,0,0,0);
                assert_ne!(GetWindowRgn(window.hwnd()?,region).0,0);
                assert!(!PtInRegion(region,320,350).as_bool(),"HUD must be outside the native drawing region");
                assert!(PtInRegion(region,100,100).as_bool(),"photo must remain inside the native drawing region");
                let _=DeleteObject(HGDIOBJ(region.0));
            }
            native_presenter::clip_overlays(&window,&[],640,400).map_err(std::io::Error::other)?;
            eprintln!("[native probe] floating HUD clipping applied and cleared");
            let handle = app.handle().clone();
            let worker_status = worker_status.clone();
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> Result<(), String> {
                        use photocraft_engine::doc::{
                            Color, ColorMode, Document, SampleType, Size,
                        };
                        let mut presenter =
                            native_presenter::Presenter::new(Arc::new(window.clone()))?;
                        let doc = Document::with_background(
                            "Probe",
                            Size::new(640, 400),
                            ColorMode::Rgb,
                            SampleType::U8,
                            Color::rgba(0.2, 0.5, 0.7, 1.0),
                        );
                        let mut view = native_canvas::View {
                            rect: [0, 0, 640, 400],
                            uv: [0.0, 0.0, 1.0, 1.0],
                            image: [0.0, 0.0, 1.0, 1.0],
                            overlays:vec![],mapping:[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0],crop_guide:None,scale:1.0,
                        };
                        // Production prewarms at 1x1, the same dimensions as
                        // default config. This must configure the surface too.
                        let warm_view=native_canvas::View{rect:[0,0,1,1],uv:[0.0,0.0,1.0,1.0],image:[0.0,0.0,1.0,1.0],overlays:vec![],mapping:[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0],crop_guide:None,scale:1.0};
                        presenter.draw(&doc,&warm_view,true)?;
                        presenter.wait_ready()?;
                        eprintln!("[native probe] initial 1x1 warmup succeeded");
                        let first = std::time::Instant::now();
                        let uploaded = presenter.draw(&doc, &view, true)?;
                        eprintln!(
                            "[native probe] first submit {:.2} ms, uploaded {uploaded} bytes",
                            first.elapsed().as_secs_f64() * 1000.0
                        );
                        // Exercise an actual adjustment, not just an unchanged blit.
                        use photocraft_engine::doc::{Layer, LayerContent};
                        let mut adjusted = doc.clone();
                        let adjustment = photocraft_engine::adjust_params::from_params(
                            "curves",
                            &serde_json::json!({"points":[[0,0],[128,145],[255,255]]}),
                            None,
                            ColorMode::Rgb,
                        )
                        .map_err(|e| e.to_string())?;
                        adjusted.layers.push(Layer::new(
                            "Exposure curve",
                            LayerContent::Adjustment(adjustment),
                        ));
                        let cold = std::time::Instant::now();
                        presenter.draw(&adjusted, &view, true)?;
                        presenter.wait_ready()?;
                        eprintln!(
                            "[native probe] cold adjustment completed {:.2} ms",
                            cold.elapsed().as_secs_f64() * 1000.0
                        );
                        let adjustment = photocraft_engine::adjust_params::from_params(
                            "curves",
                            &serde_json::json!({"points":[[0,0],[128,155],[255,255]]}),
                            None,
                            ColorMode::Rgb,
                        )
                        .map_err(|e| e.to_string())?;
                        adjusted.layers.last_mut().unwrap().content =
                            LayerContent::Adjustment(adjustment);
                        let changed = std::time::Instant::now();
                        presenter.draw(&adjusted, &view, true)?;
                        presenter.wait_ready()?;
                        eprintln!(
                            "[native probe] warmed changed adjustment completed {:.2} ms",
                            changed.elapsed().as_secs_f64() * 1000.0
                        );
                        presenter.draw(&doc, &view, true)?;
                        let warm = std::time::Instant::now();
                        for _ in 0..10 {
                            presenter.draw(&doc, &view, false)?;
                        }
                        eprintln!(
                            "[native probe] cached submit average {:.2} ms; readback=0; JPEG=0",
                            warm.elapsed().as_secs_f64() * 1000.0 / 10.0
                        );
                        // Match the 16-bit RAW proxy used by the real editor.
                        let raw=Document::with_background("16-bit RAW proxy",Size::new(1600,1067),
                            ColorMode::Rgb,SampleType::U16,Color::rgba(0.2,0.5,0.7,1.0));
                        let mut raw=raw;
                        let layer=adjusted.layers.last().unwrap().clone();
                        raw.layers.push(layer);
                        presenter.draw(&raw,&view,true)?;presenter.wait_ready()?;
                        let mut uploads=0;
                        let started=std::time::Instant::now();
                        for n in 0..20 {
                            let adjustment=photocraft_engine::adjust_params::from_params("curves",
                                &serde_json::json!({"points":[[0,0],[128,130+n],[255,255]]}),None,ColorMode::Rgb)
                                .map_err(|e|e.to_string())?;
                            raw.layers.last_mut().unwrap().content=LayerContent::Adjustment(adjustment);
                            uploads+=presenter.draw(&raw,&view,true)?;
                            presenter.wait_ready()?;
                        }
                        assert_eq!(uploads,0,"slider changes must retain 16-bit source textures on GPU");
                        eprintln!("[native probe] 16-bit RAW changing curves {:.2} ms/frame, source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
                        let mut look=doc.clone();
                        let adjustment=photocraft_engine::adjust_params::from_params("colorLookup",
                            &serde_json::json!({"lut":"warm","interpolation":"tetrahedral"}),None,ColorMode::Rgb)
                            .map_err(|e|e.to_string())?;
                        look.layers.push(Layer::new("Creative LUT",LayerContent::Adjustment(adjustment)));
                        presenter.draw(&look,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();
                        let mut uploaded=0;
                        for n in 0..10 {look.layers.last_mut().unwrap().opacity=(n as f32+1.0)/10.0;
                            uploaded+=presenter.draw(&look,&view,true)?;presenter.wait_ready()?;}
                        assert_eq!(uploaded,0,"LUT strength must not reupload source pixels");
                        eprintln!("[native probe] LUT strength {:.2} ms/frame; source reupload={uploaded}",started.elapsed().as_secs_f64()*1000.0/10.0);
                        let started=std::time::Instant::now();
                        let mut geometry_uploads=0;
                        presenter.draw(&doc,&view,true)?;presenter.wait_ready()?;
                        for n in 0..30 {
                            let r=recipe::EditRecipe{rotate_quarter:1,geometry:Some(recipe::Geometry{angle:n as f64,flip_horizontal:true,flip_vertical:false}),crop:Some(recipe::CropRect{x:0.1,y:0.1,w:0.8,h:0.8})};
                            view.mapping=native_geometry::mapping(&r,doc.size);
                            view.crop_guide=Some([0.15,0.2,0.65,0.6]);
                            geometry_uploads+=presenter.draw(&doc,&view,false)?;presenter.wait_ready()?;
                        }
                        assert_eq!(geometry_uploads,0,"geometry must only update display uniforms");
                        eprintln!("[native probe] geometry and crop guides {:.2} ms/frame; source reupload={geometry_uploads}",started.elapsed().as_secs_f64()*1000.0/30.0);
                        let overlay=Document::with_background("Overlay",Size::new(400,300),ColorMode::Rgb,SampleType::U8,Color::rgba(1.0,0.0,0.0,0.25));
                        presenter.draw_with_annotations(&doc,&view,false,Some(&overlay),true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for _ in 0..10 {uploads+=presenter.draw_with_annotations(&doc,&view,false,Some(&overlay),false)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"unchanged annotation overlays must remain resident");
                        eprintln!("[native probe] transformed photo and annotations {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/10.0);
                        let mut mask_session=photocraft_engine::Session::new();
                        mask_session.add_document(Document::new("Mask",doc.size,ColorMode::Rgb,SampleType::U8),None);
                        for (command,params) in [
                            ("layer.new.layer",serde_json::json!({})),
                            ("layer.layerMask.hideAll",serde_json::json!({})),
                            ("paint.stroke",serde_json::json!({"target":"mask","points":[[400,500,1],[1200,500,1]],"size":400,"hardness":0.5,"opacity":1,"flow":1,"color":"#ffffff"}))
                        ] {mask_session.execute(command,params).map_err(|e|e.to_string())?;}
                        let mut local=doc.clone();
                        let mut layer=adjusted.layers.last().unwrap().clone();
                        layer.mask=mask_session.active().unwrap().doc.layers.last().unwrap().mask.clone();
                        layer.mask.as_mut().unwrap().feather=12.0;
                        local.layers.push(layer);
                        presenter.draw(&local,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..20 {
                            let adjustment=photocraft_engine::adjust_params::from_params("brightnessContrast",&serde_json::json!({"brightness":n*2,"contrast":0}),None,ColorMode::Rgb).map_err(|e|e.to_string())?;
                            local.layers.last_mut().unwrap().content=LayerContent::Adjustment(adjustment);
                            uploads+=presenter.draw(&local,&view,true)?;presenter.wait_ready()?;
                        }
                        assert_eq!(uploads,0,"local slider edits must reuse source and mask textures");
                        eprintln!("[native probe] feathered local adjustments {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
                        let mut colors=doc.clone();
                        for (kind,params) in [
                            ("colorBalance",serde_json::json!({"midtones":[20,-10,5]})),
                            ("blackWhite",serde_json::json!({"tint":true,"tintColor":"#E1D3B3"})),
                            ("selectiveColor",serde_json::json!({"reds":[20,0,0,0]}))
                        ] {
                            let adjustment=photocraft_engine::adjust_params::from_params(kind,&params,None,ColorMode::Rgb).map_err(|e|e.to_string())?;
                            colors.layers.push(Layer::new(kind,LayerContent::Adjustment(adjustment)));
                        }
                        presenter.draw(&colors,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..20 {
                            let adjustment=photocraft_engine::adjust_params::from_params("colorBalance",&serde_json::json!({"midtones":[n*2,-10,5]}),None,ColorMode::Rgb).map_err(|e|e.to_string())?;
                            colors.layers[1].content=LayerContent::Adjustment(adjustment);
                            uploads+=presenter.draw(&colors,&view,true)?;presenter.wait_ready()?;
                        }
                        assert_eq!(uploads,0,"extended colors must not reupload the source");
                        eprintln!("[native probe] color balance + tinted black-white + selective color {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
                        Ok(())
                    },
                ));
                let success = matches!(result, Ok(Ok(())));
                if !success {
                    eprintln!("[native probe] failed: {result:?}");
                }
                worker_status.store(if success { 0 } else { 1 }, Ordering::Release);
                let _ = window.destroy();
                handle.exit(if success { 0 } else { 1 });
            });
            Ok(())
        })
        .run(context)
        .expect("probe runtime");
    std::process::exit(status.load(Ordering::Acquire));
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Native prototype validation currently targets Windows.");
}
