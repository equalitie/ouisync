use std::{io, panic, path::Path};

use tokio::{
    fs::{self, OpenOptions},
    task,
};

/// Moves file from `src` to `dst`. If they are on the same filesystem, it does a simple rename.
/// Otherwise it copies `src` to `dst` first and then deletes `src`. This function makes best
/// effort to never overwrite `dst` if it already exists - instead it fails with `AlreadyExists`
/// error. On sufficiently modern platforms and filesystems (see below for details) it does it
/// atomically so it doesn't suffer from the [TOCTOU]
/// (https://en.wikipedia.org/wiki/Time-of-check_to_time-of-use) problem. Otherwise it falls back
/// to a non-atomic approach.
///
/// # Atomicity
///
/// The rename is atomic (and so guaranteed to never overwrite the destination file) on the
/// following platforms:
///
/// - Windows
/// - Linux and Android whose kernel version and filesystem support the `renameat2` syscall with the
///   `RENAME_NOREPLACE` flag. See the [renameat2]
///   (https://man7.org/linux/man-pages/man2/rename.2.html) manpage for more details.
///
/// Oh other platforms/filesystems the implementation falls back to a non-atomic approach where it
/// first checks if the destination exists and if it doesn't it proceeds with the rename. This
/// suffers from the above mentioned TOCTOU problem and so in specific circumstances can cause the
/// destination file to still be overwritten.
pub async fn safe_move(src: &Path, dst: &Path) -> io::Result<()> {
    // First try atomic rename
    match rename_no_replace_atomic(src, dst).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            // Src and dst are on different filesystems. Fallback to copy+remove.
            move_across_devices(src, dst).await
        }
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
            // Atomic rename not supported on this platform and/or filesystem. Fallback to the "best
            // effor" version.
            rename_no_replace_best_effort(src, dst).await
        }
        Err(error) => Err(error),
    }
}

// Renames `src` to `dst` but fails if `dst` already exists. This is guaranteed to never overwrite
// dst but it doesn't work on all platforms and/or filesystems.
async fn rename_no_replace_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    let src = src.to_owned();
    let dst = dst.to_owned();

    match task::spawn_blocking(move || blocking_rename_no_replace_atomic(&src, &dst)).await {
        Ok(result) => result,
        Err(error) => {
            if let Ok(panic) = error.try_into_panic() {
                panic::resume_unwind(panic)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "blocking task was cancelled",
                ))
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn blocking_rename_no_replace_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    use std::{
        ffi::{CString, c_char, c_int, c_uint},
        path::{self, PathBuf},
    };

    fn to_cstring(path: PathBuf) -> io::Result<CString> {
        CString::new(path.into_os_string().into_encoded_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
    }

    // In older linux/android distros the `renameat2` function is not available. Implement it by
    // invoking the raw syscall.
    unsafe extern "C" fn renameat2(
        olddirfd: c_int,
        oldpath: *const c_char,
        newdirfd: c_int,
        newpath: *const c_char,
        flags: c_uint,
    ) -> c_int {
        unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                olddirfd,
                oldpath,
                newdirfd,
                newpath,
                flags,
            ) as _
        }
    }

    let src = to_cstring(path::absolute(src)?)?;
    let dst = to_cstring(path::absolute(dst)?)?;

    // SAFETY: Both paths are valid and are passed in as pointers to valid 0-terminated C-style
    // strings.
    let result = unsafe {
        renameat2(
            0,
            src.as_ptr(),
            0,
            dst.as_ptr(),
            libc::RENAME_NOREPLACE as _,
        )
    };

    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "windows")]
