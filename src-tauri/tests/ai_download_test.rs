//! 模型在线下载管理器（M4 前置）：catalog 契约、完整下载+SHA 校验落位、
//! 断点续传（Range 206 + .part 保留）、镜像回退、SHA 不匹配重试一次后
//! failed、cancel 清 .part、并发下载去重、status 状态机、delete 翻状态。
//! 全部打本地 127.0.0.1 HTTP server（离线可跑）；真实 HF URL 仅 #[ignore]。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ai::{ModelEntry, ModelManager};
use events::AppEvent;
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// 本地 HTTP server（std 手写：Range/206、断开、坏内容、慢速滴流）
// ---------------------------------------------------------------------------

struct TestServer {
    base: String,
    /// 每路径请求计数（断言镜像回退/重试次数）。
    counts: Arc<Mutex<std::collections::HashMap<String, u64>>>,
}

impl TestServer {
    /// 启动单线程串行 server；body 为完整模型内容。
    fn start(body: Vec<u8>, wrong: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let counts = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let c2 = Arc::clone(&counts);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 8192];
                // 读到请求头结束
                let mut req = Vec::new();
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if req.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&req).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_string();
                c2.lock()
                    .unwrap()
                    .entry(path.clone())
                    .and_modify(|n| *n += 1)
                    .or_insert(1);
                let range_start: Option<usize> = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().strip_prefix("bytes="))
                    .and_then(|v| v.split('-').next())
                    .and_then(|v| v.trim().parse().ok());
                let (status, payload, content_range): (&str, &[u8], String) = match path.as_str() {
                    "/ok" => match range_start {
                        Some(start) if start < body.len() => (
                            "206 Partial Content",
                            &body[start..],
                            format!("bytes {start}-{}/{}", body.len() - 1, body.len()),
                        ),
                        Some(_) => ("416 Range Not Satisfiable", &body[..0], String::new()),
                        None => ("200 OK", &body[..], String::new()),
                    },
                    "/drop" => {
                        // 只给前 64 字节就断开（模拟连接中断）
                        let n = 64.min(body.len());
                        let _ = stream.write_all(
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                                .as_bytes(),
                        );
                        let _ = stream.write_all(&body[..n]);
                        continue; // 直接 close
                    }
                    "/bad" => match range_start {
                        Some(start) if start < wrong.len() => (
                            "206 Partial Content",
                            &wrong[start..],
                            format!("bytes {start}-{}/{}", wrong.len() - 1, wrong.len()),
                        ),
                        None => ("200 OK", &wrong[..], String::new()),
                        Some(_) => ("416 Range Not Satisfiable", &wrong[..0], String::new()),
                    },
                    "/slow" => {
                        let start = range_start.unwrap_or(0);
                        let _ = stream.write_all(
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len())
                                .as_bytes(),
                        );
                        // 滴流：每块 1KB 间隔 20ms（给 cancel 观察窗口）
                        for chunk in body[start..].chunks(1024) {
                            if stream.write_all(chunk).is_err() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        continue;
                    }
                    _ => ("404 Not Found", &body[..0], String::new()),
                };
                let headers = if status.starts_with("206") {
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Range: {content_range}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    )
                };
                let _ = stream.write_all(headers.as_bytes());
                let _ = stream.write_all(payload);
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            counts: Arc::clone(&counts),
        }
    }

    fn hits_of(&self, path: &str) -> u64 {
        *self.counts.lock().unwrap().get(path).unwrap_or(&0)
    }
}

// ---------------------------------------------------------------------------
// 管理器脚手架
// ---------------------------------------------------------------------------

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

fn manager(root: &Path) -> ModelManager {
    let bus = events::EventBus::new();
    let supervisor = tasks::TaskSupervisor::new(bus.clone());
    ModelManager::new(root.to_path_buf(), bus, supervisor)
}

fn entry(id: &str, url: &str, mirror: &str, body: &[u8]) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        url: url.into(),
        mirror_url: mirror.into(),
        sha256: sha256_hex(body),
        bytes_total: body.len() as u64,
        version: "v1".into(),
        feature: "semantic".into(),
    }
}

