use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// SFTP session handler backed by the real container filesystem.
#[derive(Default)]
pub struct SftpSession {
    version: Option<u32>,
    handles: HashMap<String, HandleState>,
    next_handle: u64,
}

enum HandleState {
    File {
        file: std::fs::File,
    },
    Dir {
        path: PathBuf,
        read_done: bool,
    },
}

impl SftpSession {
    fn alloc_handle(&mut self) -> String {
        let h = self.next_handle;
        self.next_handle += 1;
        format!("h{}", h)
    }

    fn resolve_path(path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            Path::new("/").join(p)
        }
    }

    fn metadata_to_attrs(meta: &std::fs::Metadata) -> FileAttributes {
        FileAttributes::from(meta)
    }

    fn ok_status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: "Ok".to_string(),
            language_tag: "en-US".to_string(),
        }
    }

    fn file_entry(name: &str, meta: &std::fs::Metadata) -> File {
        File::new(name, FileAttributes::from(meta))
    }
}

impl russh_sftp::server::Handler for SftpSession {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        version: u32,
        _extensions: HashMap<String, String>,
    ) -> Result<Version, Self::Error> {
        if self.version.is_some() {
            return Err(StatusCode::ConnectionLost);
        }
        self.version = Some(version);
        debug!(version, "SFTP session initialized");
        Ok(Version::new())
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: OpenFlags,
        _attrs: FileAttributes,
    ) -> Result<Handle, Self::Error> {
        let path = Self::resolve_path(&filename);
        debug!(path = %path.display(), "SFTP open");

        let mut opts = std::fs::OpenOptions::new();
        if pflags.contains(OpenFlags::READ) {
            opts.read(true);
        }
        if pflags.contains(OpenFlags::WRITE) {
            opts.write(true);
        }
        if pflags.contains(OpenFlags::APPEND) {
            opts.append(true);
        }
        if pflags.contains(OpenFlags::CREATE) {
            opts.create(true);
        }
        if pflags.contains(OpenFlags::TRUNCATE) {
            opts.truncate(true);
        }
        if pflags.contains(OpenFlags::EXCLUDE) {
            opts.create_new(true);
        }

        match opts.open(&path) {
            Ok(file) => {
                let handle = self.alloc_handle();
                self.handles
                    .insert(handle.clone(), HandleState::File { file });
                Ok(Handle { id, handle })
            }
            Err(e) => {
                warn!(path = %path.display(), error = %e, "SFTP open failed");
                Err(io_to_status(&e))
            }
        }
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(Self::ok_status(id))
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<Data, Self::Error> {
        use std::io::{Read, Seek, SeekFrom};

        let state = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        match state {
            HandleState::File { file, .. } => {
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| io_to_status(&e))?;
                let mut buf = vec![0u8; len as usize];
                let n = file.read(&mut buf).map_err(|e| io_to_status(&e))?;
                if n == 0 {
                    return Err(StatusCode::Eof);
                }
                buf.truncate(n);
                Ok(Data { id, data: buf })
            }
            _ => Err(StatusCode::Failure),
        }
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<Status, Self::Error> {
        use std::io::{Seek, SeekFrom, Write};

        let state = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        match state {
            HandleState::File { file, .. } => {
                file.seek(SeekFrom::Start(offset))
                    .map_err(|e| io_to_status(&e))?;
                file.write_all(&data).map_err(|e| io_to_status(&e))?;
                Ok(Self::ok_status(id))
            }
            _ => Err(StatusCode::Failure),
        }
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = Self::resolve_path(&path);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| io_to_status(&e))?;
        Ok(Attrs {
            id,
            attrs: Self::metadata_to_attrs(&meta),
        })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let state = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        match state {
            HandleState::File { file, .. } => {
                let meta = file.metadata().map_err(|e| io_to_status(&e))?;
                Ok(Attrs {
                    id,
                    attrs: Self::metadata_to_attrs(&meta),
                })
            }
            HandleState::Dir { path, .. } => {
                let meta = std::fs::metadata(path).map_err(|e| io_to_status(&e))?;
                Ok(Attrs {
                    id,
                    attrs: Self::metadata_to_attrs(&meta),
                })
            }
        }
    }

    async fn setstat(
        &mut self,
        id: u32,
        path: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = Self::resolve_path(&path);
        apply_attrs(Target::Path(&path), &attrs)?;
        Ok(Self::ok_status(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let state = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        // A file handle is changed through its open descriptor: a truncate
        // on a handle opened read-only fails, as it should.
        match state {
            HandleState::File { file, .. } => apply_attrs(Target::File(file), &attrs)?,
            HandleState::Dir { path, .. } => apply_attrs(Target::Path(path), &attrs)?,
        }
        Ok(Self::ok_status(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let resolved = Self::resolve_path(&path);
        debug!(path = %resolved.display(), "SFTP opendir");

        if !resolved.is_dir() {
            return Err(StatusCode::NoSuchFile);
        }

        let handle = self.alloc_handle();
        self.handles.insert(
            handle.clone(),
            HandleState::Dir {
                path: resolved,
                read_done: false,
            },
        );
        Ok(Handle { id, handle })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        let state = self.handles.get_mut(&handle).ok_or(StatusCode::Failure)?;
        match state {
            HandleState::Dir { path, read_done } => {
                if *read_done {
                    return Err(StatusCode::Eof);
                }
                *read_done = true;

                let mut files = Vec::new();
                // Add . and ..
                if let Ok(meta) = std::fs::metadata(path.as_path()) {
                    files.push(Self::file_entry(".", &meta));
                }
                if let Some(parent) = path.parent() {
                    if let Ok(meta) = std::fs::metadata(parent) {
                        files.push(Self::file_entry("..", &meta));
                    }
                }

                match std::fs::read_dir(path.as_path()) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if let Ok(meta) = entry.metadata() {
                                files.push(Self::file_entry(&name, &meta));
                            }
                        }
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "SFTP readdir failed");
                        return Err(io_to_status(&e));
                    }
                }

                Ok(Name { id, files })
            }
            _ => Err(StatusCode::Failure),
        }
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        let path = Self::resolve_path(&filename);
        std::fs::remove_file(&path).map_err(|e| io_to_status(&e))?;
        Ok(Self::ok_status(id))
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let path = Self::resolve_path(&path);
        std::fs::create_dir(&path).map_err(|e| io_to_status(&e))?;
        Ok(Self::ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        let path = Self::resolve_path(&path);
        std::fs::remove_dir(&path).map_err(|e| io_to_status(&e))?;
        Ok(Self::ok_status(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let resolved = Self::resolve_path(&path);
        let canonical = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let name = canonical.to_string_lossy().to_string();
        let attrs = match std::fs::metadata(&canonical) {
            Ok(meta) => FileAttributes::from(&meta),
            Err(_) => FileAttributes::default(),
        };
        Ok(Name {
            id,
            files: vec![File::new(&name, attrs)],
        })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let path = Self::resolve_path(&path);
        let meta = std::fs::metadata(&path).map_err(|e| io_to_status(&e))?;
        Ok(Attrs {
            id,
            attrs: Self::metadata_to_attrs(&meta),
        })
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<Status, Self::Error> {
        let old = Self::resolve_path(&oldpath);
        let new = Self::resolve_path(&newpath);
        std::fs::rename(&old, &new).map_err(|e| io_to_status(&e))?;
        Ok(Self::ok_status(id))
    }

    async fn readlink(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let path = Self::resolve_path(&path);
        let target = std::fs::read_link(&path).map_err(|e| io_to_status(&e))?;
        let name = target.to_string_lossy().to_string();
        Ok(Name {
            id,
            files: vec![File::dummy(&name)],
        })
    }

    async fn symlink(
        &mut self,
        id: u32,
        linkpath: String,
        targetpath: String,
    ) -> Result<Status, Self::Error> {
        #[cfg(unix)]
        {
            let link = Self::resolve_path(&linkpath);
            std::os::unix::fs::symlink(&targetpath, &link).map_err(|e| io_to_status(&e))?;
            Ok(Self::ok_status(id))
        }
        #[cfg(not(unix))]
        {
            let _ = (id, linkpath, targetpath);
            Err(StatusCode::OpUnsupported)
        }
    }
}

fn io_to_status(e: &std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

/// What a SETSTAT or FSETSTAT changes: a path, or an open file.
enum Target<'a> {
    Path(&'a Path),
    File(&'a std::fs::File),
}

/// Apply a SETSTAT/FSETSTAT's attributes, all of them, or answer with why
/// not (stormd#93).
///
/// **Only permissions were applied, and OK answered regardless.** A client
/// that truncated before rewriting a shorter file was told it had, and the
/// old trailing bytes stayed. Now each attribute SFTP v3 carries is applied
/// — size (`set_len`, truncating or extending), owner (`chown`), permissions,
/// access and modification times — in an order where none undoes another
/// (times last: a truncate moves mtime), through the open file for an
/// FSETSTAT. The first that fails is the answer. User and group *names*
/// (SFTP v4+) are not something this server sets: `OpUnsupported`, before
/// anything is changed.
fn apply_attrs(target: Target, attrs: &FileAttributes) -> Result<(), StatusCode> {
    use std::os::unix::fs::PermissionsExt;
    if attrs.user.is_some() || attrs.group.is_some() {
        return Err(StatusCode::OpUnsupported);
    }
    let io = |e: std::io::Error| io_to_status(&e);
    if let Some(size) = attrs.size {
        match target {
            Target::File(f) => f.set_len(size).map_err(io)?,
            Target::Path(p) => std::fs::OpenOptions::new().write(true).open(p).and_then(|f| f.set_len(size)).map_err(io)?,
        }
    }
    if attrs.uid.is_some() || attrs.gid.is_some() {
        match target {
            Target::File(f) => std::os::unix::fs::fchown(f, attrs.uid, attrs.gid).map_err(io)?,
            Target::Path(p) => std::os::unix::fs::chown(p, attrs.uid, attrs.gid).map_err(io)?,
        }
    }
    if let Some(perms) = attrs.permissions {
        let permissions = std::fs::Permissions::from_mode(perms & 0o7777);
        match target {
            Target::File(f) => f.set_permissions(permissions).map_err(io)?,
            Target::Path(p) => std::fs::set_permissions(p, permissions).map_err(io)?,
        }
    }
    if attrs.atime.is_some() || attrs.mtime.is_some() {
        let at = |secs: u32| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs.into());
        let mut times = std::fs::FileTimes::new();
        if let Some(a) = attrs.atime {
            times = times.set_accessed(at(a));
        }
        if let Some(m) = attrs.mtime {
            times = times.set_modified(at(m));
        }
        match target {
            Target::File(f) => f.set_times(times).map_err(io)?,
            // Any open will do: setting times needs ownership, not write.
            Target::Path(p) => std::fs::File::open(p).and_then(|f| f.set_times(times)).map_err(io)?,
        }
    }
    Ok(())
}

/// stormd#93 (dbd850683639): SETSTAT and FSETSTAT do what they answer OK to.
#[cfg(test)]
mod setstat_tests {
    use super::SftpSession;
    use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
    use russh_sftp::server::Handler;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn dir(label: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stormd-sftp-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn size(n: u64) -> FileAttributes {
        FileAttributes { size: Some(n), ..FileAttributes::empty() }
    }

    #[tokio::test]
    async fn setstat_truncates_and_extends() {
        let d = dir("setstat");
        let f = d.join("f");
        std::fs::write(&f, b"0123456789").unwrap();
        let mut s = SftpSession::default();
        s.setstat(1, f.display().to_string(), size(4)).await.unwrap();
        let short = std::fs::read(&f).unwrap();
        s.setstat(2, f.display().to_string(), size(6)).await.unwrap();
        let long = std::fs::read(&f).unwrap();
        let missing = s.setstat(3, d.join("nope").display().to_string(), size(1)).await;
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(short, b"0123", "truncated");
        assert_eq!(long, b"0123\0\0", "extended with zeros");
        assert_eq!(missing.unwrap_err(), StatusCode::NoSuchFile);
    }

    /// The rewrite the finding names: truncate through the handle, write a
    /// shorter body, and nothing of the old one is left.
    #[tokio::test]
    async fn fsetstat_truncates_through_the_open_handle() {
        let d = dir("fsetstat");
        let f = d.join("config");
        std::fs::write(&f, b"a much longer old body").unwrap();
        let mut s = SftpSession::default();
        let h = s
            .open(1, f.display().to_string(), OpenFlags::WRITE, FileAttributes::empty())
            .await
            .unwrap()
            .handle;
        s.fsetstat(2, h.clone(), size(0)).await.unwrap();
        s.write(3, h.clone(), 0, b"short".to_vec()).await.unwrap();
        s.close(4, h).await.unwrap();
        // A handle opened read-only cannot truncate, and says so.
        let ro = s.open(5, f.display().to_string(), OpenFlags::READ, FileAttributes::empty()).await.unwrap().handle;
        let refused = s.fsetstat(6, ro, size(0)).await;
        let body = std::fs::read(&f).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(body, b"short");
        assert!(refused.is_err(), "a read-only handle reported a truncate as done");
    }

    #[tokio::test]
    async fn permissions_and_times_are_applied_and_names_refused() {
        let d = dir("attrs");
        let f = d.join("f");
        std::fs::write(&f, b"x").unwrap();
        let mut s = SftpSession::default();
        let attrs = FileAttributes {
            permissions: Some(0o640),
            atime: Some(1_000_000_000),
            mtime: Some(1_200_000_000),
            ..FileAttributes::empty()
        };
        s.setstat(1, f.display().to_string(), attrs).await.unwrap();
        let m = std::fs::metadata(&f).unwrap();
        let named = s
            .setstat(2, f.display().to_string(), FileAttributes { user: Some("root".into()), size: Some(0), ..FileAttributes::empty() })
            .await;
        let still = std::fs::read(&f).unwrap();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(m.permissions().mode() & 0o7777, 0o640);
        assert_eq!((m.atime(), m.mtime()), (1_000_000_000, 1_200_000_000));
        assert_eq!(named.unwrap_err(), StatusCode::OpUnsupported);
        assert_eq!(still, b"x", "nothing applied when refused");
    }
}
