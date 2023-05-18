use camino::Utf8PathBuf;
use dokan::{
    init, shutdown, unmount, CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler,
    FileSystemMounter, FileTimeOperation, FillDataError, FillDataResult, FindData, MountFlags,
    MountOptions, OperationInfo, OperationResult, VolumeInfo, IO_SECURITY_CONTEXT,
};
use dokan_sys::win32::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
    FILE_OVERWRITE, FILE_OVERWRITE_IF, FILE_SUPERSEDE,
};
use ouisync_lib::{deadlock::AsyncMutex, path, File, JointEntryRef, Repository};
use std::{
    collections::{hash_map, HashMap},
    fmt,
    io::{self, SeekFrom},
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::UNIX_EPOCH,
};
use tracing::instrument;
use widestring::{U16CStr, U16CString};
use winapi::{shared::ntstatus::*, um::winnt};

// Use the same value as NTFS.
pub const MAX_COMPONENT_LENGTH: u32 = 255;

pub fn mount(
    runtime_handle: tokio::runtime::Handle,
    repository: Arc<Repository>,
    mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    // TODO: Check these flags.
    let mut flags = MountFlags::empty();
    //flags |= ALT_STREAM;
    //flags |= MountFlags::DEBUG | MountFlags::STDERR;
    flags |= MountFlags::REMOVABLE;

    let options = MountOptions {
        single_thread: false,
        flags,
        ..Default::default()
    };

    let mount_point = match U16CString::from_os_str(mount_point.as_ref().as_os_str()) {
        Ok(mount_point) => mount_point,
        Err(error) => {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to convert mount point to U16CString: {error:?}"),
            ));
        }
    };

    let (on_mount_tx, on_mount_rx) = oneshot::channel();
    let (unmount_tx, unmount_rx) = mpsc::sync_channel(1);

    let join_handle = thread::spawn(move || {
        let handler = VirtualFilesystem::new(runtime_handle, repository);

        // TODO: Ensure this is done only once.
        init();

        let mut mounter = FileSystemMounter::new(&handler, &mount_point, &options);

        let file_system = match mounter.mount() {
            Ok(file_system) => file_system,
            Err(error) => {
                on_mount_tx
                    .send(Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to mount: {error:?}"),
                    )))
                    .unwrap_or(());
                return;
            }
        };

        // Tell the main thread we've successfully mounted.
        on_mount_tx.send(Ok(())).unwrap_or(());

        // Wait here to preserve `file_system`'s lifetime.
        unmount_rx.recv().unwrap_or(());

        // If we don't do this then dropping `file_system` will block.
        if !unmount(&mount_point) {
            tracing::warn!("Failed to unmount {mount_point:?}");
        }

        drop(file_system);

        shutdown();
    });

    if let Err(error) = on_mount_rx.recv().unwrap() {
        return Err(error);
    }

    Ok(MountGuard {
        unmount_tx,
        join_handle: Some(join_handle),
    })
}

pub struct MountGuard {
    unmount_tx: mpsc::SyncSender<()>,
    join_handle: Option<thread::JoinHandle<()>>,
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take() {
            self.unmount_tx.try_send(()).unwrap_or(());
            join_handle.join().unwrap_or(());
        }
    }
}

type Handles = HashMap<Utf8PathBuf, Arc<AsyncMutex<Shared>>>;

struct Shared {
    id: u64,
    path: Utf8PathBuf,
    handle_count: usize,
    delete_on_close: bool,
}

struct VirtualFilesystem {
    rt: tokio::runtime::Handle,
    repo: Arc<Repository>,
    handles: Arc<AsyncMutex<Handles>>,
    next_id: AtomicU64,
}

impl VirtualFilesystem {
    fn new(rt: tokio::runtime::Handle, repo: Arc<Repository>) -> Self {
        Self {
            rt,
            repo,
            handles: Arc::new(AsyncMutex::new(Default::default())),
            next_id: AtomicU64::new(1),
        }
    }

