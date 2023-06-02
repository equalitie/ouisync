pub(crate) mod multi_repo_mount;
pub(crate) mod single_repo_mount;

use camino::Utf8PathBuf;
use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileTimeOperation, FillDataError,
    FillDataResult, FindData, MountFlags, OperationInfo, OperationResult, VolumeInfo,
    IO_SECURITY_CONTEXT,
};
use dokan_sys::win32::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
    FILE_OVERWRITE, FILE_OVERWRITE_IF, FILE_SUPERSEDE,
};
use ouisync_lib::{
    deadlock::{AsyncMutex, AsyncMutexGuard},
    path, File, JointDirectory, JointEntryRef, Repository,
};
use std::{
    collections::{hash_map, HashMap},
    fmt,
    io::SeekFrom,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::UNIX_EPOCH,
};
use tracing::instrument;
use widestring::{U16CStr, U16CString};
use winapi::{shared::ntstatus::*, um::winnt};

// Use the same value as NTFS.
pub const MAX_COMPONENT_LENGTH: u32 = 255;

struct VirtualFilesystem {
    rt: tokio::runtime::Handle,
    repo: Arc<Repository>,
    handles: Arc<AsyncMutex<Handles>>,
    entry_id_generator: Arc<EntryIdGenerator>,
}

impl VirtualFilesystem {
    fn new(
        rt: tokio::runtime::Handle,
        entry_id_generator: Arc<EntryIdGenerator>,
        repo: Arc<Repository>,
    ) -> Self {
        Self {
            rt,
            repo,
            handles: Arc::new(AsyncMutex::new(Default::default())),
            entry_id_generator,
        }
    }

