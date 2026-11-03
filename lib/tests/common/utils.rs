use camino::{Utf8Path, Utf8PathBuf};
use ouisync::{EntryType, Error, Repository};
use ouisync_rand_test_dir as rtd;
use rand::Rng;

/// Copy the content of `src` into `dst`.
pub(crate) async fn copy(src: &rtd::File, dst: &mut ouisync::File) {
    use std::io::Read;

    let mut buffer = vec![0; 8 * 1024];
    let mut reader = src.reader();

    loop {
        // NOTE: this `read` never blocks so it's ok to use it in an `async` function.
        let n = reader.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }

        dst.write_all(&buffer[..n]).await.unwrap();
    }
}

/// Copy the whole directory content into the repository.
pub(crate) async fn copy_all(src: &rtd::Directory, dst: &Repository) {
    for (path, entry) in src.walk() {
        let path = Utf8Path::new(&path);

        match entry {
            rtd::Entry::File(src_file) => {
                let mut dst_file = dst.create_file(path).await.unwrap();
                copy(src_file, &mut dst_file).await;
                dst_file.flush().await.unwrap();
            }
            rtd::Entry::Directory(_) => {
                dst.create_directory(path).await.unwrap();
            }
        }
    }
}

pub(crate) async fn collect_entries(
    repo: &Repository,
) -> Result<Vec<(Utf8PathBuf, EntryType)>, Error> {
    let mut output = Vec::new();
    let mut stack = vec![Utf8PathBuf::from("")];

    while let Some(parent_path) = stack.pop() {
        let mut parent_dir = repo.open_directory(&parent_path).await?;

        for entry in parent_dir.entries() {
            let path = parent_path.join(entry.name());

            if matches!(entry.entry_type(), EntryType::Directory) {
                stack.push(path.clone());
            }

            output.push((path, entry.entry_type()));
        }
    }

    Ok(output)
}