    fn generate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn get_or_set_shared(
        &self,
        path: Utf8PathBuf,
        delete_on_close: bool,
    ) -> (Arc<AsyncMutex<Shared>>, u64) {
        match self.handles.lock().await.entry(path.clone()) {
            hash_map::Entry::Occupied(entry) => {
                let shared = entry.get().clone();
                let mut lock = shared.lock().await;
                lock.handle_count += 1;
                lock.delete_on_close |= delete_on_close;
                let id = lock.id;
                drop(lock);
                (shared, id)
            }
            hash_map::Entry::Vacant(entry) => {
                let id = self.generate_id();
                let shared = Arc::new(AsyncMutex::new(Shared {
                    id,
                    path: path.clone(),
                    handle_count: 1,
                    delete_on_close,
                }));
                entry.insert(shared.clone());
                (shared, id)
            }
        }
    }

    async fn create_entry_impl(
        &self,
        path: &Utf8PathBuf,
        // Only used when creating a new entry, don't use it to determine what type of entry we
        // want to open if it does already exist.
        create_directory: bool,
        create_disposition: CreateDisposition,
        shared: Arc<AsyncMutex<Shared>>,
    ) -> Result<(Entry, bool), Error> {
        use ouisync_lib::Error as E;

        let (parent, child) = match path::decompose(path) {
            Some((parent, child)) => (parent, child),
            None => {
                // It's the root.
                return Ok((Entry::Directory(path.clone()), false));
            }
        };

        let dir = self.repo.cd(parent).await?;

        let existing_entry = match dir.lookup_unique(child) {
            Ok(existing_entry) => existing_entry,
            Err(E::EntryNotFound) => {
                if !create_disposition.should_create() {
                    return Err(E::EntryNotFound.into());
                }

                let created = true;

                if create_directory {
                    return self
                        .repo
                        .create_directory(&path)
                        .await
                        .map(|_dir| (Entry::Directory(path.clone()), created))
                        .map_err(|e| e.into());
                } else {
                    let mut file = self.repo.create_file(path).await?;
                    file.flush().await?;
                    return Ok((
                        Entry::File(FileEntry {
                            file: AsyncMutex::new(Some(file)),
                            shared,
                        }),
                        created,
                    ));
                }
            }
            Err(other) => return Err(other.into()),
        };

        let created = false;

        match existing_entry {
            JointEntryRef::File(file_entry) => {
                let file = file_entry.open().await?;
                Ok((
                    Entry::File(FileEntry {
                        file: AsyncMutex::new(Some(file)),
                        shared,
                    }),
                    created,
                ))
            }
            JointEntryRef::Directory(_) => Ok((Entry::Directory(path.clone()), created)),
        }
    }