    async fn get_or_set_shared(
        &self,
        path: Utf8PathBuf,
        delete_on_close: bool,
    ) -> (Arc<AsyncMutex<Shared>>, u64) {
        // It is not clear whether `if path1 == path2 => id1 == id2` from the documentation, but
        // the memfs example in dokan-rust seems to always generate a new `id` in `create_file` so
        // the above probably doesn't have to hold.

        let id = self.entry_id_generator.generate_id();

        match self.handles.lock().await.entry(path.clone()) {
            hash_map::Entry::Occupied(entry) => {
                let shared = entry.get().clone();
                let mut lock = shared.lock().await;
                lock.handle_count += 1;
                lock.delete_on_close |= delete_on_close;
                drop(lock);
                (shared, id)
            }
            hash_map::Entry::Vacant(entry) => {
                let shared = Arc::new(AsyncMutex::new(Shared {
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
        delete_access: bool,
        shared: Arc<AsyncMutex<Shared>>,
    ) -> Result<(Entry, bool), Error> {
        use ouisync_lib::Error as E;

        let (parent, child) = match path::decompose(path) {
            Some((parent, child)) => (parent, child),
            None => {
                // It's the root.
                return Ok((Entry::new_dir(self.repo.cd(path).await?, shared), false));
            }
        };

        let parent_dir = self.repo.cd(parent).await?;

        let existing_entry = match parent_dir.lookup_unique(child) {
            Ok(existing_entry) => existing_entry,
            Err(E::EntryNotFound) => {
                if !create_disposition.should_create() {
                    return Err(E::EntryNotFound.into());
                }

                let entry = if delete_access {
                    if create_directory {
                        Entry::new_dir(parent_dir.cd(child).await?, shared)
                    } else {
                        Entry::new_file(
                            OpenState::Lazy {
                                path: path.clone(),
                                create_disposition,
                            },
                            shared,
                        )
                    }
                } else {
                    if create_directory {
                        self.repo.create_directory(&path).await?;
                        Entry::new_dir(self.repo.cd(path).await?, shared)
                    } else {
                        let mut file = self.repo.create_file(path).await?;
                        file.flush().await?;
                        Entry::new_file(OpenState::Open(file), shared)
                    }
                };

                return Ok((entry, true /* is new */));
            }
            Err(other) => return Err(other.into()),
        };

        let entry = if delete_access {
            match existing_entry {
                JointEntryRef::File(_) => Entry::new_file(
                    OpenState::Lazy {
                        path: path.clone(),
                        create_disposition,
                    },
                    shared,
                ),
                JointEntryRef::Directory(_) => Entry::new_dir(parent_dir.cd(child).await?, shared),
            }
        } else {
            match existing_entry {
                JointEntryRef::File(file_entry) => {
                    let file = file_entry.open().await?;
                    Entry::new_file(OpenState::Open(file), shared)
                }
                JointEntryRef::Directory(_) => Entry::new_dir(parent_dir.cd(child).await?, shared),
            }
        };

        Ok((entry, false /* not new */))
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
        tracing::trace!("create_entry");

        let (shared, id) = self.get_or_set_shared(path.clone(), delete_on_close).await;

        let result = self
            .create_entry_impl(
                &path,
                create_directory,
                create_disposition,
                delete_access,
                shared,
            )
            .await;

        if result.is_err() {
            match self.handles.lock().await.entry(path.clone()) {
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

    async fn close_shared(
        &self,
        shared: &Arc<AsyncMutex<Shared>>,
        handles: &mut AsyncMutexGuard<'_, Handles>,
    ) -> Option<Utf8PathBuf> {
        let mut lock = shared.lock().await;

        match handles.entry(lock.path.clone()) {
            hash_map::Entry::Occupied(occupied) => {
                assert_eq!(Arc::as_ptr(occupied.get()), Arc::as_ptr(&shared));

                lock.handle_count -= 1;

                if lock.handle_count == 0 {
                    let to_delete = if lock.delete_on_close {
                        Some(lock.path.clone())
                    } else {
                        None
                    };

                    drop(lock);
                    occupied.remove();

                    return to_delete;
                }

                return None;
            }
            // This FileEntry exists, so it must be in `handles`.
            hash_map::Entry::Vacant(_) => {
                panic!("File {:?} has no entry in `handles`", lock.path)
            }
        }
    }

    async fn find_files_impl(
        &self,
        mut fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        dir: &JointDirectory,
        pattern: Option<&U16CStr>,
    ) -> Result<(), Error> {
        // TODO: We might want to check whether the root version vector of the repository has
        // changed and reload the JointDirectory if so.
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
    #[instrument(skip(self, file_name, _security_context, access_mask, _file_attributes, _share_access, create_disposition, create_options), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_create_file<'c, 'h: 'c>(
        &'h self,
        file_name: &U16CStr,
        _security_context: &IO_SECURITY_CONTEXT,
        access_mask: AccessMask,
        _file_attributes: u32,
        _share_access: u32,
        create_disposition: u32,
        create_options: u32,
    ) -> Result<CreateFileInfo<EntryHandle>, Error> {
        tracing::trace!("async_create_file");

        let create_disposition = create_disposition.try_into()?;
        let delete_on_close = create_options & FILE_DELETE_ON_CLOSE > 0;
        let create_dir = create_options & FILE_DIRECTORY_FILE > 0;
        let delete_access = access_mask.has_delete();

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

    fn create_file(
        &self,
        file_name: &U16CStr,
        security_context: &IO_SECURITY_CONTEXT,
        desired_access: winnt::ACCESS_MASK,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
    ) -> OperationResult<CreateFileInfo<EntryHandle>> {
        self.rt
            .block_on(self.async_create_file(
                file_name,
                security_context,
                desired_access.into(),
                file_attributes,
                share_access,
                create_disposition,
                create_options,
            ))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _file_name, _info, context), fields(file_name = ?to_path(_file_name)))]
    async fn async_close_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) {
        tracing::trace!("async_close_file");

        match &context.entry {
            Entry::File(entry) => {
                let mut file_lock = entry.file.lock().await;

                match &mut *file_lock {
                    OpenState::Open(file) => {
                        if let Err(error) = file.flush().await {
                            tracing::error!("Failed to flush on file close: {error:?}");
                        }
                    }
                    OpenState::Lazy { .. } => (),
                    OpenState::Closed => {
                        tracing::error!("File already closed");
                    }
                };

                // Close the file handle.
                *file_lock = OpenState::Closed;
                drop(file_lock);
            }
            Entry::Directory(_) => (),
        };

        // We need to lock `self.handles` here to prevent anything from opening the file while this
        // function runs. It is because if the file is marked for removal here and if some other
        // function opens the file, then the function `self.repo.remove_entry` will fail with
        // `Error::Locked`.
        let mut handles = self.handles.lock().await;

        if let Some(to_delete) = self
            .close_shared(context.entry.shared(), &mut handles)
            .await
        {
            // Now all handles to this particular entry are closed, so we shouldn't get the
            // `ouisync_lib::Error::Locked` error.
            if let Err(error) = self.repo.remove_entry(to_delete.clone()).await {
                tracing::warn!("Failed to delete file \"{to_delete:?}\" on close: {error:?}");
            }
        }
    }

    fn close_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) {
        self.rt
            .block_on(self.async_close_file(file_name, info, context))
    }

    #[instrument(skip(self, _file_name, buffer, _info, context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_read_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<u32, Error> {
        tracing::trace!("async_read_file");

        let entry = match &context.entry {
            Entry::File(entry) => entry,
            Entry::Directory(_) => return Err(STATUS_ACCESS_DENIED.into()),
        };

        let mut lock = entry.file.lock().await;
        let file = lock.opened_file(&self.repo).await?;

        let offset: u64 = offset
            .try_into()
            .map_err(|_| ouisync_lib::Error::OffsetOutOfRange)?;
        file.seek(SeekFrom::Start(offset)).await?;
        let size = file.read(buffer).await?;

        Ok(size as u32)
    }

    fn read_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<u32> {
        self.rt
            .block_on(self.async_read_file(file_name, offset, buffer, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _file_name, buffer, info, context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_write_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<u32, Error> {
        let file_entry = context.entry.as_file()?;

        let mut lock = file_entry.file.lock().await;
        let file = lock.opened_file(&self.repo).await?;

        let offset = if info.write_to_eof() {
            file.len()
        } else {
            offset.try_into().map_err(|_| STATUS_INVALID_PARAMETER)?
        };

        let local_branch = self.repo.local_branch()?;

        if offset != file.len() {
            file.flush().await?;
            file.seek(SeekFrom::Start(offset)).await?;
        }

        tracing::trace!("L{} async_write_file", line!());
        file.fork(local_branch).await?;
        file.write(buffer).await?;

        Ok(buffer.len().try_into().unwrap_or(u32::MAX))
    }

    fn write_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<u32> {
        self.rt
            .block_on(self.async_write_file(file_name, offset, buffer, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _info, context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_flush_file_buffers<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_flush_file_buffers");
        match &context.entry {
            Entry::File(entry) => {
                let mut lock = entry.file.lock().await;
                if let OpenState::Open(file) = &mut *lock {
                    file.flush().await?;
                }
            }
            Entry::Directory(_) => (),
        }
        Ok(())
    }

    fn flush_file_buffers<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_flush_file_buffers(file_name, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_get_file_information<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<FileInfo, Error> {
        tracing::trace!("async_get_file_information");

        let (attributes, file_size) = match &context.entry {
            Entry::File(entry) => {
                let mut lock = entry.file.lock().await;
                let file = lock.opened_file(&self.repo).await?;
                let len = file.len();

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

    fn get_file_information<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<FileInfo> {
        self.rt
            .block_on(self.async_get_file_information(file_name, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, file_name, fill_find_data, _info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_find_files<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_find_files");
        let dir_entry = context.entry.as_directory()?;
        self.find_files_impl(fill_find_data, &dir_entry.dir, None)
            .await
    }

    fn find_files<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_find_files(file_name, fill_find_data, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, file_name, pattern, fill_find_data, _info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_find_files_with_pattern<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_find_files_with_pattern");
        let dir_entry = context.entry.as_directory()?;
        self.find_files_impl(fill_find_data, &dir_entry.dir, Some(pattern))
            .await
    }

    fn find_files_with_pattern<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
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

    #[instrument(skip(self, _file_name, _file_attributes, _info, _context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_set_file_attributes<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        _file_attributes: u32,
        _info: &OperationInfo<'c, 'h, Super>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_set_file_attributes");
        Err(STATUS_NOT_IMPLEMENTED.into())
    }

    fn set_file_attributes<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        file_attributes: u32,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_file_attributes(file_name, file_attributes, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _file_name, _creation_time, _last_access_time, _last_write_time, _info, _context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_set_file_time<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        _creation_time: FileTimeOperation,
        _last_access_time: FileTimeOperation,
        _last_write_time: FileTimeOperation,
        _info: &OperationInfo<'c, 'h, Super>,
        _context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::warn!("set_file_time not implemented yet");
        Ok(())
    }

    fn set_file_time<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        creation_time: FileTimeOperation,
        last_access_time: FileTimeOperation,
        last_write_time: FileTimeOperation,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
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

    #[instrument(skip(self, _file_name, info, context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_delete_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_delete_file");
        let file_entry = context.entry.as_file()?;
        file_entry.shared.lock().await.delete_on_close = info.delete_on_close();
        Ok(())
    }

    fn delete_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_delete_file(file_name, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, file_name, info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_delete_directory<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_delete_directory");
        let dir_entry = context.entry.as_directory()?;
        let path = to_path(file_name)?;
        let mut shared = dir_entry.shared.lock().await;

        let dir = self.repo.cd(&path).await?;

        match (dir.is_empty(), info.delete_on_close()) {
            (true, true) => shared.delete_on_close = true,
            (true, false) => shared.delete_on_close = false,
            (false, true) => return Err(STATUS_DIRECTORY_NOT_EMPTY.into()),
            (false, false) => shared.delete_on_close = false,
        }

        Ok(())
    }

    fn delete_directory<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_delete_directory(file_name, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, file_name, new_file_name, _replace_if_existing, _info, handle),
        fields(file_name = ?to_path(file_name), new_file_name = ?to_path(new_file_name)),
        err(Debug))]
    async fn async_move_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        _replace_if_existing: bool,
        _info: &OperationInfo<'c, 'h, Super>,
        handle: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_move_file");

        let src_path = to_path(file_name)?;
        let dst_path = to_path(new_file_name)?;

        if src_path == dst_path {
            return Ok(());
        }

        let (src_dir, src_name) = path::decompose(&src_path).ok_or(STATUS_INVALID_PARAMETER)?;

        let (dst_dir, dst_name) = path::decompose(&dst_path).ok_or(STATUS_INVALID_PARAMETER)?;

        // Lock this entry in `self.shared` so that no other thread can open/create the `File`
        // while we're renaming it.
        let _shared_lock = handle.entry.shared().lock().await;

        if let Entry::File(file_entry) = &handle.entry {
            let mut file = file_entry.file.lock().await;

            match *file {
                OpenState::Open(_) => {
                    // TODO: If this is to be reopened (which it probably won't), we should
                    // preserve the seek offset.
                    *file = OpenState::Lazy {
                        path: src_path.clone(),
                        create_disposition: CreateDisposition::FileOpen,
                    }
                }
                OpenState::Lazy { .. } | OpenState::Closed => {}
            }
        }

        self.repo
            .move_entry(&src_dir, &src_name, &dst_dir, &dst_name)
            .await?;

        Ok(())
    }

    fn move_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
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

    #[instrument(skip(self, file_name, info, context), fields(file_name = ?to_path(file_name)), err(Debug))]
    async fn async_set_end_of_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        offset: i64,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_set_end_of_file");
        // TODO: How do the fwo functions differ?
        self.async_set_allocation_size(file_name, offset, info, context)
            .await
    }

    fn set_end_of_file<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        offset: i64,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_end_of_file(file_name, offset, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _file_name, _info, context), fields(file_name = ?to_path(_file_name)), err(Debug))]
    async fn async_set_allocation_size<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _file_name: &U16CStr,
        alloc_size: i64,
        _info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> Result<(), Error> {
        tracing::trace!("async_set_allocation_size");
        let desired_len: u64 = alloc_size
            .try_into()
            .map_err(|_| STATUS_INVALID_PARAMETER)?;

        let entry = context.entry.as_file()?;
        let mut lock = entry.file.lock().await;
        let file = lock.opened_file(&self.repo).await?;

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

            let mut remaining: usize = (desired_len - start_len)
                .try_into()
                .map_err(|_| STATUS_INVALID_PARAMETER)?;

            let zeros = vec![0; ouisync_lib::BLOCK_SIZE];

            while remaining != 0 {
                let to_write = remaining.min(zeros.len());
                file.write(&zeros[0..to_write]).await?;
                remaining -= to_write;
            }

            file.seek(SeekFrom::Start(start_pos)).await?;
        }

        file.flush().await?;

        Ok(())
    }

    fn set_allocation_size<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        file_name: &U16CStr,
        alloc_size: i64,
        info: &OperationInfo<'c, 'h, Super>,
        context: &'c EntryHandle,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_set_allocation_size(file_name, alloc_size, info, context))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _info), err(Debug))]
    async fn async_get_disk_free_space<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _info: &OperationInfo<'c, 'h, Super>,
    ) -> Result<DiskSpaceInfo, Error> {
        tracing::trace!("async_get_disk_free_space");
        // TODO
        Ok(DiskSpaceInfo {
            byte_count: 1024 * 1024 * 1024,
            free_byte_count: 512 * 1024 * 1024,
            available_byte_count: 512 * 1024 * 1024,
        })
    }

    fn get_disk_free_space<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        info: &OperationInfo<'c, 'h, Super>,
    ) -> OperationResult<DiskSpaceInfo> {
        self.rt
            .block_on(self.async_get_disk_free_space(info))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _info), err(Debug))]
    async fn async_get_volume_information<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _info: &OperationInfo<'c, 'h, Super>,
    ) -> Result<VolumeInfo, Error> {
        tracing::trace!("async_get_volume_information");
        Ok(VolumeInfo {
            name: U16CString::from_str("ouisync").unwrap(),
            serial_number: 0,
            max_component_length: MAX_COMPONENT_LENGTH,
            fs_flags: winnt::FILE_CASE_PRESERVED_NAMES
                | winnt::FILE_CASE_SENSITIVE_SEARCH
                | winnt::FILE_UNICODE_ON_DISK,
            // Custom names don't play well with UAC.
            fs_name: U16CString::from_str("NTFS").unwrap(),
        })
    }

    fn get_volume_information<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        info: &OperationInfo<'c, 'h, Super>,
    ) -> OperationResult<VolumeInfo> {
        self.rt
            .block_on(self.async_get_volume_information(info))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _mount_point, _info), err(Debug))]
    async fn async_mounted<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _mount_point: &U16CStr,
        _info: &OperationInfo<'c, 'h, Super>,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn mounted<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        mount_point: &U16CStr,
        info: &OperationInfo<'c, 'h, Super>,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_mounted(mount_point, info))
            .map_err(Error::into)
    }

    #[instrument(skip(self, _info), err(Debug))]
    async fn async_unmounted<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        _info: &OperationInfo<'c, 'h, Super>,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn unmounted<'c, 'h: 'c, Super: FileSystemHandler<'c, 'h>>(
        &self,
        info: &OperationInfo<'c, 'h, Super>,
    ) -> OperationResult<()> {
        self.rt
            .block_on(self.async_unmounted(info))
            .map_err(Error::into)
    }
}

