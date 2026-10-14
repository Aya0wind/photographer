use photographer_media::rawlab::{Backend, DevelopOptions};
use std::sync::atomic::AtomicBool;
#[test]
fn bridge_rejects_invalid_input_and_cancellation() {
    let Some(path) = std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE") else {
        return;
    };
    // CI provides our just-built native artifact, not a user-selected library.
    let backend = unsafe { Backend::load(path) }.unwrap();
    assert!(backend
        .develop(
            "missing-source.DNG",
            DevelopOptions::default(),
            &AtomicBool::new(false)
        )
        .is_err());
    assert!(backend
        .develop(
            "missing-source.DNG",
            DevelopOptions::default(),
            &AtomicBool::new(true)
        )
        .is_err());
}
/// Private fixtures stay outside the repository. Run explicitly with both
/// PHOTOGRAPHER_RAW_BRIDGE and PHOTOGRAPHER_RAW_FIXTURE configured.
#[test]
#[ignore = "requires a licensed real RAW fixture and built native bridge"]
fn real_raw_float_lifetime_and_orientation_contract() {
    let backend =
        unsafe { Backend::load(std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE").unwrap()) }.unwrap();
    let path = std::env::var("PHOTOGRAPHER_RAW_FIXTURE").unwrap();
    for half_size in [false, true] {
        let image = backend
            .develop(
                &path,
                DevelopOptions {
                    half_size,
                    ..Default::default()
                },
                &AtomicBool::new(false),
            )
            .unwrap();
        let [width, height] = image.dimensions();
        assert!(width > 0 && height > 0);
        assert_eq!(image.rgb().len(), width as usize * height as usize * 3);
        assert!(image.rgb().iter().all(|v| v.is_finite()));
        assert!(image.baseline_ev().is_finite());
        if !half_size {
            assert_eq!(Some(image.dimensions()), image.source_dimensions());
        }
        // Drop native pixels before repeat decode, while retaining backend.
    }
}

#[test]
fn native_neutral_contract_and_gpu_auto_fallback() {
    use photographer_media::rawlab::{Encoding, GpuMode};
    let Some(path) = std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE") else {
        return;
    };
    let backend = unsafe { Backend::load(path) }.unwrap();
    let cancel = AtomicBool::new(false);
    let rgb = [-0.1, 0.1845, 1.0, 2.0, 3.0, 4.0];
    let cpu = backend
        .render(
            &rgb,
            [2, 1],
            Some("missing-film.cube"),
            0.0,
            GpuMode::Off,
            &cancel,
        )
        .unwrap();
    assert_eq!(cpu.encoding(), Encoding::DisplaySrgb);
    assert!(!cpu.gpu_used());
    assert_eq!(cpu.rgb()[0], 0.0);
    assert!((cpu.rgb()[1] - 0.4664).abs() < 0.001);
    assert!(cpu.rgb()[5] > cpu.rgb()[4] && cpu.rgb()[4] > cpu.rgb()[3]);
    let auto = backend
        .render(&rgb, [2, 1], None, 0.0, GpuMode::Auto, &cancel)
        .unwrap();
    for (reference, actual) in cpu.rgb().iter().zip(auto.rgb()) {
        assert!((reference - actual).abs() < 0.002);
    }
    if cfg!(target_os = "linux") {
        assert!(!auto.gpu_used());
        assert!(backend
            .render(&rgb, [2, 1], None, 0.0, GpuMode::Force, &cancel)
            .is_err());
    }
    assert!(backend
        .render(&rgb, [1, 1], None, 0.0, GpuMode::Off, &cancel)
        .is_err());
    assert!(backend
        .render(&rgb, [2, 1], None, f32::NAN, GpuMode::Off, &cancel)
        .is_err());
    assert!(backend
        .render(
            &rgb,
            [2, 1],
            None,
            0.0,
            GpuMode::Off,
            &AtomicBool::new(true)
        )
        .is_err());
}

#[test]
fn core_film_strength_and_log_input_are_shared_with_gpu_auto() {
    use photographer_media::rawlab::GpuMode;
    use std::io::Write;
    let Some(library) = std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE") else {
        return;
    };
    let backend = unsafe { Backend::load(library) }.unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "photographer-film-{}-{nonce}.cube",
        std::process::id()
    ));
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Remove(path.clone());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    // Original synthetic identity fixture, not a redistributed Fuji LUT.
    writeln!(
        file,
        "# Gamma: FLog2 to display\n# Gamut: F-Gamut to ITU-R BT.709\nLUT_3D_SIZE 2"
    )
    .unwrap();
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                writeln!(file, "{r} {g} {b}").unwrap();
            }
        }
    }
    drop(file);
    let path = path.to_str().unwrap();
    let info = backend.lut_info(path).unwrap();
    assert_eq!(info.size, 2);
    assert_ne!(info.input_transfer, 0);
    assert_ne!(info.output_transfer, 0);
    let cancel = AtomicBool::new(false);
    let rgb = [0.18; 3];
    let neutral = backend
        .render(&rgb, [1, 1], Some(path), 0.0, GpuMode::Off, &cancel)
        .unwrap();
    let full = backend
        .render(&rgb, [1, 1], Some(path), 1.0, GpuMode::Off, &cancel)
        .unwrap();
    let half = backend
        .render(&rgb, [1, 1], Some(path), 0.5, GpuMode::Off, &cancel)
        .unwrap();
    let auto = backend
        .render(&rgb, [1, 1], Some(path), 1.0, GpuMode::Auto, &cancel)
        .unwrap();
    for i in 0..3 {
        assert!((full.rgb()[i] - 400.0 / 1023.0).abs() < 0.001); // F-Log2 gray anchor
        assert!((half.rgb()[i] - (neutral.rgb()[i] + full.rgb()[i]) * 0.5).abs() < 1e-6);
        assert!((auto.rgb()[i] - full.rgb()[i]).abs() < 0.002);
    }
}

