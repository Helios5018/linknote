use crate::document::Document;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Length plus FNV-1a hash. Enough to tell whether a file still matches the version
/// an edit was based on, without storing that version in every recovery draft.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub len: u64,
    pub hash: u64,
}

pub fn fingerprint(bytes: &[u8]) -> Fingerprint {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    Fingerprint {
        len: bytes.len() as u64,
        hash: h,
    }
}

/// The disk version that local edits are based on.
#[derive(Debug)]
enum Base {
    Bytes(Vec<u8>),
    /// After recovering a draft whose base no longer matches the disk.
    Known(Fingerprint),
}

impl Base {
    fn matches(&self, disk: &[u8]) -> bool {
        match self {
            Base::Bytes(bytes) => bytes == disk,
            Base::Known(f) => fingerprint(disk) == *f,
        }
    }
    fn fingerprint(&self) -> Fingerprint {
        match self {
            Base::Bytes(bytes) => fingerprint(bytes),
            Base::Known(f) => *f,
        }
    }
}

/// An exclusive lock file for one note. The file is removed when no draft needs it.
#[derive(Debug)]
struct Lock {
    file: fs::File,
    path: PathBuf,
    recovery: PathBuf,
}

impl Lock {
    fn acquire(recovery: &Path) -> Result<Self> {
        let path = recovery.with_extension("lock");
        for _ in 0..4 {
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&path)?;
            file.try_lock()
                .context("这篇笔记正在另一个 LinkNote 窗口中编辑，请先关闭它")?;
            // Another instance may have removed the file between open and lock.
            if same_file(&file, &path) {
                return Ok(Self {
                    file,
                    path,
                    recovery: recovery.to_path_buf(),
                });
            }
        }
        bail!("无法锁定笔记，请稍后重试")
    }
}

#[cfg(unix)]
fn same_file(file: &fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(_: &fs::File, path: &Path) -> bool {
    path.exists()
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Remove while still holding the lock; acquirers verify the path afterwards.
        if !self.recovery.exists() {
            let _ = fs::remove_file(&self.path);
        }
        let _ = self.file.unlock();
    }
}

#[derive(Debug)]
pub struct Note {
    pub path: PathBuf,
    pub doc: Document,
    base: Base,
    saved_text: String,
    pub crlf: bool,
    pub conflict: bool,
    pub error: Option<String>,
    pub changed_at: Instant,
    pub first_change: Instant,
    pub scroll: usize,
    pub recovery_path: PathBuf,
    /// Why the last recovery draft could not be written. Saving continues regardless.
    pub journal_error: Option<String>,
    journal_revision: Option<u64>,
    lock: Lock,
}

#[derive(Serialize, Deserialize)]
pub struct Draft {
    pub path: PathBuf,
    /// Drafts written by 0.1.0 stored the whole base file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<Vec<u8>>,
    #[serde(default)]
    base_fingerprint: Option<Fingerprint>,
    pub text: String,
    pub cursor: usize,
    pub crlf: bool,
}

/// A recovery draft that could not be read. It was moved to the given path so it
/// neither prevents opening the note nor gets overwritten by the next draft.
#[derive(Debug)]
pub struct DraftSetAside(pub PathBuf);

impl std::fmt::Display for DraftSetAside {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "恢复草稿已损坏，已移到 {}", self.0.display())
    }
}

impl std::error::Error for DraftSetAside {}

impl Draft {
    pub fn base_fingerprint(&self) -> Fingerprint {
        self.base_fingerprint
            .or_else(|| self.base.as_deref().map(fingerprint))
            .unwrap_or_default()
    }
}