    #[instrument(
        skip(self),
        fields(path, is_directory, create_disposition, delete_on_close),
        err(Debug)
    )]
    async fn create_entry(
        &self,
        path: Utf8PathBuf,
        // Only used when creating a new entry, don't use it to determine what type of entry we
        // want to open if it does already exist.
        create_directory: bool,
        create_disposition: CreateDisposition,
        delete_on_close: bool,
        delete_access: bool,
    ) -> Result<(Entry, bool, u64), Error> {
        let (shared, id) = self.get_or_set_shared(path.clone(), delete_on_close).await;

        let result = if !delete_access {
            self.create_entry_impl(&path, create_directory, create_disposition, shared)
                .await
        } else {
            Ok((
                Entry::File(FileEntry {
                    file: AsyncMutex::new(None),
                    shared,
                }),
                // TODO
                false,
            ))
        };

        if result.is_err() {
            match self.handles.lock().await.entry(path) {
                hash_map::Entry::Occupied(entry) => {
                    let mut handle = entry.get().lock().await;
                    handle.handle_count -= 1;
                    if handle.handle_count == 0 {
                        drop(handle);
                        entry.remove();
                    }
                }
                // Unreachable because we ensured it's occupied above.
                hash_map::Entry::Vacant(_) => unreachable!(),
            }
        }

        result.map(|(entry, is_new)| (entry, is_new, id))
    }

    async fn close_file_impl(&self, entry: &FileEntry) {
        let mut file_lock = entry.file.lock().await;

        let file = match file_lock.as_mut() {
            Some(file) => file,
            None => {
                tracing::error!("File already closed");
                return;
            }
        };

        if let Err(error) = file.flush().await {
            tracing::error!("Failed to flush on file close: {error:?}");
        }

        // Note: `handles` must never be locked *after* `entry.shared`.
        let mut handles = self.handles.lock().await;
        let mut lock = entry.shared.lock().await;

        match handles.entry(lock.path.clone()) {
            hash_map::Entry::Occupied(occupied) => {
                assert_eq!(Arc::as_ptr(occupied.get()), Arc::as_ptr(&entry.shared));

                lock.handle_count -= 1;

                if lock.handle_count == 0 {
                    let to_delete = if lock.delete_on_close {
                        Some(lock.path.clone())
                    } else {
                        None
                    };

                    // Close the file handle.
                    file_lock.take();
                    drop(lock);
                    drop(file_lock);
                    occupied.remove();

                    // Now all file handles to this particular file are closed, so we
                    // shouldn't get the `ouisync_lib::Error::Locked` error.
                    if let Some(to_delete) = to_delete {
                        if let Err(error) = self.repo.remove_entry(to_delete.clone()).await {
                            tracing::warn!(
                                "Failed to delete file \"{to_delete:?}\" on close: {error:?}"
                            );
                        }
                    }
                }
            }
            // This FileEntry exists, so it must be in `handles`.
            hash_map::Entry::Vacant(_) => unreachable!(),
        }
    }

    async fn find_files_impl(
        &self,
        mut fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        directory_path: &Utf8PathBuf,
        pattern: Option<&U16CStr>,
    ) -> Result<(), Error> {
        let dir = self.repo.open_directory(directory_path).await?;

        for entry in dir.entries() {
            let name = entry.unique_name();

            if name == "." || name == ".." {
                continue;
            }

            // TODO: Unwrap
            let file_name = U16CString::from_str(entry.unique_name().as_ref()).unwrap();

            let (attributes, file_size) = match &entry {
                JointEntryRef::File(file) => {
                    let file_size = match file.open().await {
                        Ok(file) => file.len(),
                        Err(_) => 0,
                    };
                    (winnt::FILE_ATTRIBUTE_NORMAL, file_size)
                }
                JointEntryRef::Directory(_) => {
                    // TODO: Count block sizes
                    (winnt::FILE_ATTRIBUTE_DIRECTORY, 0)
                }
            };

            if let Some(pattern) = pattern {
                let ignore_case = true;
                if !dokan::is_name_in_expression(pattern, &file_name, ignore_case) {
                    continue;
                }
            }

            fill_find_data(&FindData {
                attributes,
                // TODO
                creation_time: UNIX_EPOCH,
                last_access_time: UNIX_EPOCH,
                last_write_time: UNIX_EPOCH,
                file_size,
                file_name,
            })
            .or_else(ignore_name_too_long)?;
        }
        Ok(())
    }

    // https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilea
    // https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile
    async fn async_create_file<'c, 'h: 'c>(
        &'h self,
        file_name: &U16CStr,
        _security_context: &IO_SECURITY_CONTEXT,
        desired_access: winnt::ACCESS_MASK,
        _file_attributes: u32,
        _share_access: u32,
        create_disposition: u32,
        create_options: u32,
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> Result<CreateFileInfo<EntryHandle>, Error> {
        let create_disposition = create_disposition.try_into()?;
        let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
        let create_dir = create_options & FILE_DIRECTORY_FILE > 0;
        let delete_access = desired_access & winnt::DELETE > 0;

        let path = to_path(file_name)?;

        let (entry, is_new, id) = self
            .create_entry(
                path,
                create_dir,
                create_disposition,
                delete_on_close,
                delete_access,
            )
            .await?;

        let is_dir = entry.as_directory().is_ok();

        Ok(CreateFileInfo {
            context: EntryHandle { id, entry },
            is_dir,
            new_file_created: is_new,
        })
    }

    async fn async_close_file<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) {
        match &context.entry {
            Entry::File(entry) => {
                self.close_file_impl(entry).await;
            }
            Entry::Directory(_) => (),
        };
    }

    async fn async_read_file<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<u32, Error> {
        let entry = match &context.entry {
            Entry::File(entry) => entry,
            Entry::Directory(_) => return Err(STATUS_ACCESS_DENIED.into()),
        };

        let mut lock = entry.file.lock().await;
        let file = lock.as_mut().ok_or(STATUS_FILE_CLOSED)?;
        let offset: u64 = offset
            .try_into()
            .map_err(|_| ouisync_lib::Error::OffsetOutOfRange)?;
        file.seek(SeekFrom::Start(offset)).await?;
        let size = file.read(buffer).await?;

        Ok(size as u32)
    }

    async fn async_write_file<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<u32, Error> {
        let file_entry = context.entry.as_file()?;

        let mut lock = file_entry.file.lock().await;
        let file = lock.as_mut().ok_or(STATUS_FILE_CLOSED)?;

        let offset = if info.write_to_eof() {
            file.len()
        } else {
            offset.try_into().map_err(|_| STATUS_INVALID_PARAMETER)?
        };

        let local_branch = self.repo.local_branch()?;

        file.seek(SeekFrom::Start(offset)).await?;
        file.fork(local_branch).await?;
        file.write(buffer).await?;

        Ok(buffer.len().try_into().unwrap_or(u32::MAX))
    }

    async fn async_flush_file_buffers<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        Ok(())
    }

    #[instrument(skip(self, _info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_get_file_information<'c, 'h: 'c>(
        &'h self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<FileInfo, Error> {
        let (attributes, file_size) = match &context.entry {
            Entry::File(entry) => {
                let len = if let Some(file) = entry.file.lock().await.as_ref() {
                    file.len()
                } else {
                    // Ideally we wouldn't need to do this and expect that the file is open if we
                    // get to this function. But Windows insists on "opening" an entry, reading
                    // its size when moving/renaming a file prior to first closing it. This is
                    // currently verboten in ouisync due to this issue:
                    // https://github.com/equalitie/ouisync/issues/58
                    // And so we need to open the file manually, read the length and then close it.
                    let lock = entry.shared.lock().await;
                    let file = self.repo.open_file(&lock.path).await?;
                    file.len()
                };

                (winnt::FILE_ATTRIBUTE_NORMAL, len)
            }
            Entry::Directory(_) => (
                winnt::FILE_ATTRIBUTE_DIRECTORY,
                // TODO: Should we count the blocks?
                0,
            ),
        };

        Ok(FileInfo {
            attributes,
            // TODO
            creation_time: UNIX_EPOCH,
            last_access_time: UNIX_EPOCH,
            last_write_time: UNIX_EPOCH,
            file_size,
            number_of_links: 1,
            file_index: context.id,
        })
    }

    async fn async_find_files<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        let dir_path = context.entry.as_directory()?;
        self.find_files_impl(fill_find_data, dir_path, None).await
    }

    async fn async_find_files_with_pattern<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        let dir_path = context.entry.as_directory()?;
        self.find_files_impl(fill_find_data, dir_path, Some(pattern))
            .await
    }

    async fn async_set_file_attributes<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        _file_attributes: u32,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        Err(STATUS_NOT_IMPLEMENTED.into())
    }

    async fn async_set_file_time<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        _creation_time: FileTimeOperation,
        _last_access_time: FileTimeOperation,
        _last_write_time: FileTimeOperation,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::warn!("set_file_time not implemented yet");
        Ok(())
    }

    async fn async_delete_file<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        let file_entry = context.entry.as_file()?;
        file_entry.shared.lock().await.delete_on_close = info.delete_on_close();
        Ok(())
    }

    async fn async_delete_directory<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        todo!()
    }

    async fn async_move_file<'c, 'h: 'c>(
        &'h self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        _replace_if_existing: bool,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        // Note: Don't forget to rename in `self.handles`.
        let src_path = to_path(file_name)?;
        let dst_path = to_path(new_file_name)?;

        if src_path == dst_path {
            return Ok(());
        }

        let (src_dir, src_name) = path::decompose(&src_path).ok_or(STATUS_INVALID_PARAMETER)?;

        let (dst_dir, dst_name) = path::decompose(&dst_path).ok_or(STATUS_INVALID_PARAMETER)?;

        self.repo
            .move_entry(&src_dir, &src_name, &dst_dir, &dst_name)
            .await?;

        let mut handles = self.handles.lock().await;

        if let Some(handle) = handles.remove(&src_path) {
            let mut lock = handle.lock().await;
            lock.path = dst_path;
        }

        Ok(())
    }

    async fn async_set_end_of_file<'c, 'h: 'c>(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        // TODO: How do the fwo functions differ?
        self.async_set_allocation_size(file_name, offset, info, context)
            .await
    }

    async fn async_set_allocation_size<'c, 'h: 'c>(
        &'h self,
        _file_name: &U16CStr,
        alloc_size: i64,
        _info: &OperationInfo<'c, 'h, Self>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        let desired_len: u64 = alloc_size
            .try_into()
            .map_err(|_| STATUS_INVALID_PARAMETER)?;

        let entry = context.entry.as_file()?;
        let mut lock = entry.file.lock().await;
        let file = lock.as_mut().ok_or(STATUS_FILE_CLOSED)?;

        let start_len = file.len();

        if start_len == desired_len {
            return Ok(());
        }

        let local_branch = self.repo.local_branch()?;

        file.fork(local_branch).await?;

        if start_len > desired_len {
            file.truncate(desired_len).await?;
        } else {
            let start_pos = file.seek(SeekFrom::Current(0)).await?;
            file.seek(SeekFrom::End(0)).await?;
            // TODO: We shouldn't need to do allocation here as we're only writing constants.
            let buf = vec![0; (desired_len - start_len) as usize];
            file.write(&buf).await?;
            file.seek(SeekFrom::Start(start_pos)).await?;
        }

        Ok(())
    }

    async fn async_get_disk_free_space<'c, 'h: 'c>(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> Result<DiskSpaceInfo, Error> {
        // TODO
        Ok(DiskSpaceInfo {
            byte_count: 1024 * 1024 * 1024,
            free_byte_count: 512 * 1024 * 1024,
            available_byte_count: 512 * 1024 * 1024,
        })
    }

    async fn async_get_volume_information<'c, 'h: 'c>(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> Result<VolumeInfo, Error> {
        Ok(VolumeInfo {
            name: U16CString::from_str("ouisync").unwrap(),
            serial_number: 0,
            max_component_length: MAX_COMPONENT_LENGTH,
            fs_flags: winnt::FILE_CASE_PRESERVED_NAMES
                | winnt::FILE_CASE_SENSITIVE_SEARCH
                | winnt::FILE_UNICODE_ON_DISK
                | winnt::FILE_PERSISTENT_ACLS
                | winnt::FILE_NAMED_STREAMS,
            // Custom names don't play well with UAC.
            fs_name: U16CString::from_str("NTFS").unwrap(),
        })
    }

    async fn async_mounted<'c, 'h: 'c>(
        &'h self,
        _mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn async_unmounted<'c, 'h: 'c>(
        &'h self,
        _info: &OperationInfo<'c, 'h, Self>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Debug)]
