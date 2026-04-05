use russh_sftp::protocol::{
    Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// SFTP session handler backed by the real container filesystem.
pub struct SftpSession {
    version: Option<u32>,
    handles: HashMap<String, HandleState>,
    next_handle: u64,
}

enum HandleState {
    File {
        path: PathBuf,
        file: std::fs::File,
    },
    Dir {
        path: PathBuf,
        read_done: bool,
    },
}

impl Default for SftpSession {
    fn default() -> Self {
        Self {
            version: None,
            handles: HashMap::new(),
            next_handle: 0,
        }
    }
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
                    .insert(handle.clone(), HandleState::File { path, file });
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
        apply_attrs(&path, &attrs)?;
        Ok(Self::ok_status(id))
    }

    async fn fsetstat(
        &mut self,
        id: u32,
        handle: String,
        attrs: FileAttributes,
    ) -> Result<Status, Self::Error> {
        let state = self.handles.get(&handle).ok_or(StatusCode::Failure)?;
        let path = match state {
            HandleState::File { path, .. } => path.clone(),
            HandleState::Dir { path, .. } => path.clone(),
        };
        apply_attrs(&path, &attrs)?;
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

fn apply_attrs(path: &Path, attrs: &FileAttributes) -> Result<(), StatusCode> {
    #[cfg(unix)]
    {
        if let Some(perms) = attrs.permissions {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(perms & 0o7777);
            std::fs::set_permissions(path, permissions).map_err(|e| io_to_status(&e))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, attrs);
    }
    Ok(())
}
