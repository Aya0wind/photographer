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
        pub straighten_guide:Option<[f32;4]>,
        pub compare_split:Option<f32>,
        pub clipping:u8,
        pub mask_guide:Option<[f32;8]>,
        #[allow(dead_code)]
        pub mask_id:Option<String>,
        #[allow(dead_code)]
        pub mask_preview:Option<[f64;4]>,
        pub mask_source_space:bool,
        #[allow(dead_code)]
        pub mask_stroke:Option<serde_json::Value>,
        pub scale:f32,
    }
}
#[cfg(windows)]
#[path = "../src/edit/native_presenter.rs"]
mod native_presenter;


#[cfg(windows)]
mod recipe {
    pub struct ParametricCurve{pub highlights:f64,pub lights:f64,pub darks:f64,pub shadows:f64,pub splits:[f64;3]}
    #[derive(Clone,serde::Serialize)]
    pub struct ColorGrading {pub shadows:photocraft_algo::camera_raw::Wheel,pub midtones:photocraft_algo::camera_raw::Wheel,pub highlights:photocraft_algo::camera_raw::Wheel,pub global:photocraft_algo::camera_raw::Wheel,pub blending:f64,pub balance:f64}
    impl Default for ColorGrading{fn default()->Self{Self{shadows:Default::default(),midtones:Default::default(),highlights:Default::default(),global:Default::default(),blending:50.0,balance:0.0}}}
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
#[path="../src/edit/grading.rs"]
mod grading;
#[cfg(windows)]
#[path="../src/edit/parametric.rs"]
mod parametric;
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
                            overlays:vec![],mapping:[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0],crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0,
                        };
                        // Production prewarms at 1x1, the same dimensions as
                        // default config. This must configure the surface too.
                        let warm_view=native_canvas::View{rect:[0,0,1,1],uv:[0.0,0.0,1.0,1.0],image:[0.0,0.0,1.0,1.0],overlays:vec![],mapping:[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0],crop_guide:None,straighten_guide:None,compare_split:None,clipping:0,mask_guide:None,mask_id:None,mask_preview:None,mask_source_space:false,mask_stroke:None,scale:1.0};
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
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..20 {view.straighten_guide=Some([0.15,0.3,0.8,0.3+n as f32*0.01]);uploads+=presenter.draw(&doc,&view,false)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"straightening guides only update uniforms");
                        eprintln!("[native probe] straighten guide {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);view.straighten_guide=None;
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
                        let mut grouped=doc.clone();
                        let mut child=local.layers.last().unwrap().clone();let mask=child.mask.take();
                        let mut group=Layer::group("Local adjustments",vec![child]);group.mask=mask;group.opacity=0.6;
                        grouped.layers.push(group);
                        presenter.draw(&grouped,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..20 {
                            let adjustment=photocraft_engine::adjust_params::from_params("brightnessContrast",&serde_json::json!({"brightness":n*2,"contrast":0}),None,ColorMode::Rgb).map_err(|e|e.to_string())?;
                            if let LayerContent::Group(group)=&mut grouped.layers.last_mut().unwrap().content {group.children[0].content=LayerContent::Adjustment(adjustment);}
                            uploads+=presenter.draw(&grouped,&view,true)?;presenter.wait_ready()?;
                        }
                        assert_eq!(uploads,0,"masked pass-through groups must retain source and mask textures");
                        eprintln!("[native probe] masked adjustment group {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
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
                        let mut graded=doc.clone();let mut grade=recipe::ColorGrading::default();grade.midtones.sat=25.0;grade.midtones.hue=40.0;
                        graded.layers.push(Layer::new("Grading",LayerContent::Adjustment(grading::adjustment(&grade)?)));
                        presenter.draw(&graded,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..15{grade.midtones.hue=40.0+n as f32*2.0;graded.layers.last_mut().unwrap().content=LayerContent::Adjustment(grading::adjustment(&grade)?);uploads+=presenter.draw(&graded,&view,true)?;presenter.wait_ready()?;}
                        eprintln!("[native probe] upstream sampled grading {:.2} ms/frame; uploaded bytes={uploads}",started.elapsed().as_secs_f64()*1000.0/15.0);
                        let mut region=recipe::ParametricCurve{highlights:-25.0,lights:20.0,darks:-15.0,shadows:30.0,splits:[25.0,50.0,75.0]};
                        let mut param=doc.clone();param.layers.push(Layer::new("Parametric",LayerContent::Adjustment(parametric::adjustment(&region))));
                        presenter.draw(&param,&view,true)?;presenter.wait_ready()?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..20{region.shadows=30.0+n as f64;param.layers.last_mut().unwrap().content=LayerContent::Adjustment(parametric::adjustment(&region));uploads+=presenter.draw(&param,&view,true)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"parametric curve updates retain source textures");
                        eprintln!("[native probe] upstream parametric curve {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
                        presenter.prepare_comparison(&doc)?;
                        assert_eq!(presenter.prepare_comparison(&doc)?,0,"comparison source remains resident");
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..30{view.compare_split=Some(n as f32/29.0);uploads+=presenter.draw(&param,&view,false)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"split dragging only updates view uniforms");
                        eprintln!("[native probe] original/edited split {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/30.0);
                        view.compare_split=None;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..30{view.clipping=(n%4) as u8;view.compare_split=Some(n as f32/29.0);uploads+=presenter.draw(&param,&view,false)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"clipping switches only update uniforms");
                        eprintln!("[native probe] clipping + comparison {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/30.0);
                        view.clipping=0;
                        // Reuse the upstream-painted mask already validated above.
                        let mut coverage_doc=mask_session.active().unwrap().doc.as_ref().clone();
                        coverage_doc.layers.last_mut().unwrap().content=doc.layers[0].content.clone();
                        presenter.prepare_mask_overlay(&coverage_doc)?;
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..30{view.mask_guide=Some([0.2,0.5,0.6+n as f32/100.0,0.5,0.3,0.4,if n%2==0{1.0}else{2.0},if n%3==0{0.0}else{1.0}]);uploads+=presenter.draw(&param,&view,false)?;presenter.wait_ready()?;}
                        assert_eq!(uploads,0,"gradient guides and overlay switches retain photo and coverage textures");
                        eprintln!("[native probe] mask overlay + gradient guides {:.2} ms/frame; source reupload={uploads}",started.elapsed().as_secs_f64()*1000.0/30.0);
                        let mut gradient_session=photocraft_engine::Session::new();gradient_session.add_document(Document::new("Live gradient",doc.size,ColorMode::Rgb,SampleType::U8),None);
                        gradient_session.execute("gradient.fill.create",serde_json::json!({"from":[400,500],"to":[1200,500],"style":"linear","stops":[[0,"#ffffff"],[1,"#000000"]],"dither":false})).map_err(|e|e.to_string())?;
                        let mut gradient=gradient_session.active().unwrap().doc.as_ref().clone();
                        if let LayerContent::Fill(photocraft_engine::doc::Fill::Gradient{stops,..})=&mut gradient.layers[0].content{for (_,c) in stops{*c=Color::rgba(1.0,0.2,0.2,c.to_rgb()[0]);}}
                        presenter.prepare_mask_overlay(&gradient)?;view.mask_source_space=true;
                        let started=std::time::Instant::now();let mut uploads=0;
                        let gradient_bounds=gradient.bounds();
                        for n in 0..20{if let LayerContent::Fill(photocraft_engine::doc::Fill::Gradient{style,angle,scale,offset,..})=&mut gradient.layers[0].content{let (a,s,o)=photocraft_compose::gradient_fill::from_handles(*style,[400.0,500.0],[1000.0+n as f32*10.0,600.0],gradient_bounds,*angle);*angle=a;*scale=s;*offset=o;}
                            uploads+=presenter.prepare_mask_overlay(&gradient)?;uploads+=presenter.draw(&param,&view,false)?;presenter.wait_ready()?;}
                        eprintln!("[native probe] live gradient coverage {:.2} ms/frame; uploaded bytes={uploads}",started.elapsed().as_secs_f64()*1000.0/20.0);
                        std::sync::Arc::make_mut(&mut mask_session.active_mut().unwrap().doc).layers.last_mut().unwrap().content=doc.layers[0].content.clone();
                        view.mask_source_space=false;view.mask_stroke=Some(serde_json::json!({"points":[{"x":0.3,"y":0.5}],"widthRel":0.1,"hardness":0.5,"opacity":0.2,"flow":0.5,"erase":false}));
                        let started=std::time::Instant::now();let mut uploads=0;
                        for n in 0..8{mask_session.execute("paint.stroke",serde_json::json!({"target":"mask","points":[[400+n*10,500,1],[1100+n*10,600,1]],"size":120,"hardness":0.5,"opacity":0.2,"flow":0.5,"spacing":0.15,"color":"#ffffff"})).map_err(|e|e.to_string())?;
                            let painted=mask_session.active().unwrap().doc.as_ref().clone();uploads+=presenter.prepare_mask_overlay(&painted)?;view.mask_guide=Some([0.3,0.5,(1100+n*10)as f32/1600.0,600.0/1067.0,0.05,0.08,3.0,1.0]);uploads+=presenter.draw(&param,&view,false)?;presenter.wait_ready()?;}
                        eprintln!("[native probe] upstream brush paint + native coverage {:.2} ms/frame; uploaded bytes={uploads}",started.elapsed().as_secs_f64()*1000.0/8.0);
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