enum CreateDisposition {
    // If the file already exists, replace it with the given file. If it does not, create the given file.
    FileSupersede,
    // If the file already exists, fail the request and do not create or open the given file. If it does not, create the given file.
    FileCreate,
    // If the file already exists, open it instead of creating a new file. If it does not, fail the request and do not create a new file.
    FileOpen,
    // If the file already exists, open it. If it does not, create the given file.
    FileOpenIf,
    //  If the file already exists, open it and overwrite it. If it does not, fail the request.
    FileOverwrite,
    //  If the file already exists, open it and overwrite it. If it does not, create the given file.
    FileOverwriteIf,
}

impl CreateDisposition {
    fn should_create(&self) -> bool {
        match self {
            Self::FileSupersede => true,
            Self::FileCreate => true,
            Self::FileOpen => false,
            Self::FileOpenIf => true,
            Self::FileOverwrite => false,
            Self::FileOverwriteIf => true,
        }
    }
}

impl TryFrom<u32> for CreateDisposition {
    type Error = Error;

    fn try_from(n: u32) -> Result<Self, Self::Error> {
        match n {
            FILE_SUPERSEDE => Ok(Self::FileSupersede),
            FILE_CREATE => Ok(Self::FileCreate),
            FILE_OPEN => Ok(Self::FileOpen),
            FILE_OPEN_IF => Ok(Self::FileOpenIf),
            FILE_OVERWRITE => Ok(Self::FileOverwrite),
            FILE_OVERWRITE_IF => Ok(Self::FileOverwriteIf),
            _ => Err(STATUS_INVALID_PARAMETER.into()),
        }
    }
}