#[test]
fn reusable_session_preserves_source_and_recovers_after_cancel() {
    use photographer_media::rawlab::{ColorOptions, GpuMode};
    let Some(library) = std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE") else {
        return;
    };
    let backend = unsafe { Backend::load(library) }.unwrap();
    let cancel = AtomicBool::new(false);
    let source = [0.1, 0.2, 0.3, 0.4, 0.5, 2.0];
    let mut session = backend.color_session(&source, [2, 1]).unwrap();
    let neutral = session
        .render(
            ColorOptions {
                gpu: GpuMode::Off,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
    let gains = [1.2, 1.0, 0.8];
    let adjusted = session
        .render(
            ColorOptions {
                gpu: GpuMode::Off,
                exposure_ev: 1.0,
                relative_white_balance: gains,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
    let expected_source: Vec<_> = source
        .iter()
        .enumerate()
        .map(|(i, v)| v * 2.0 * gains[i % 3])
        .collect();
    let expected = backend
        .render(&expected_source, [2, 1], None, 0.0, GpuMode::Off, &cancel)
        .unwrap();
    for (a, b) in adjusted.rgb().iter().zip(expected.rgb()) {
        assert!((a - b).abs() < 1e-6);
    }
    assert!(session
        .render(ColorOptions::default(), &AtomicBool::new(true))
        .is_err());
    assert!(session
        .render(
            ColorOptions {
                relative_white_balance: [0.0; 3],
                ..Default::default()
            },
            &cancel
        )
        .is_err());
    // A failed request and an exposure edit must not mutate the cached source.
    let restored = session
        .render(
            ColorOptions {
                gpu: GpuMode::Auto,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
    for (a, b) in neutral.rgb().iter().zip(restored.rgb()) {
        assert!((a - b).abs() < 0.002);
    }
    drop(session);
    // Native output owns its own storage, independent of the device/source lease.
    assert_eq!(neutral.rgb().len(), 6);
    assert!(backend.color_session(&[f32::NAN; 3], [1, 1]).is_err());
    assert!(backend.color_session(&source, [1, 1]).is_err());
    assert!(backend.lut_info("missing-film.cube").is_err());
}
