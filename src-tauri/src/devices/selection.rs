//! Explicit local file/folder selections share the existing import pipeline.
use super::{
    folder::{absolutize, LocalFolderSource},
    DeviceError, DeviceResult, DeviceSource, FileEntry, SourceKind,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

struct Location {
    root: PathBuf,
    file: Option<String>,
    source: LocalFolderSource,
}

pub struct LocalPathsSource {
    id: String,
    locations: Vec<Location>,
}

impl LocalPathsSource {
    pub fn new(paths: &[String]) -> DeviceResult<Self> {
        let mut paths: Vec<_> = paths
            .iter()
            .map(|path| absolutize(Path::new(path)))
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            return Err(DeviceError::Other("请选择图片文件或文件夹".into()));
        }
        let mut hash = Sha256::new();
        let mut locations = Vec::new();
        for path in paths {
            let metadata = fs::metadata(&path)?;
            let (root, file) = if metadata.is_dir() {
                (path.clone(), None)
            } else if metadata.is_file()
                && super::is_media_ext(&path.extension().unwrap_or_default().to_string_lossy())
            {
                (
                    path.parent()
                        .ok_or_else(|| DeviceError::Other("文件目录无效".into()))?
                        .to_path_buf(),
                    Some(
                        path.file_name()
                            .ok_or_else(|| DeviceError::Other("文件名无效".into()))?
                            .to_string_lossy()
                            .into_owned(),
                    ),
                )
            } else {
                continue;
            };
            hash.update(path.to_string_lossy().as_bytes());
            hash.update([0]);
            let source = LocalFolderSource::new(&root)?;
            locations.push(Location { root, file, source });
        }
        if locations.is_empty() {
            return Err(DeviceError::Other("所选项目没有支持的图片文件".into()));
        }
        Ok(Self {
            id: format!("FILES:{:x}", hash.finalize()),
            locations,
        })
    }

    fn resolve<'a>(&'a self, id: &'a str) -> DeviceResult<(&'a LocalFolderSource, &'a str)> {
        let (index, relative) = id
            .split_once('/')
            .ok_or_else(|| DeviceError::Other("文件标识无效".into()))?;
        let location = index
            .parse::<usize>()
            .ok()
            .and_then(|index| self.locations.get(index))
            .ok_or_else(|| DeviceError::Other("文件来源无效".into()))?;
        if relative.is_empty()
            || Path::new(relative).components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
            || location.file.as_ref().is_some_and(|file| file != relative)
        {
            return Err(DeviceError::Other("文件不在所选来源中".into()));
        }
        Ok((&location.source, relative))
    }
}

impl DeviceSource for LocalPathsSource {
    fn id(&self) -> String {
        self.id.clone()
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }
    fn name(&self) -> String {
        format!("已选择 {} 个项目", self.locations.len())
    }
    fn copy_only(&self) -> bool {
        self.locations
            .iter()
            .any(|location| location.source.copy_only())
    }
    fn local_roots(&self) -> Vec<PathBuf> {
        self.locations
            .iter()
            .map(|location| {
                location
                    .file
                    .as_ref()
                    .map(|file| location.root.join(file))
                    .unwrap_or_else(|| location.root.clone())
            })
            .collect()
    }
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for (index, location) in self.locations.iter().enumerate() {
            let entries = if let Some(file) = &location.file {
                let metadata = fs::metadata(location.root.join(file))?;
                vec![FileEntry {
                    id: file.clone(),
                    rel_path: file.clone(),
                    size: metadata.len(),
                    mtime: metadata.modified()?.into(),
                }]
            } else {
                location.source.list_names()?
            };
            for mut entry in entries {
                let path = location
                    .source
                    .local_path(&entry.id)
                    .ok_or_else(|| DeviceError::Other("文件路径不可用".into()))?;
                if !seen.insert(path.clone()) {
                    continue;
                }
                entry.id = format!("{index}/{}", entry.id);
                entry.rel_path = path.to_string_lossy().replace('\\', "/");
                files.push(entry);
            }
        }
        files.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(files)
    }
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        let (source, id) = self.resolve(id)?;
        source.open_head(id, max)
    }
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        let (source, id) = self.resolve(id)?;
        source.stream(id)
    }
    fn local_path(&self, id: &str) -> Option<PathBuf> {
        let (source, id) = self.resolve(id).ok()?;
        source.local_path(id)
    }
    fn delete(&self, id: &str) -> DeviceResult<()> {
        let (source, id) = self.resolve(id)?;
        source.delete(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_roots_preserve_original_paths_and_deduplicate_overlapping_selections() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("same.jpg"), b"one").unwrap();
        fs::write(second.join("same.jpg"), b"two").unwrap();
        fs::write(first.join("unselected.jpg"), b"other").unwrap();
        let paths = vec![
            first.join("same.jpg").to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
            second.join("same.jpg").to_string_lossy().into_owned(),
        ];
        let source = LocalPathsSource::new(&paths).unwrap();
        let entries = source.list().unwrap();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert!(source.local_path(&entry.id).unwrap().is_file());
            assert_eq!(source.open_head(&entry.id, 3).unwrap().len(), 3);
        }
        let mut reverse = paths.clone();
        reverse.reverse();
        assert_eq!(source.id(), LocalPathsSource::new(&reverse).unwrap().id());
    }
    #[test]
    fn explicit_files_cannot_open_or_delete_other_files_in_their_parent() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected.jpg");
        fs::write(&selected, b"one").unwrap();
        let other = temp.path().join("other.jpg");
        fs::write(&other, b"two").unwrap();
        let source = LocalPathsSource::new(&[selected.to_string_lossy().into_owned()]).unwrap();
        assert!(source.stream("0/other.jpg").is_err());
        assert!(source.delete("0/other.jpg").is_err());
        assert!(source.local_path("0/../other.jpg").is_none());
        assert!(other.is_file());
    }
}