struct FileEntry {
    file: AsyncMutex<Option<File>>,
    shared: Arc<AsyncMutex<Shared>>,
}

enum Entry {
    File(FileEntry),
    Directory(Utf8PathBuf),
}

impl Entry {
    fn as_file(&self) -> Result<&FileEntry, Error> {
        match self {
            Entry::File(file_entry) => Ok(file_entry),
            Entry::Directory(_) => Err(STATUS_INVALID_DEVICE_REQUEST.into()),
        }
    }

    fn as_directory(&self) -> Result<&Utf8PathBuf, Error> {
        match self {
            Entry::File(_) => Err(STATUS_INVALID_DEVICE_REQUEST.into()),
            Entry::Directory(path) => Ok(path),
        }
    }
}

struct EntryHandle {
    id: u64,
    entry: Entry,
}

//  https://dokan-dev.github.io/dokany-doc/html/struct_d_o_k_a_n___o_p_e_r_a_t_i_o_n_s.html
impl<'c, 'h: 'c> FileSystemHandler<'c, 'h> for VirtualFilesystem {
    type Context = EntryHandle;

    fn create_file(
        &'h self,
        file_name: &U16CStr,
        security_context: &IO_SECURITY_CONTEXT,
        desired_access: winnt::ACCESS_MASK,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        self.rt
            .block_on(self.async_create_file(
                file_name,
                security_context,
                desired_access,
                file_attributes,
                share_access,
                create_disposition,
                create_options,
                info,
            ))
            .map_err(Error::into)
    }

    fn cleanup(
        &'h self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Self>,
        _context: &'c Self::Context,
    ) {
    }

    fn close_file(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) {
        self.rt
            .block_on(self.async_close_file(file_name, info, context))
    }

    fn read_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        self.rt
            .block_on(self.async_read_file(file_name, offset, buffer, info, context))
            .map_err(Error::into)
    }

    fn write_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        self.rt
            .block_on(self.async_write_file(file_name, offset, buffer, info, context))
            .map_err(Error::into)
    }

    fn flush_file_buffers(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_flush_file_buffers(file_name, info, context))
            .map_err(Error::into)
    }

    fn get_file_information(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        self.rt
            .block_on(self.async_get_file_information(file_name, info, context))
            .map_err(Error::into)
    }

    fn find_files(
        &'h self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_find_files(file_name, fill_find_data, info, context))
            .map_err(Error::into)
    }

    fn find_files_with_pattern(
        &'h self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_find_files_with_pattern(
                file_name,
                pattern,
                fill_find_data,
                info,
                context,
            ))
            .map_err(Error::into)
    }

    fn set_file_attributes(
        &'h self,
        file_name: &U16CStr,
        file_attributes: u32,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_file_attributes(file_name, file_attributes, info, context))
            .map_err(Error::into)
    }

    fn set_file_time(
        &'h self,
        file_name: &U16CStr,
        creation_time: FileTimeOperation,
        last_access_time: FileTimeOperation,
        last_write_time: FileTimeOperation,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_file_time(
                file_name,
                creation_time,
                last_access_time,
                last_write_time,
                info,
                context,
            ))
            .map_err(Error::into)
    }

    fn delete_file(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_delete_file(file_name, info, context))
            .map_err(Error::into)
    }

    fn delete_directory(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_delete_directory(file_name, info, context))
            .map_err(Error::into)
    }

    fn move_file(
        &'h self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_move_file(
                file_name,
                new_file_name,
                replace_if_existing,
                info,
                context,
            ))
            .map_err(Error::into)
    }

    fn set_end_of_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_end_of_file(file_name, offset, info, context))
            .map_err(Error::into)
    }

    fn set_allocation_size(
        &'h self,
        file_name: &U16CStr,
        alloc_size: i64,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_allocation_size(file_name, alloc_size, info, context))
            .map_err(Error::into)
    }

    fn get_disk_free_space(
        &'h self,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        self.rt
            .block_on(self.async_get_disk_free_space(info))
            .map_err(Error::into)
    }

    fn get_volume_information(
        &'h self,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        self.rt
            .block_on(self.async_get_volume_information(info))
            .map_err(Error::into)
    }

    fn mounted(
        &'h self,
        mount_point: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_mounted(mount_point, info))
            .map_err(Error::into)
    }

    fn unmounted(&'h self, info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        self.rt
            .block_on(self.async_unmounted(info))
            .map_err(Error::into)
    }
}

