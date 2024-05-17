use super::{EntryHandle, EntryIdGenerator, VirtualFilesystem};
use dokan::{
    init, shutdown, unmount, CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler,
    FileSystemMounter, FileTimeOperation, FillDataResult, FindData, MountOptions, OperationInfo,
    OperationResult, VolumeInfo, IO_SECURITY_CONTEXT,
};
use ouisync_lib::Repository;
use std::io;
use std::{
    path::Path,
    sync::{mpsc, Arc},
    thread,
};
use tracing::span;
use widestring::{U16CStr, U16CString};
use winapi::um::winnt;

struct SingleRepoVFS {
    vfs: VirtualFilesystem,
    span: Option<tracing::Span>,
}

impl SingleRepoVFS {
    fn enter_span(&self) -> Option<span::Entered<'_>> {
        if let Some(span) = &self.span {
            Some(span.enter())
        } else {
            None
        }
    }
}

//  https://dokan-dev.github.io/dokany-doc/html/struct_d_o_k_a_n___o_p_e_r_a_t_i_o_n_s.html
impl<'c, 'h: 'c> FileSystemHandler<'c, 'h> for SingleRepoVFS {
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
        _info: &mut OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<CreateFileInfo<Self::Context>> {
        let _span_guard = self.enter_span();

        self.vfs.create_file(
            file_name,
            security_context,
            desired_access,
            file_attributes,
            share_access,
            create_disposition,
            create_options,
        )
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
        let _span_guard = self.enter_span();
        self.vfs.close_file(file_name, info, context)
    }

    fn read_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &mut [u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        let _span_guard = self.enter_span();
        self.vfs.read_file(file_name, offset, buffer, info, context)
    }

    fn write_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        buffer: &[u8],
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<u32> {
        let _span_guard = self.enter_span();
        self.vfs
            .write_file(file_name, offset, buffer, info, context)
    }

    fn flush_file_buffers(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.flush_file_buffers(file_name, info, context)
    }

    fn get_file_information(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<FileInfo> {
        let _span_guard = self.enter_span();
        self.vfs.get_file_information(file_name, info, context)
    }

    fn find_files(
        &'h self,
        file_name: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs
            .find_files(file_name, fill_find_data, info, context)
    }

    fn find_files_with_pattern(
        &'h self,
        file_name: &U16CStr,
        pattern: &U16CStr,
        fill_find_data: impl FnMut(&FindData) -> FillDataResult,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs
            .find_files_with_pattern(file_name, pattern, fill_find_data, info, context)
    }

    fn set_file_attributes(
        &'h self,
        file_name: &U16CStr,
        file_attributes: u32,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs
            .set_file_attributes(file_name, file_attributes, info, context)
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
        let _span_guard = self.enter_span();
        self.vfs.set_file_time(
            file_name,
            creation_time,
            last_access_time,
            last_write_time,
            info,
            context,
        )
    }

    fn delete_file(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.delete_file(file_name, info, context)
    }

    fn delete_directory(
        &'h self,
        file_name: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.delete_directory(file_name, info, context)
    }

    fn move_file(
        &'h self,
        file_name: &U16CStr,
        new_file_name: &U16CStr,
        replace_if_existing: bool,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs
            .move_file(file_name, new_file_name, replace_if_existing, info, context)
    }

    fn set_end_of_file(
        &'h self,
        file_name: &U16CStr,
        offset: i64,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.set_end_of_file(file_name, offset, info, context)
    }

    fn set_allocation_size(
        &'h self,
        file_name: &U16CStr,
        alloc_size: i64,
        info: &OperationInfo<'c, 'h, Self>,
        context: &'c Self::Context,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs
            .set_allocation_size(file_name, alloc_size, info, context)
    }

    fn get_disk_free_space(
        &'h self,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<DiskSpaceInfo> {
        let _span_guard = self.enter_span();
        self.vfs.get_disk_free_space(info)
    }

    fn get_volume_information(
        &'h self,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<VolumeInfo> {
        let _span_guard = self.enter_span();
        self.vfs.get_volume_information(info)
    }

    fn mounted(
        &'h self,
        mount_point: &U16CStr,
        info: &OperationInfo<'c, 'h, Self>,
    ) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.mounted(mount_point, info)
    }

    fn unmounted(&'h self, info: &OperationInfo<'c, 'h, Self>) -> OperationResult<()> {
        let _span_guard = self.enter_span();
        self.vfs.unmounted(info)
    }
}

pub fn mount(
    runtime_handle: tokio::runtime::Handle,
    repository: Arc<Repository>,
    mount_point: impl AsRef<Path>,
) -> Result<MountGuard, io::Error> {
    mount_with_span(runtime_handle, repository, mount_point, None)
}

pub fn mount_with_span(
    runtime_handle: tokio::runtime::Handle,
    repository: Arc<Repository>,
    mount_point: impl AsRef<Path>,
    span: Option<tracing::Span>,
) -> Result<MountGuard, io::Error> {
    let options = MountOptions {
        single_thread: false,
        flags: super::default_mount_flags(),
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

    let (on_mount_tx, on_mount_rx) = mpsc::sync_channel(0);
    let (unmount_tx, unmount_rx) = mpsc::sync_channel(1);

    let join_handle = thread::spawn(move || {
        // TODO: Ensure this is done only once.
        init();

        let handler = SingleRepoVFS {
            vfs: VirtualFilesystem::new(
                runtime_handle,
                Arc::new(EntryIdGenerator::new()),
                repository,
            ),
            span,
        };
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

    on_mount_rx.recv().unwrap()?;

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