pub(crate) struct EntryIdGenerator {
    next_id: AtomicU64,
}

impl EntryIdGenerator {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
        }
    }

    fn generate_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

pub(crate) fn ignore_name_too_long(err: FillDataError) -> OperationResult<()> {
    match err {
        // Normal behavior.
        FillDataError::BufferFull => Err(STATUS_BUFFER_OVERFLOW),
        // Silently ignore this error because 1) file names passed to create_file should have been checked
        // by Windows. 2) We don't want an error on a single file to make the whole directory unreadable.
        FillDataError::NameTooLong => Ok(()),
    }
}

pub(crate) enum Error {
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

type Handles = HashMap<Utf8PathBuf, Arc<AsyncMutex<Shared>>>;

struct Shared {
    path: Utf8PathBuf,
    handle_count: usize,
    delete_on_close: bool,
}

struct FileEntry {
    shared: Arc<AsyncMutex<Shared>>,
    file: AsyncMutex<OpenState>,
}

struct DirEntry {
    shared: Arc<AsyncMutex<Shared>>,
    dir: JointDirectory,
}

enum Entry {
    File(FileEntry),
    Directory(DirEntry),
}

impl Entry {
    fn new_file(file: OpenState, shared: Arc<AsyncMutex<Shared>>) -> Entry {
        Entry::File(FileEntry {
            shared,
            file: AsyncMutex::new(file),
        })
    }

