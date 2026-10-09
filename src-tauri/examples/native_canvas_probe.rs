//! Hidden, disposable hardware validation of the native presentation path.
//! No library/photos/settings are opened or changed.
#[cfg(windows)]
mod native_canvas {
    pub struct View {
        pub rect: [i32; 4],
        pub uv: [f32; 4],
    }
}
#[cfg(windows)]
#[path = "../src/edit/native_presenter.rs"]
mod native_presenter;

#[cfg(windows)]
fn main() {
    use std::sync::{Arc,atomic::{AtomicI32,Ordering}};
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let status=Arc::new(AtomicI32::new(1));
    let worker_status=status.clone();
    tauri::Builder::default()
        .setup(move |app| {
            let window = tauri::WindowBuilder::new(app, "native-canvas-probe")
                .title("Photo Hub GPU validation")
                .inner_size(640.0, 400.0)
                .visible(false)
                .skip_taskbar(true)
                .decorations(false)
                .build()?;
            let handle = app.handle().clone();
            let worker_status=worker_status.clone();
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
                        let view = native_canvas::View {
                            rect: [0, 0, 640, 400],
                            uv: [0.0, 0.0, 1.0, 1.0],
                        };
                        let first = std::time::Instant::now();
                        let uploaded = presenter.draw(&doc, &view, true)?;
                        eprintln!(
                            "[native probe] first submit {:.2} ms, uploaded {uploaded} bytes",
                            first.elapsed().as_secs_f64() * 1000.0
                        );
                        let warm = std::time::Instant::now();
                        for _ in 0..10 {
                            presenter.draw(&doc, &view, false)?;
                        }
                        eprintln!(
                            "[native probe] cached submit average {:.2} ms; readback=0; JPEG=0",
                            warm.elapsed().as_secs_f64() * 1000.0 / 10.0
                        );
                        Ok(())
                    },
                ));
                let success = matches!(result, Ok(Ok(())));
                if !success {
                    eprintln!("[native probe] failed: {result:?}");
                }
                worker_status.store(if success {0} else {1},Ordering::Release);
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