fn blocking_rename_no_replace_atomic(src: &Path, dst: &Path) -> io::Result<()> {
    use std::{iter, os::windows::ffi::OsStrExt};

    fn to_cwstring(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(iter::once(0u16))
            .collect()
    }

    let src = to_cwstring(src);
    let dst = to_cwstring(dst);

    let result = unsafe { winapi::um::winbase::MoveFileExW(src.as_ptr(), dst.as_ptr(), 0) };

    if result != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// Renames `src` to `dst` but fails if `dst` already exists. This is not atomic so it's possible
// that the dst file might still get deleted if it's created concurrently with calling this
// function. This is used only as a fallback on platforms/filesystems which don't support the
// atomic rename.
async fn rename_no_replace_best_effort(src: &Path, dst: &Path) -> io::Result<()> {
    if !fs::try_exists(dst).await? {
        fs::rename(src, dst).await
    } else {
        Err(io::ErrorKind::AlreadyExists.into())
    }
}

async fn move_across_devices(src: &Path, dst: &Path) -> io::Result<()> {
    match copy_no_replace(src, dst).await {
        Ok(_) => {
            // Copy succeeeded - remove the source file.
            fs::remove_file(src).await?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            // Destination file already exists. Do nothing.
            Err(error)
        }
        Err(error) => {
            // Copy failed. Try to remove the partially copied destination file (if any) before
            // returning the error. If the removal fails, return the original error which is more
            // relevant.
            fs::remove_file(dst).await.ok();
            Err(error)
        }
    }
}

// Copies `src` to `dst` but fails if `dst` already exists.
async fn copy_no_replace(src: &Path, dst: &Path) -> io::Result<u64> {
    let mut src = OpenOptions::new().read(true).open(src).await?;
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .await?;

    tokio::io::copy(&mut src, &mut dst).await
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn rename_when_dst_does_not_exist() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::write(&src_path, content).await.unwrap();

        rename_no_replace_atomic(&src_path, &dst_path)
            .await
            .unwrap();

        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), content);
        assert!(!fs::try_exists(&src_path).await.unwrap());
    }

    #[tokio::test]
    async fn rename_when_dst_exists() {
        let temp_dir = TempDir::new().unwrap();

        let src_path = temp_dir.path().join("src");
        let src_content = "content of src";

        let dst_path = temp_dir.path().join("dst");
        let dst_content = "content of dst";

        fs::write(&src_path, src_content).await.unwrap();
        fs::write(&dst_path, dst_content).await.unwrap();

        match rename_no_replace_atomic(&src_path, &dst_path).await {
            Ok(()) => panic!("unexpected success"),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::AlreadyExists),
        }

        assert_eq!(fs::read_to_string(&src_path).await.unwrap(), src_content);
        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), dst_content);
    }

    #[tokio::test]
    async fn copy_when_dst_does_not_exists() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::write(&src_path, content).await.unwrap();

        copy_no_replace(&src_path, &dst_path).await.unwrap();

        assert_eq!(fs::read_to_string(&src_path).await.unwrap(), content);
        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), content);
    }

    #[tokio::test]
    async fn copy_when_dst_exists() {
        let temp_dir = TempDir::new().unwrap();

        let src_path = temp_dir.path().join("src");
        let src_content = "content of src";

        let dst_path = temp_dir.path().join("dst");
        let dst_content = "content of dst";

        fs::write(&src_path, src_content).await.unwrap();
        fs::write(&dst_path, dst_content).await.unwrap();

        match copy_no_replace(&src_path, &dst_path).await {
            Ok(_) => panic!("unexpected success"),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::AlreadyExists),
        }

        assert_eq!(fs::read_to_string(&src_path).await.unwrap(), src_content);
        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), dst_content);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn move_across_filesystems() {
        use std::os::unix::fs::MetadataExt;

        let src_dir = TempDir::new().unwrap();

        // `/dev/shm` is a filesystem in ram that is writable by all users and should be available
        // on most linux distros. This is not its intended use but it should work nevertheless.
        // (https://unix.stackexchange.com/questions/26364/how-can-i-create-a-tmpfs-as-a-regular-non-root-user)
        let dst_dir = TempDir::new_in("/dev/shm").unwrap();

        // Assert they are really on different filesystems.
        assert_ne!(
            fs::metadata(src_dir.path()).await.unwrap().dev(),
            fs::metadata(dst_dir.path()).await.unwrap().dev()
        );

        let src_path = src_dir.path().join("src");
        let dst_path = dst_dir.path().join("dst");
        let content = "hello from across the filesystem boundary";

        fs::write(&src_path, content).await.unwrap();

        safe_move(&src_path, &dst_path).await.unwrap();

        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), content);
        assert!(!fs::try_exists(&src_path).await.unwrap());
    }

    #[tokio::test]
    async fn attempt_to_move_to_subdir_of_src() {
        let temp_dir = TempDir::new().unwrap();

        let src_path = temp_dir.path().join("src");
        fs::create_dir_all(&src_path).await.unwrap();
        fs::write(src_path.join("file"), "hello world")
            .await
            .unwrap();

        let dst_path = src_path.join("dst");

        match safe_move(&src_path, &dst_path).await {
            Ok(_) => panic!("unexpected success"),
            Err(error) => assert_eq!(
                error.kind(),
                io::ErrorKind::InvalidInput,
                "unexpected error: {error:?}"
            ),
        }
    }
}