    fn new_dir(dir: JointDirectory, shared: Arc<AsyncMutex<Shared>>) -> Entry {
        Entry::Directory(DirEntry { shared, dir })
    }

    fn as_file(&self) -> Result<&FileEntry, Error> {
        match self {
            Entry::File(file_entry) => Ok(file_entry),
            Entry::Directory(_) => Err(STATUS_INVALID_DEVICE_REQUEST.into()),
        }
    }

    fn as_directory(&self) -> Result<&DirEntry, Error> {
        match self {
            Entry::File(_) => Err(STATUS_INVALID_DEVICE_REQUEST.into()),
            Entry::Directory(entry) => Ok(entry),
        }
    }

    fn shared(&self) -> &Arc<AsyncMutex<Shared>> {
        match self {
            Entry::File(entry) => &entry.shared,
            Entry::Directory(entry) => &entry.shared,
        }
    }
}

pub(crate) struct EntryHandle {
    id: u64,
    entry: Entry,
}

enum OpenState {
    Open(File),
    Lazy {
        path: Utf8PathBuf,
        create_disposition: CreateDisposition,
    },
    Closed,
}

impl OpenState {
    fn as_mut_file(&mut self) -> Option<&mut File> {
        match self {
            Self::Open(file) => Some(file),
            Self::Lazy { .. } => None,
            Self::Closed => None,
        }
    }