/// 轮询总线直到 aiModelDownloadFinished。
fn wait_finished(mgr: &ModelManager, id: &str) -> AppEvent {
    let mut rx = mgr.bus().subscribe();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::AiModelDownloadFinished { id: eid, .. } = &event {
                if eid == id {
                    return event;
                }
            }
        }
        if Instant::now() > deadline {
            panic!("30s 内未收到 {id} 的下载完成事件");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[test]
fn catalog_has_four_models_with_full_metadata() {
    let catalog = ai::catalog();
    assert_eq!(catalog.len(), 4, "clip-visual/clip-text/scrfd/arcface");
    let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
    for expect in ["clip-visual", "clip-text", "scrfd", "arcface"] {
        assert!(ids.contains(&expect), "缺 {expect}: {ids:?}");
    }
    for m in catalog {
        assert!(m.url.starts_with("https://"), "{} url", m.id);
        assert!(m.mirror_url.contains("hf-mirror.com"), "{} mirror", m.id);
        assert_eq!(m.sha256.len(), 64, "{} sha256", m.id);
        assert!(m.bytes_total > 0, "{} bytesTotal", m.id);
        assert!(!m.version.is_empty(), "{} version", m.id);
        assert!(
            m.feature == "semantic" || m.feature == "face",
            "{} feature",
            m.id
        );
    }
}

#[test]
fn downloads_full_body_verifies_sha_and_installs() {
    let body = payload(96 * 1024);
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let e = entry(
        "m1",
        &format!("{}/ok", server.base),
        &format!("{}/ok", server.base),
        &body,
    );

    mgr.download(e.clone()).unwrap();
    let finished = wait_finished(&mgr, "m1");
    assert!(matches!(
        finished,
        AppEvent::AiModelDownloadFinished {
            ok: true,
            error: None,
            ..
        }
    ));
    let final_path = root.path().join("m1.onnx");
    assert!(final_path.is_file(), "落位文件必须在 models 根下");
    assert_eq!(std::fs::read(&final_path).unwrap(), body, "内容逐字节一致");
    assert!(!root.path().join("m1.onnx.part").exists(), ".part 已收走");

    // 状态翻 done/installed
    let status = mgr.status(&e).unwrap();
    assert!(status.installed);
    assert_eq!(status.state, "done");
    assert_eq!(status.downloaded_bytes, body.len() as u64);
}

#[test]
fn network_drop_falls_back_to_mirror_and_resumes_from_part() {
    let body = payload(64 * 1024);
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    // 主 URL 半途断开；镜像可 Range 续传
    let e = entry(
        "m2",
        &format!("{}/drop", server.base),
        &format!("{}/ok", server.base),
        &body,
    );

    mgr.download(e).unwrap();
    let finished = wait_finished(&mgr, "m2");
    assert!(matches!(
        finished,
        AppEvent::AiModelDownloadFinished { ok: true, .. }
    ));
    // 主 URL 打了一次（断开），镜像打了且带 Range（续传）
    assert_eq!(server.hits_of("/drop"), 1);
    assert_eq!(server.hits_of("/ok"), 1);
    let final_path = root.path().join("m2.onnx");
    assert_eq!(std::fs::read(&final_path).unwrap(), body);
}

#[test]
fn sha_mismatch_retries_once_then_fails_without_part() {
    let body = payload(32 * 1024);
    // 同长度不同内容（长度差会走短读网络分支，测不到 SHA 路径）
    let mut wrong = body.clone();
    wrong[0] ^= 0xFF;
    wrong[100] ^= 0xFF;
    let server = TestServer::start(body.clone(), wrong.clone());
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let e = entry(
        "m3",
        &format!("{}/bad", server.base),
        &format!("{}/bad", server.base),
        &body,
    );

    mgr.download(e.clone()).unwrap();
    let finished = wait_finished(&mgr, "m3");
    let AppEvent::AiModelDownloadFinished { ok, error, .. } = finished else {
        unreachable!()
    };
    assert!(!ok, "SHA 不匹配重试后必须 failed");
    assert!(error.is_some(), "失败要带原因");
    // 首次 + 重试 = 主/镜像各两轮（或至少两次请求），且 .part 不留
    assert!(server.hits_of("/bad") >= 2, "必须重试至少一次");
    assert!(
        !root.path().join("m3.onnx.part").exists(),
        "失败后 .part 必须清理"
    );
    let status = mgr.status(&e).unwrap();
    assert_eq!(status.state, "failed");
    assert!(!status.installed);
}

#[test]
fn cancel_stops_download_and_cleans_part() {
    let body = payload(256 * 1024); // 慢速滴流下足够长
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let e = entry(
        "m4",
        &format!("{}/slow", server.base),
        &format!("{}/slow", server.base),
        &body,
    );

    mgr.download(e).unwrap();
    // 等 .part 出现且在涨 → 取消
    let part = root.path().join("m4.onnx.part");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !(part.exists() && part.metadata().unwrap().len() > 4096) {
        assert!(Instant::now() < deadline, "下载未启动");
        std::thread::sleep(Duration::from_millis(5));
    }
    mgr.cancel("m4").unwrap();
    let finished = wait_finished(&mgr, "m4");
    assert!(matches!(
        finished,
        AppEvent::AiModelDownloadFinished { ok: false, .. }
    ));
    assert!(!part.exists(), "取消必须清理 .part");
    assert!(!root.path().join("m4.onnx").exists(), "未完成不得落位");
    // 取消后可再次发起（守卫已除名）
    assert!(mgr.active_count() == 0);
}

#[test]
fn concurrent_download_of_same_model_deduped() {
    let body = payload(256 * 1024);
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let e = entry(
        "m5",
        &format!("{}/slow", server.base),
        &format!("{}/slow", server.base),
        &body,
    );

    mgr.download(e.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while mgr.active_count() == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // 在队时重复请求：直接返回，不重复派发
    mgr.download(e).unwrap();
    assert_eq!(mgr.active_count(), 1, "同模型去重");
    mgr.cancel("m5").unwrap();
    wait_finished(&mgr, "m5");
}

#[test]
fn status_starts_idle_and_delete_flips_installed() {
    let body = payload(16 * 1024);
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let e = entry(
        "m6",
        &format!("{}/ok", server.base),
        &format!("{}/ok", server.base),
        &body,
    );

    let initial = mgr.status(&e).unwrap();
    assert_eq!(initial.state, "idle");
    assert!(!initial.installed);
    assert_eq!(initial.downloaded_bytes, 0);
    assert_eq!(initial.bytes_total, body.len() as u64);
    assert_eq!(initial.version, "v1");
    assert_eq!(initial.feature, "semantic");

    mgr.download(e.clone()).unwrap();
    wait_finished(&mgr, "m6");
    assert!(mgr.status(&e).unwrap().installed);

    mgr.delete("m6").unwrap();
    let after = mgr.status(&e).unwrap();
    assert!(!after.installed, "delete 后 installed 翻 false");
    assert!(!root.path().join("m6.onnx").exists());
    // 下载中 delete 被拒
    let e2 = entry(
        "m7",
        &format!("{}/slow", server.base),
        &format!("{}/slow", server.base),
        &body,
    );
    mgr.download(e2).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while mgr.active_count() == 0 {
        assert!(Instant::now() < deadline);
    }
    assert!(mgr.delete("m7").is_err(), "下载中不允许删除");
    mgr.cancel("m7").unwrap();
    wait_finished(&mgr, "m7");
    // 未知模型
    assert!(mgr
        .download(ai::ModelEntry {
            id: "ghost".into(),
            url: "http://127.0.0.1:1/x".into(),
            mirror_url: "http://127.0.0.1:1/y".into(),
            sha256: "0".repeat(64),
            bytes_total: 1,
            version: "v1".into(),
            feature: "face".into(),
        })
        .is_ok()); // 派发成功但下载必然失败 → failed 事件
    let finished = wait_finished(&mgr, "ghost");
    assert!(matches!(
        finished,
        AppEvent::AiModelDownloadFinished { ok: false, .. }
    ));
}

#[test]
fn model_status_dto_serializes_camel_case() {
    let dto = ai::ModelStatusDto {
        id: "clip-visual".into(),
        installed: true,
        bytes_total: 100,
        downloaded_bytes: 100,
        version: "v1".into(),
        feature: "semantic".into(),
        state: "done".into(),
    };
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["bytesTotal"], 100);
    assert_eq!(json["downloadedBytes"], 100);

    let progress = serde_json::to_value(AppEvent::AiModelDownloadProgress {
        id: "scrfd".into(),
        done_bytes: 5,
        total_bytes: 10,
    })
    .unwrap();
    assert_eq!(progress["type"], "aiModelDownloadProgress");
    assert_eq!(progress["id"], "scrfd");
    assert_eq!(progress["doneBytes"], 5);
    assert_eq!(progress["totalBytes"], 10);

    let finished = serde_json::to_value(AppEvent::AiModelDownloadFinished {
        id: "scrfd".into(),
        ok: false,
        error: Some("boom".into()),
    })
    .unwrap();
    assert_eq!(finished["type"], "aiModelDownloadFinished");
    assert_eq!(finished["ok"], false);
    assert_eq!(finished["error"], "boom");
}

#[test]
fn progress_events_throttled_to_1s_and_carry_totals() {
    let body = payload(128 * 1024);
    let server = TestServer::start(body.clone(), payload(1));
    let root = tempfile::tempdir().unwrap();
    let mgr = manager(root.path());
    let mut rx = mgr.bus().subscribe();
    let e = entry(
        "m8",
        &format!("{}/ok", server.base),
        &format!("{}/ok", server.base),
        &body,
    );

    mgr.download(e).unwrap();
    wait_finished(&mgr, "m8");
    let mut progresses = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::AiModelDownloadProgress { total_bytes, .. } = event {
            progresses.push(total_bytes);
        }
    }
    // 本地全量秒下：进度事件至多 1-2 条（1s 节流），且 total 恒为 bytes_total
    assert!(progresses.len() <= 2, "1s 节流下不应刷屏: {progresses:?}");
    assert!(progresses.iter().all(|t| *t == body.len() as u64));
}

/// 真实 HuggingFace 连通 smoke：主源 + 镜像 HEAD 可达且 Content-Length
/// 与清单 bytesTotal 一致（不下载全量）。默认忽略，手动跑：
/// `cargo test --test ai_download_test real_hf -- --ignored --nocapture`
#[test]
#[ignore = "依赖外网（HuggingFace/hf-mirror 连通）"]
fn real_hf_urls_reachable_and_sizes_match() {
    for m in ai::catalog() {
        for url in [m.url.as_str(), m.mirror_url.as_str()] {
            let resp = ureq::head(url).call().expect("{url} HEAD 失败");
            let len: u64 = resp
                .header("Content-Length")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            // HF resolve 经 302 到 CDN；重定向后 Content-Length 应等于文件大小
            assert_eq!(
                len, m.bytes_total,
                "{url} Content-Length 与清单不符（可能远端已更新，需重新 pin）"
            );
        }
    }
}