fn ignore_name_too_long(err: FillDataError) -> OperationResult<()> {
    match err {
        // Normal behavior.
        FillDataError::BufferFull => Err(STATUS_BUFFER_OVERFLOW),
        // Silently ignore this error because 1) file names passed to create_file should have been checked
        // by Windows. 2) We don't want an error on a single file to make the whole directory unreadable.
        FillDataError::NameTooLong => Ok(()),
    }
}

enum Error {
    NtStatus(i32),
    OuiSync(ouisync_lib::Error),
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NtStatus(status) => match *status {
                STATUS_OBJECT_NAME_NOT_FOUND => write!(f, "STATUS_OBJECT_NAME_NOT_FOUND"),
                STATUS_ACCESS_DENIED => write!(f, "STATUS_ACCESS_DENIED"),
                STATUS_INVALID_PARAMETER => write!(f, "STATUS_INVALID_PARAMETER"),
                STATUS_NOT_IMPLEMENTED => write!(f, "STATUS_NOT_IMPLEMENTED"),
                STATUS_LOCK_NOT_GRANTED => write!(f, "STATUS_LOCK_NOT_GRANTED"),
                STATUS_INVALID_DEVICE_REQUEST => write!(f, "STATUS_INVALID_DEVICE_REQUEST"),
                STATUS_FILE_CLOSED => write!(f, "STATUS_FILE_CLOSED"),
                other => write!(f, "{:#x}", other),
            },
            Self::OuiSync(error) => {
                write!(f, "{:?}", error)
            }
        }
    }
}