    async fn opened_file(&mut self, repo: &Repository) -> Result<&mut File, Error> {
        match self {
            Self::Open(file) => Ok(file),
            Self::Lazy {
                path,
                create_disposition,
            } => {
                use ouisync_lib::Error as E;
                let file = match repo.open_file(&path).await {
                    Ok(file) => file,
                    Err(E::EntryNotFound) => {
                        if !create_disposition.should_create() {
                            return Err(E::EntryNotFound.into());
                        }
                        repo.create_file(path).await?
                    }
                    Err(other) => {
                        return Err(other.into());
                    }
                };
                *self = OpenState::Open(file);
                Ok(self.as_mut_file().unwrap())
            }
            Self::Closed => Err(STATUS_FILE_CLOSED.into()),
        }
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

#[derive(Clone, Copy)]
struct AccessMask {
    mask: winnt::ACCESS_MASK,
}

impl AccessMask {
    fn has_delete(&self) -> bool {
        self.mask & winnt::DELETE > 0
    }
}

impl From<winnt::ACCESS_MASK> for AccessMask {
    fn from(mask: winnt::ACCESS_MASK) -> Self {
        Self { mask }
    }
}

impl From<AccessMask> for winnt::ACCESS_MASK {
    fn from(mask: AccessMask) -> Self {
        mask.mask
    }
}

impl fmt::Debug for AccessMask {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut mask = self.mask;
        let mut first = true;
        if mask & winnt::DELETE > 0 {
            first = false;
            write!(f, "DELETE")?;
            mask ^= winnt::DELETE;
        }
        if mask & winnt::READ_CONTROL > 0 {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            write!(f, "READ_CONTROL")?;
            mask ^= winnt::READ_CONTROL;
        }
        if mask & winnt::WRITE_DAC > 0 {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            write!(f, "WRITE_DAC")?;
            mask ^= winnt::WRITE_DAC;
        }
        if mask & winnt::WRITE_OWNER > 0 {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            write!(f, "WRITE_OWNER")?;
            mask ^= winnt::WRITE_OWNER;
        }
        if mask & winnt::SYNCHRONIZE > 0 {
            if !first {
                write!(f, "|")?;
            }
            first = false;
            write!(f, "SYNCHRONIZE")?;
            mask ^= winnt::SYNCHRONIZE;
        }
        if mask > 0 {
            if !first {
                write!(f, "|")?;
            }
            write!(f, "{:#x}", mask)?;
        }
        Ok(())
    }
}

pub(crate) fn default_mount_flags() -> MountFlags {
    // TODO: Check these flags.
    let flags = MountFlags::empty();
    //flags |= ALT_STREAM;
    //flags |= MountFlags::DEBUG | MountFlags::STDERR;
    //flags |= MountFlags::REMOVABLE;
    flags
}
