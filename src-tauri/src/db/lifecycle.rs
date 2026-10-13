use std::collections::HashMap;
use std::path::Path;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Entry { closed: bool, deleting: bool, connections: usize }
fn registry() -> &'static (Mutex<HashMap<String, Entry>>, Condvar) {
    static REGISTRY: OnceLock<(Mutex<HashMap<String, Entry>>, Condvar)> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
fn key(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let value = absolute.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { value.to_lowercase() } else { value }
}
pub(super) struct Lease(String);
impl Drop for Lease {
    fn drop(&mut self) {
        let (entries, changed) = registry();
        if let Ok(mut entries) = entries.lock() {
            if let Some(entry) = entries.get_mut(&self.0) { entry.connections = entry.connections.saturating_sub(1); }
            changed.notify_all();
        }
    }
}
pub(super) fn acquire(path: &Path, inspection: bool) -> rusqlite::Result<Lease> {
    let mut entries = registry().0.lock().expect("database lifecycle poisoned");
    let key = key(path);
    let entry = entries.entry(key.clone()).or_default();
    if entry.closed && (!inspection || entry.deleting) {
        return Err(rusqlite::Error::ToSqlConversionFailure(std::io::Error::new(
            std::io::ErrorKind::WouldBlock, "数据库已关闭，请从库选择器重新打开",
        ).into()));
    }
    entry.connections += 1;
    Ok(Lease(key))
}
pub fn block(path: &Path) { registry().0.lock().unwrap().entry(key(path)).or_default().closed = true; }
pub fn resume(path: &Path) { let mut entries = registry().0.lock().unwrap(); let entry = entries.entry(key(path)).or_default(); entry.closed = false; entry.deleting = false; }
pub fn begin_delete(path: &Path) { let mut entries = registry().0.lock().unwrap(); let entry = entries.entry(key(path)).or_default(); entry.closed = true; entry.deleting = true; }
pub fn cancel_delete(path: &Path) { registry().0.lock().unwrap().entry(key(path)).or_default().deleting = false; }
pub fn is_closed(path: &Path) -> bool { registry().0.lock().unwrap().get(&key(path)).is_some_and(|e| e.closed) }
pub fn wait_closed(path: &Path, timeout: Duration) -> bool {
    let (entries, changed) = registry();
    let mut entries = entries.lock().unwrap();
    let key = key(path);
    let deadline = Instant::now() + timeout;
    while entries.get(&key).is_some_and(|entry| entry.connections != 0) {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else { return false; };
        entries = changed.wait_timeout(entries, remaining).unwrap().0;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_waits_for_connections_and_prevents_recreating_deleted_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("database");
        let file = dir.join("library.db");
        let db = crate::db::Db::open(&file).unwrap();
        block(&file);
        assert!(!wait_closed(&file, Duration::from_millis(5)));
        drop(db);
        assert!(wait_closed(&file, Duration::from_secs(1)));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(crate::db::Db::open(&file).is_err());
        assert!(!dir.exists());
        resume(&file);
        assert!(crate::db::Db::open(&file).is_ok());
    }
}