impl From<i32> for Error {
    fn from(ntstatus: i32) -> Self {
        Self::NtStatus(ntstatus)
    }
}

impl From<ouisync_lib::Error> for Error {
    fn from(error: ouisync_lib::Error) -> Self {
        Self::OuiSync(error)
    }
}

impl From<Error> for i32 {
    fn from(error: Error) -> i32 {
        match error {
            // List of NTSTATUS values:
            // https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-erref/596a1078-e883-4972-9bbc-49e60bebca55
            Error::NtStatus(ntstatus) => ntstatus,
            Error::OuiSync(error) => {
                use ouisync_lib::Error as E;

                match error {
                    E::Db(_) => STATUS_INTERNAL_DB_ERROR,
                    E::PermissionDenied => STATUS_ACCESS_DENIED,
                    E::MalformedData => STATUS_DATA_ERROR,
                    E::BlockNotFound(_) => STATUS_RESOURCE_DATA_NOT_FOUND,
                    E::BlockNotReferenced => STATUS_RESOURCE_DATA_NOT_FOUND,
                    E::WrongBlockLength(_) => STATUS_DATA_ERROR,
                    E::MalformedDirectory => STATUS_DATA_ERROR,
                    E::EntryExists => STATUS_OBJECT_NAME_EXISTS,
                    E::EntryNotFound => STATUS_OBJECT_NAME_NOT_FOUND,
                    E::AmbiguousEntry => STATUS_FLT_DUPLICATE_ENTRY,
                    // These two are as they were used in the memfs dokan example.
                    E::EntryIsFile => STATUS_INVALID_DEVICE_REQUEST,
                    E::EntryIsDirectory => STATUS_INVALID_DEVICE_REQUEST,
                    E::NonUtf8FileName => STATUS_OBJECT_NAME_INVALID,
                    E::OffsetOutOfRange => STATUS_INVALID_PARAMETER,
                    E::DirectoryNotEmpty => STATUS_DIRECTORY_NOT_EMPTY,
                    E::OperationNotSupported => STATUS_NOT_IMPLEMENTED,
                    E::Writer(_) => STATUS_IO_DEVICE_ERROR,
                    E::RequestTimeout => STATUS_TIMEOUT,
                    E::StorageVersionMismatch => STATUS_IO_DEVICE_ERROR,
                    E::Locked => STATUS_LOCK_NOT_GRANTED,
                }
            }
        }
    }
}

fn to_path(path_cstr: &U16CStr) -> OperationResult<Utf8PathBuf> {
    let path_str: String = match path_cstr.to_string() {
        Ok(path_str) => path_str,
        Err(_) => {
            tracing::warn!("Failed to convert U16CStr to Utf8Path: {path_cstr:?}");
            return Err(STATUS_OBJECT_NAME_INVALID);
        }
    };

    Ok(Utf8PathBuf::from(path_str))
}
