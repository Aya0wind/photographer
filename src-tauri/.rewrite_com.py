# -*- coding: utf-8 -*-
"""com 模块改造 v2：stream_channel + 泵线程；移除 MtpStream/ComSend/drop_on_mta。"""
import io

p = 'src/devices/wpd.rs'
s = io.open(p, encoding='utf-8').read()
orig = s

# 1) ComApartment → pub(super)
a = '    struct ComApartment {\n        owned: bool,\n    }\n\n    impl ComApartment {\n        fn init()'
b = '    pub(super) struct ComApartment {\n        owned: bool,\n    }\n\n    impl ComApartment {\n        pub(super) fn init()'
assert a in s, 'anchor 1'
s = s.replace(a, b, 1)

# 2) 移除 com::drop_on_mta
start = s.index('    /// 在专用 MTA 线程释放可能持有 COM 接口的值')
end = s.index('    fn to_wide(s: &str) -> Vec<u16> {', start)
s = s[:start] + s[end:]

# 3) 旧 stream() → stream_channel()
sstart = s.index('    pub fn stream(pnp_id: &str, obj_id: &str)')
send = s.index('    /// 删除对象（move 模式删源）', sstart)
new_stream = '''    pub fn stream_channel(
        pnp_id: &str,
        obj_id: &str,
    ) -> DeviceResult<tokio::sync::mpsc::Receiver<super::StreamChunk>> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);
        let (stream, _) = open_resource(&content, &resolved).map_err(win_error)?;

        let (tx, rx) = tokio::sync::mpsc::channel::<super::StreamChunk>(STREAM_BUFFER_CHUNKS);
        // 泵线程：会话与流移交本线程（MTA 初始化；in-proc 对象跨 MTA 线程
        // 合法）——分块泵入有界通道（背压：reader 消费慢/暂停时阻塞在此，
        // 不占 worker）；reader drop → 通道关闭 → 循环退出 → 在本线程释放
        // 全部 COM（WPD_RELEASE_COUNT 递增）。COM 生命周期永不落在调用方
        // 线程（断开闪退整类消灭）。
        std::thread::Builder::new()
            .name("wpd-stream".into())
            .spawn(move || {
                let _com = ComApartment::init();
                let _device = device; // 保活：流关闭前不 Close 设备
                let mut stream = stream;
                let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];
                loop {
                    let mut got = 0u32;
                    // SAFETY: chunk 可写长度 ≥ 请求长度
                    let hr = unsafe {
                        stream.Read(
                            chunk.as_mut_ptr() as *mut core::ffi::c_void,
                            chunk.len() as u32,
                            Some(&mut got),
                        )
                    };
                    if hr.is_err() {
                        let _ = tx.blocking_send(Err(hr_to_io(hr)));
                        break;
                    }
                    if got == 0 {
                        break; // EOF：tx 随作用域 drop → reader 见 Ok(0)
                    }
                    if tx
                        .blocking_send(Ok(chunk[..got as usize].to_vec()))
                        .is_err()
                    {
                        break; // reader 已 drop（取消/完成）：就此收尾释放
                    }
                }
                super::WPD_RELEASE_COUNT.fetch_add(1, Ordering::SeqCst);
            })
            .map_err(|e| DeviceError::Other(format!("流泵线程启动失败: {e}")))?;
        Ok(rx)
    }

    /// HRESULT → io::Error（读取中断映射 ConnectionAborted：调用方据此
    /// 判定拔线并暂停任务）。
    fn hr_to_io(hr: HRESULT) -> std::io::Error {
        let kind = match hr_error(hr) {
            DeviceError::AccessDenied => std::io::ErrorKind::PermissionDenied,
            DeviceError::Disconnected => std::io::ErrorKind::ConnectionAborted,
            _ => std::io::ErrorKind::Other,
        };
        std::io::Error::new(kind, format!("MTP stream error 0x{:08X}", hr.0 as u32))
    }

'''
s = s[:sstart] + new_stream + s[send:]

# 4) 移除 MtpStream/ComSend/Send/Drop/Read 全块（delete 之后到 com 模块闭合）
mstart = s.index('    /// IStream')
# com 模块以文件中最后一个 "    }\n}" 结束（Read impl 尾 + mod 闭）
mend = s.rindex('            Ok(got as usize)\n        }\n    }\n}') + len('            Ok(got as usize)\n        }\n    }\n}')
s = s[:mstart] + '}\n' + s[mend + 1:]  # 保留 com 模块闭合 "}"

# 5) 常量
anchor = '    /// open_head 的读块大小。\n    const HEAD_CHUNK: usize = 64 * 1024;'
assert anchor in s, 'anchor consts'
s = s.replace(anchor, anchor + '''
    /// 流泵分块大小（1MB：内存可控，导入侧 8MB 读循环多次接收）。
    const STREAM_CHUNK_BYTES: usize = 1024 * 1024;
    /// 流泵有界通道缓冲块数（8MB 在途；背压阻塞泵线程而非 worker）。
    const STREAM_BUFFER_CHUNKS: usize = 8;''', 1)

assert s != orig
io.open(p, 'w', encoding='utf-8', newline='\n').write(s)
print('com reworked ok; MtpStream gone:', 'MtpStream' not in s, '; drop_on_mta gone:', 'drop_on_mta' not in s)
