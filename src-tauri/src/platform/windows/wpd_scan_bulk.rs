// Included in wpd::com. Only plain Rust data crosses callback threads.
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use windows::core::{implement, Interface, Ref};
use windows::Win32::Devices::PortableDevices::{
    IPortableDevicePropertiesBulk, IPortableDevicePropertiesBulkCallback,
    IPortableDevicePropertiesBulkCallback_Impl, IPortableDeviceValuesCollection, WPD_OBJECT_ID,
};

#[derive(Default, Clone)]
struct ObjectData {
    content_type: Option<GUID>,
    original_name: Option<String>,
    name: Option<String>,
    persistent_id: Option<String>,
    size: Option<u64>,
    mtime: Option<DateTime<Utc>>,
}
impl ObjectData {
    fn merge(&mut self, other: Self) {
        if other.content_type.is_some() {
            self.content_type = other.content_type;
        }
        if other.original_name.is_some() {
            self.original_name = other.original_name;
        }
        if other.name.is_some() {
            self.name = other.name;
        }
        if other.persistent_id.is_some() {
            self.persistent_id = other.persistent_id;
        }
        if other.size.is_some() {
            self.size = other.size;
        }
        if other.mtime.is_some() {
            self.mtime = other.mtime;
        }
    }
}
fn object_data(values: &IPortableDeviceValues) -> ObjectData {
    ObjectData {
        content_type: unsafe { values.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE) }.ok(),
        original_name: string_prop(values, &WPD_OBJECT_ORIGINAL_FILE_NAME),
        name: string_prop(values, &WPD_OBJECT_NAME),
        persistent_id: string_prop(values, &WPD_OBJECT_PERSISTENT_UNIQUE_ID),
        size: unsafe { values.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE) }.ok(),
        mtime: date_prop(values, &WPD_OBJECT_DATE_MODIFIED),
    }
}
enum BulkMessage {
    Values(Vec<(String, ObjectData)>),
    End(HRESULT),
}
#[implement(IPortableDevicePropertiesBulkCallback)]
struct BulkCallback {
    send: Sender<BulkMessage>,
}
impl IPortableDevicePropertiesBulkCallback_Impl for BulkCallback_Impl {
    fn OnStart(&self, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnProgress(
        &self,
        _: *const GUID,
        results: Ref<'_, IPortableDeviceValuesCollection>,
    ) -> windows::core::Result<()> {
        let Some(results) = results.as_ref() else {
            return Ok(());
        };
        let mut count: u32 = 0;
        unsafe { results.GetCount(&mut count) }?;
        let mut batch = Vec::new();
        for index in 0..count {
            let values = unsafe { results.GetAt(index) }?;
            if let Some(id) = string_prop(&values, &WPD_OBJECT_ID) {
                batch.push((id, object_data(&values)));
            }
        }
        self.send
            .send(BulkMessage::Values(batch))
            .map_err(|_| windows::core::Error::from_hresult(HRESULT(0x80004004u32 as i32)))
    }
    fn OnEnd(&self, _: *const GUID, status: HRESULT) -> windows::core::Result<()> {
        let _ = self.send.send(BulkMessage::End(status));
        Ok(())
    }
}
fn bulk_unsupported(code: HRESULT) -> bool {
    matches!(code.0 as u32, 0x80004001 | 0x80004002 | 0x80070032)
}

pub(super) fn file_sizes(pnp: &str, ids: &[String]) -> DeviceResult<Option<Vec<(String, u64)>>> {
    let _apartment = ComApartment::init().map_err(win_error)?;
    let device = open_device(pnp)?;
    let content = unsafe { device.Content() }.map_err(win_error)?;
    let keys: IPortableDeviceKeyCollection = unsafe { CoCreateInstance(
        &CLSID_PORTABLE_DEVICE_KEY_COLLECTION, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER
    ) }.map_err(win_error)?;
    unsafe { keys.Add(&WPD_OBJECT_SIZE) }.map_err(win_error)?;
    Ok(read_properties_batch(&content, &keys, ids)?.and_then(|values| {
        let sizes: Vec<_> = values.into_iter().filter_map(|(id, data)| data.size.map(|size| (id, size))).collect();
        if sizes.is_empty() && !ids.is_empty() { None } else { Some(sizes) }
    }))
}
fn read_properties_batch(
    content: &IPortableDeviceContent,
    keys: &IPortableDeviceKeyCollection,
    ids: &[String],
) -> DeviceResult<Option<HashMap<String, ObjectData>>> {
    let properties = unsafe { content.Properties() }.map_err(win_error)?;
    let bulk = match properties.cast::<IPortableDevicePropertiesBulk>() {
        Ok(bulk) => bulk,
        Err(error) if bulk_unsupported(error.code()) => return Ok(None),
        Err(error) => return Err(win_error(error)),
    };
    let request: IPortableDevicePropVariantCollection = unsafe {
        CoCreateInstance(
            &CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(win_error)?;
    for id in ids {
        let mut value = lpstr_propvariant(id)
            .ok_or_else(|| DeviceError::Other("构造批量对象集合失败".into()))?;
        let added = unsafe { request.Add(&value) };
        let _ = unsafe { PropVariantClear(&mut value) };
        added.map_err(win_error)?;
    }
    let (send, receive) = mpsc::channel();
    let callback: IPortableDevicePropertiesBulkCallback = BulkCallback { send }.into();
    let context = match unsafe { bulk.QueueGetValuesByObjectList(&request, keys, &callback) } {
        Ok(context) => context,
        Err(error) if bulk_unsupported(error.code()) => return Ok(None),
        Err(error) => return Err(win_error(error)),
    };
    if let Err(error) = unsafe { bulk.Start(&context) } {
        let _ = unsafe { bulk.Cancel(&context) };
        if bulk_unsupported(error.code()) {
            return Ok(None);
        }
        return Err(win_error(error));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut values = HashMap::<String, ObjectData>::new();
    let result = loop {
        if let Err(error) = super::super::timed_worker::checkpoint() {
            break Err(error);
        }
        if std::time::Instant::now() >= deadline {
            break Err(DeviceError::Other("批量读取相机属性超时".into()));
        }
        match receive.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(BulkMessage::Values(batch)) => {
                for (id, data) in batch {
                    // Callbacks can arrive out of order and split one object's fields.
                    if ids.contains(&id) {
                        values.entry(id).or_default().merge(data);
                    }
                }
            }
            Ok(BulkMessage::End(status)) if status.is_ok() => break Ok(Some(values)),
            Ok(BulkMessage::End(status)) if bulk_unsupported(status) => break Ok(None),
            Ok(BulkMessage::End(status)) => break Err(hr_error(status)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => break Err(DeviceError::Other("批量属性读取意外结束".into())),
        }
    };
    if result.is_err() {
        let _ = unsafe { bulk.Cancel(&context) };
    }
    result
}

#[cfg(test)]
mod bulk_scan_tests {
    use super::*;
    #[test]
    fn unsupported_bulk_falls_back_but_transport_failures_do_not() {
        assert!(bulk_unsupported(HRESULT(0x80004002u32 as i32)));
        assert!(bulk_unsupported(HRESULT(0x80070032u32 as i32)));
        assert!(!bulk_unsupported(HRESULT(0x8007048fu32 as i32)));
        assert!(!bulk_unsupported(HRESULT(0x80070005u32 as i32)));
    }

    #[test]
    fn fallback_property_keys_do_not_request_sizes_or_dates() {
        let _apartment = ComApartment::init().unwrap();
        let keys = build_property_keys(true).unwrap();
        let mut count: u32 = 0;
        unsafe { keys.GetCount(&mut count as *mut u32 as *const u32) }.unwrap();
        assert_eq!(count, 3);
        for i in 0..3 {
            let mut key = PROPERTYKEY::default();
            unsafe { keys.GetAt(i, &mut key as *mut PROPERTYKEY as *const PROPERTYKEY) }.unwrap();
            assert!(key != WPD_OBJECT_SIZE && key != WPD_OBJECT_DATE_MODIFIED);
        }
    }
    #[test]
    fn split_property_results_preserve_fields_and_valid_zero_sizes() {
        let mut data = ObjectData {
            name: Some("A.JPG".into()),
            size: Some(0),
            ..Default::default()
        };
        data.merge(ObjectData {
            mtime: Some(Utc::now()),
            ..Default::default()
        });
        assert_eq!(data.name.as_deref(), Some("A.JPG"));
        assert_eq!(data.size, Some(0));
        assert!(data.mtime.is_some());
    }

    #[test]
    #[ignore = "requires an exclusively connected Sony in MTP mode; enumerates metadata only"]
    fn live_sony_bulk_scan() {
        let cameras = enumerate().unwrap();
        let (id, name) = cameras
            .into_iter()
            .find(|(_, name)| name.contains("ILCE"))
            .expect("Sony MTP camera connected");
        let started = std::time::Instant::now();
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback: super::super::super::FileBatchCallback = std::sync::Arc::new(move |files| {
            let total =
                count.fetch_add(files.len(), std::sync::atomic::Ordering::Relaxed) + files.len();
            if total % 500 < files.len() {
                eprintln!(
                    "scan progress: {total} files elapsed={:?}",
                    started.elapsed()
                );
            }
        });
        let files = list(&id, Some(callback), true).unwrap();
        eprintln!("{name}: {} files in {:?}", files.len(), started.elapsed());
        assert!(!files.is_empty());
    }

    #[test]
    #[ignore = "requires Sony MTP camera; gets counts without enumerating filenames or properties"]
    fn live_sony_fast_count() {
        use crate::devices::DeviceSource;
        let (id, name) = enumerate().unwrap().into_iter().find(|(_, name)| name.contains("ILCE")).unwrap();
        let source = super::super::WpdSource::new(id, name);
        let now = std::time::Instant::now();
        let counts = source.fast_file_counts().unwrap().expect("fast counts supported");
        eprintln!("Fast discovery counts={counts:?} elapsed={:?}", now.elapsed());
        assert!(counts.values().sum::<u64>() > 0);
    }

    #[test]
    #[ignore = "requires exclusive Sony MTP camera; validates preview access while discovery is active"]
    fn live_preview_during_discovery() {
        use crate::devices::DeviceSource;
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        let (id, name) = enumerate().unwrap().into_iter().find(|(_, name)| name.contains("ILCE")).unwrap();
        let source = Arc::new(super::super::WpdSource::new(id, name));
        let (send, receive) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let resume = std::sync::Mutex::new(resume);
        let sent = AtomicBool::new(false);
        let callback: super::super::super::FileBatchCallback = Arc::new(move |files| {
            if !files.is_empty() && !sent.swap(true, Ordering::SeqCst) {
                send.send(files[0].id.clone()).unwrap();
                resume.lock().unwrap().recv_timeout(std::time::Duration::from_secs(15)).unwrap();
            }
        });
        let scan_source = source.clone();
        let scan = std::thread::spawn(move || scan_source.list_names_with_progress(callback));
        let id = receive.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        let start = std::time::Instant::now();
        let preview = source.thumbnail(&id);
        let elapsed = start.elapsed();
        let _ = release.send(());
        eprintln!("Preview during paused discovery: result={:?} elapsed={elapsed:?}", preview.as_ref().map(|data| data.as_ref().map(Vec::len)));
        let result = scan.join().unwrap();
        assert!(elapsed < std::time::Duration::from_secs(10));
        preview.unwrap();
        result.unwrap();
    }
}