pub fn read_note(path: &Path) -> Result<Vec<u8>> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("无法读取 {}", path.display()))?;
    if meta.file_type().is_symlink() {
        bail!("暂不编辑符号链接，请打开实际文件");
    }
    if !meta.is_file() {
        bail!("不是普通文件");
    }
    if meta.len() > MAX_FILE_BYTES {
        bail!("文件超过 4 MiB，暂不支持在此编辑");
    }
    let bytes = fs::read(path)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("文件超过 4 MiB");
    }
    std::str::from_utf8(&bytes).context("文件不是 UTF-8，未修改原文件")?;
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> (String, bool) {
    let s = String::from_utf8_lossy(bytes);
    let crlf = s.contains("\r\n");
    (s.replace("\r\n", "\n"), crlf)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("文件没有父目录")?;
    let mut temp = NamedTempFile::new_in(parent).context("无法创建保存临时文件")?;
    if let Ok(meta) = fs::metadata(path) {
        temp.as_file().set_permissions(meta.permissions())?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|e| e.error)
        .context("无法原子替换文件")?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn recovery_path(dir: &Path, path: &Path) -> PathBuf {
    // Stable FNV-1a. The original path in the draft is also checked before use.
    let mut h = 0xcbf29ce484222325u64;
    for b in path.as_os_str().as_encoded_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    dir.join(format!("{h:016x}.json"))
}

impl Note {
    pub fn relocate(&mut self, path: PathBuf) -> Result<()> {
        let recovery = recovery_path(self.recovery_path.parent().unwrap(), &path);
        let lock = Lock::acquire(&recovery).context("目标笔记已在另一个窗口打开")?;
        self.clear_journal()?;
        self.path = path;
        self.recovery_path = recovery;
        self.journal_revision = None;
        self.lock = lock;
        Ok(())
    }
    pub fn open(path: PathBuf, recovery_dir: &Path) -> Result<Self> {
        fs::create_dir_all(recovery_dir)?;
        let recovery_path = recovery_path(recovery_dir, &path);
        let lock = Lock::acquire(&recovery_path)?;
        let base = read_note(&path)?;
        let (text, crlf) = decode(&base);
        let now = Instant::now();
        Ok(Self {
            path,
            doc: Document::new(text.clone()),
            saved_text: text,
            base: Base::Bytes(base),
            crlf,
            conflict: false,
            error: None,
            changed_at: now,
            first_change: now,
            scroll: 0,
            recovery_path,
            journal_error: None,
            journal_revision: None,
            lock,
        })
    }
    pub fn bytes(&self) -> Vec<u8> {
        if self.crlf {
            self.doc.text.replace('\n', "\r\n").into_bytes()
        } else {
            self.doc.text.as_bytes().to_vec()
        }
    }
    pub fn dirty(&self) -> bool {
        self.doc.text != self.saved_text
    }
    pub fn edited(&mut self, was_dirty: bool) {
        self.changed_at = Instant::now();
        if !was_dirty {
            self.first_change = self.changed_at;
        }
        self.error = None;
    }
    /// Drafts of large notes are written less often; each one rewrites the whole text.
    pub fn journal_interval(&self) -> Duration {
        if self.doc.text.len() > 512 * 1024 {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(150)
        }
    }
    pub fn save_due(&self) -> bool {
        self.dirty()
            && !self.conflict
            && self.error.is_none()
            && (self.changed_at.elapsed().as_millis() >= 500
                || self.first_change.elapsed().as_secs() >= 2)
    }
    pub fn journal(&mut self) -> Result<()> {
        let result = self.write_journal();
        self.journal_error = result.as_ref().err().map(|e| format!("{e:#}"));
        result
    }
    fn write_journal(&mut self) -> Result<()> {
        if !self.dirty() {
            self.clear_journal()?;
            self.journal_revision = None;
            return Ok(());
        }
        if self.journal_revision == Some(self.doc.revision) {
            return Ok(());
        }
        fs::create_dir_all(self.recovery_path.parent().unwrap())?;
        let draft = Draft {
            path: self.path.clone(),
            base: None,
            base_fingerprint: Some(self.base.fingerprint()),
            text: self.doc.text.clone(),
            cursor: self.doc.cursor,
            crlf: self.crlf,
        };
        atomic_write(&self.recovery_path, &serde_json::to_vec(&draft)?)?;
        self.journal_revision = Some(self.doc.revision);
        Ok(())
    }
    pub fn draft(&self) -> Result<Option<Draft>> {
        let bytes = match fs::read(&self.recovery_path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let draft = match serde_json::from_slice::<Draft>(&bytes) {
            Ok(draft) if draft.path == self.path => draft,
            _ => return Err(DraftSetAside(self.set_aside_draft()?).into()),
        };
        if draft.text == self.doc.text {
            self.clear_journal()?;
            return Ok(None);
        }
        Ok(Some(draft))
    }
    fn set_aside_draft(&self) -> Result<PathBuf> {
        for n in 1..10_000 {
            let aside = self
                .recovery_path
                .with_extension(format!("corrupt-{n}.json"));
            if !aside.exists() {
                fs::rename(&self.recovery_path, &aside).context("无法移走损坏的恢复草稿")?;
                return Ok(aside);
            }
        }
        bail!("无法移走损坏的恢复草稿")
    }
    pub fn recover(&mut self, draft: Draft) {
        let base = draft.base_fingerprint();
        self.conflict = self.base.fingerprint() != base;
        if self.conflict {
            // The disk changed since the draft: keep detecting it as a conflict.
            self.base = Base::Known(base);
        }
        self.crlf = draft.crlf;
        self.doc.reload(draft.text);
        self.doc.set_cursor(draft.cursor, false);
        self.edited(false);
    }
    pub fn clear_journal(&self) -> Result<()> {
        match fs::remove_file(&self.recovery_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&mut self) -> Result<()> {
        if !self.dirty() {
            self.clear_journal()?;
            return Ok(());
        }
        // Best effort: a failing draft is reported separately and must not stop the
        // note itself from being saved.
        let _ = self.journal();
        let disk = read_note(&self.path);
        if disk.as_ref().map(|v| !self.base.matches(v)).unwrap_or(true) {
            self.conflict = true;
            bail!("文件已被外部修改或删除，已暂停自动保存；Ctrl+R 处理冲突");
        }
        let bytes = self.bytes();
        if fs::metadata(&self.path)?.permissions().readonly() {
            bail!("文件为只读，未覆盖原文件；Ctrl+R 可保留副本");
        }
        // Check again after the temporary file has been flushed. This catches changes
        // made during the slow part of a save; unrelated editors do not share our lock.
        let mut temp = NamedTempFile::new_in(self.path.parent().context("文件没有父目录")?)?;
        temp.as_file()
            .set_permissions(fs::metadata(&self.path)?.permissions())?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        if read_note(&self.path)
            .as_ref()
            .map(|v| !self.base.matches(v))
            .unwrap_or(true)
        {
            self.conflict = true;
            bail!("保存期间磁盘文件发生变化，已保留本地草稿");
        }
        temp.persist(&self.path).map_err(|e| e.error)?;
        #[cfg(unix)]
        fs::File::open(self.path.parent().unwrap())?.sync_all()?;
        self.base = Base::Bytes(bytes);
        self.saved_text = self.doc.text.clone();
        self.conflict = false;
        self.error = None;
        self.clear_journal()?;
        self.journal_revision = None;
        Ok(())
    }
    pub fn check_external(&mut self) -> Result<bool> {
        let disk = match read_note(&self.path) {
            Ok(disk) => disk,
            Err(e) => {
                self.conflict = true;
                return Err(e);
            }
        };
        if self.base.matches(&disk) {
            // The disk is back at the version the edits are based on, e.g. after a
            // branch switch or a sync tool briefly removed the file.
            if self.conflict {
                self.conflict = false;
                self.error = None;
            }
            return Ok(false);
        }
        if self.dirty() {
            self.conflict = true;
            return Ok(false);
        }
        self.accept_disk(disk)?;
        Ok(true)
    }
    /// Replace the buffer with a disk version. The old text stays in undo history.
    fn accept_disk(&mut self, disk: Vec<u8>) -> Result<()> {
        let (text, crlf) = decode(&disk);
        self.doc.reload(text);
        self.saved_text = self.doc.text.clone();
        self.base = Base::Bytes(disk);
        self.crlf = crlf;
        self.conflict = false;
        self.error = None;
        self.journal_revision = None;
        self.clear_journal()
    }
    /// Conflict resolution: discard local edits in favour of the disk version (undoable).
    pub fn use_disk(&mut self) -> Result<()> {
        let disk = read_note(&self.path).context("磁盘上没有可用的版本")?;
        self.accept_disk(disk)
    }
    /// Conflict resolution chosen explicitly by the user: write the local text over
    /// whatever is on disk now, or recreate a deleted file.
    pub fn overwrite_disk(&mut self) -> Result<()> {
        self.base = Base::Bytes(read_note(&self.path).unwrap_or_default());
        if !self.path.exists() {
            atomic_write(&self.path, b"")?;
        }
        self.conflict = false;
        self.error = None;
        if !self.dirty() {
            self.saved_text = String::new();
        }
        self.save()
    }
    /// The current disk text for a diff view; empty when the file is missing.
    pub fn disk_text(&self) -> Option<String> {
        read_note(&self.path).ok().map(|b| decode(&b).0)
    }
    /// Preserve the local version in a new file; never overwrite the external version.
    pub fn keep_both(&mut self) -> Result<PathBuf> {
        let parent = self.path.parent().unwrap();
        let stem = self.path.file_stem().unwrap_or_default().to_string_lossy();
        for n in 1..10_000 {
            let copy = parent.join(format!("{stem}.local-{n}.md"));
            let mut temp = NamedTempFile::new_in(parent)?;
            temp.write_all(&self.bytes())?;
            temp.as_file().sync_all()?;
            match temp.persist_noclobber(&copy) {
                Ok(_) => {
                    self.clear_journal()?;
                    return Ok(copy);
                }
                Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.error.into()),
            }
        }
        bail!("无法找到可用的副本文件名")
    }
}
