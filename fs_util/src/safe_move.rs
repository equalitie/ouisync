use std::{io, panic, path::Path};

use tokio::{
    fs::{self, OpenOptions},
    task,
};

/// Moves file from `src` to `dst`. If they are on the same filesystem, it does a simple rename.
/// Otherwise it copies `src` to `dst` first and then deletes `src`. Also this function never
/// overwrite `dst` if it already exists - instead it fails with `AlreadyExists` error. It does it
/// atomically so it doesn't suffer from the [TOCTOU]
/// (https://en.wikipedia.org/wiki/Time-of-check_to_time-of-use) problem.
pub async fn safe_move(src: &Path, dst: &Path) -> io::Result<()> {
    // First try rename
    match safe_rename(src, dst).await {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => (),
        Err(error) => return Err(error),
    }

    // `src` and `dst` are on different filesystems, fall back to copy + remove.
    safe_copy(src, dst).await?;
    fs::remove_file(src).await?;

    Ok(())
}

// Renames `src` to `dst` but fails if `dst` already exists.
async fn safe_rename(src: &Path, dst: &Path) -> io::Result<()> {
    let src = src.to_owned();
    let dst = dst.to_owned();

    match task::spawn_blocking(move || blocking_safe_rename(&src, &dst)).await {
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
fn blocking_safe_rename(src: &Path, dst: &Path) -> io::Result<()> {
    use std::{
        ffi::CString,
        path::{self, PathBuf},
    };

    fn to_cstring(path: PathBuf) -> io::Result<CString> {
        CString::new(path.into_os_string().into_encoded_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))
    }

    let src = to_cstring(path::absolute(src)?)?;
    let dst = to_cstring(path::absolute(dst)?)?;

    // SAFETY: Both paths are valid and are passed in as pointers to valid 0-terminated C-style
    // strings.
    let result = unsafe {
        libc::renameat2(
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
fn blocking_safe_rename(src: &Path, dst: &Path) -> io::Result<()> {
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

// Copies `src` to `dst` but fails if `dst` already exists.
async fn safe_copy(src: &Path, dst: &Path) -> io::Result<u64> {
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
    async fn safe_rename_when_dst_does_not_exist() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::write(&src_path, content).await.unwrap();

        safe_rename(&src_path, &dst_path).await.unwrap();

        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), content);
        assert!(!fs::try_exists(&src_path).await.unwrap());
    }

    #[tokio::test]
    async fn safe_rename_when_dst_exists() {
        let temp_dir = TempDir::new().unwrap();

        let src_path = temp_dir.path().join("src");
        let src_content = "content of src";

        let dst_path = temp_dir.path().join("dst");
        let dst_content = "content of dst";

        fs::write(&src_path, src_content).await.unwrap();
        fs::write(&dst_path, dst_content).await.unwrap();

        match safe_rename(&src_path, &dst_path).await {
            Ok(()) => panic!("unexpected success"),
            Err(error) => assert_eq!(error.kind(), io::ErrorKind::AlreadyExists),
        }

        assert_eq!(fs::read_to_string(&src_path).await.unwrap(), src_content);
        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), dst_content);
    }

    #[tokio::test]
    async fn safe_copy_when_dst_does_not_exists() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::write(&src_path, content).await.unwrap();

        safe_copy(&src_path, &dst_path).await.unwrap();

        assert_eq!(fs::read_to_string(&src_path).await.unwrap(), content);
        assert_eq!(fs::read_to_string(&dst_path).await.unwrap(), content);
    }

    #[tokio::test]
    async fn safe_copy_when_dst_exists() {
        let temp_dir = TempDir::new().unwrap();

        let src_path = temp_dir.path().join("src");
        let src_content = "content of src";

        let dst_path = temp_dir.path().join("dst");
        let dst_content = "content of dst";

        fs::write(&src_path, src_content).await.unwrap();
        fs::write(&dst_path, dst_content).await.unwrap();

        match safe_copy(&src_path, &dst_path).await {
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
}
