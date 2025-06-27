use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};

use fs_util::WalkDir;
use futures_util::future;
use tokio::{fs, sync::mpsc};
use tokio_stream::StreamExt;

use crate::defaults;

const OLD_APP_ID: &str = "ouisync";

fn old_config_dirs() -> impl Iterator<Item = PathBuf> {
    dirs::config_dir()
        .map(|base| base.join(OLD_APP_ID))
        .into_iter()
}

fn old_store_dirs() -> impl Iterator<Item = PathBuf> {
    dirs::data_dir()
        .map(|base| base.join(OLD_APP_ID))
        .into_iter()
}

pub(crate) async fn migrate_config_dir() {
    let new = defaults::config_dir();
    for old in old_config_dirs() {
        run_migration(&old, &new, ConflictStrategy::KeepDst).await;
    }
}

pub(crate) async fn migrate_store_dir() {
    let new = defaults::store_dir();
    for old in old_store_dirs() {
        run_migration(&old, &new, ConflictStrategy::Rename).await;
    }
}

async fn run_migration(src: &Path, dst: &Path, conflict_strategy: ConflictStrategy) {
    let (report_tx, report_rx) = mpsc::channel(1);

    future::join(
        migrate_dir(src, dst, conflict_strategy, report_tx.clone()),
        log_reports(report_rx),
    )
    .await;
}

async fn log_reports(mut rx: mpsc::Receiver<MigrateReport>) {
    while let Some(report) = rx.recv().await {
        match report.result {
            Ok(()) => tracing::info!(
                "Migrated '{}' to '{}'",
                report.src.display(),
                report.dst.display()
            ),
            Err(error) => tracing::error!(
                "Failed to migrate '{}' to '{}': {}",
                report.src.display(),
                report.dst.display(),
                error
            ),
        }
    }
}

async fn migrate_dir(
    src: &Path,
    dst: &Path,
    conflict_strategy: ConflictStrategy,
    report_tx: mpsc::Sender<MigrateReport>,
) {
    let mut entries = WalkDir::new(src).contents_first(true).into_stream();

    while let Some(entry) = entries.next().await {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error)
                if error.io_error().map(|error| error.kind()) == Some(io::ErrorKind::NotFound) =>
            {
                // src missing, skip it
                continue;
            }
            Err(error) => {
                report_tx
                    .send(MigrateReport::err(
                        src,
                        dst,
                        format!("failed to open src: {error}"),
                    ))
                    .await
                    .ok();
                continue;
            }
        };

        let src_entry_path = entry.path();
        let src_entry_path_rel = match src_entry_path.strip_prefix(src) {
            Ok(path) => path,
            Err(_) => {
                report_tx
                    .send(MigrateReport::err(src_entry_path, dst, "invalid src path"))
                    .await
                    .ok();
                continue;
            }
        };
        let dst_entry_path = dst.join(src_entry_path_rel);

        if entry.file_type().is_dir() {
            match fs::remove_dir(src_entry_path).await {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => (),
                Err(error) => {
                    report_tx
                        .send(MigrateReport::err(
                            src_entry_path,
                            dst_entry_path,
                            format!("failed to remove src: {error}"),
                        ))
                        .await
                        .ok();
                }
            }
            continue;
        }

        if let Some(parent) = dst_entry_path.parent() {
            match fs::create_dir_all(parent).await {
                Ok(()) => (),
                Err(error) => {
                    report_tx
                        .send(MigrateReport::err(
                            src_entry_path,
                            dst_entry_path,
                            format!("failed to create dst parent directory: {error}"),
                        ))
                        .await
                        .ok();
                    continue;
                }
            }
        }

        let mut dst_entry_path = dst_entry_path;

        loop {
            match fs_util::safe_move(src_entry_path, &dst_entry_path).await {
                Ok(()) => {
                    report_tx
                        .send(MigrateReport::ok(src_entry_path, &dst_entry_path))
                        .await
                        .ok();
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match conflict_strategy {
                        ConflictStrategy::KeepDst => {
                            report_tx
                                .send(MigrateReport::err(
                                    src_entry_path,
                                    &dst_entry_path,
                                    "dst already exists",
                                ))
                                .await
                                .ok();
                            break;
                        }
                        ConflictStrategy::Rename => {
                            dst_entry_path = next_path(&dst_entry_path);
                            continue;
                        }
                    }
                }
                Err(error) => {
                    report_tx
                        .send(MigrateReport::err(
                            src_entry_path,
                            &dst_entry_path,
                            format!("failed to move file: {error}"),
                        ))
                        .await
                        .ok();
                    continue;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ConflictStrategy {
    KeepDst,
    Rename,
}

struct MigrateReport {
    src: PathBuf,
    dst: PathBuf,
    result: Result<(), String>,
}

impl MigrateReport {
    fn ok(src: impl Into<PathBuf>, dst: impl Into<PathBuf>) -> Self {
        Self {
            src: src.into(),
            dst: dst.into(),
            result: Ok(()),
        }
    }

    fn err(src: impl Into<PathBuf>, dst: impl Into<PathBuf>, error: impl Into<String>) -> Self {
        Self {
            src: src.into(),
            dst: dst.into(),
            result: Err(error.into()),
        }
    }
}

fn next_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or(OsStr::new(""));
    let stem_str = stem.to_string_lossy();

    let parts = stem_str
        .rsplit_once('-')
        .and_then(|(prefix, suffix)| suffix.parse::<u64>().ok().map(|seq| (prefix, seq)));

    let stem = if let Some((prefix, seq)) = parts {
        format!("{prefix}-{}", seq + 1).into()
    } else {
        let mut stem = stem.to_os_string();
        stem.push("-1");
        stem
    };

    let output = if let Some(parent) = path.parent() {
        parent.join(stem)
    } else {
        PathBuf::from(stem)
    };

    if let Some(extension) = path.extension() {
        output.with_extension(extension)
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tokio::fs;
    use tokio_stream::wrappers::ReceiverStream;

    use super::*;

    #[tokio::test]
    async fn migrate_src_missing() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");

        let reports =
            migrate_and_collect_reports(&src_path, &dst_path, ConflictStrategy::Rename).await;
        assert!(reports.is_empty());

        assert!(!fs::try_exists(src_path).await.unwrap());
        assert!(!fs::try_exists(dst_path).await.unwrap());
    }

    #[tokio::test]
    async fn migrate_file() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::create_dir_all(&src_path).await.unwrap();
        fs::write(src_path.join("file.txt"), content).await.unwrap();

        let reports =
            migrate_and_collect_reports(&src_path, &dst_path, ConflictStrategy::Rename).await;

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].src, src_path.join("file.txt"));
        assert_eq!(reports[0].dst, dst_path.join("file.txt"));
        assert_eq!(reports[0].result, Ok(()));

        assert_eq!(
            fs::read_to_string(dst_path.join("file.txt")).await.unwrap(),
            content
        );
        assert!(!fs::try_exists(src_path).await.unwrap());
    }

    #[tokio::test]
    async fn migrate_subdirectory() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");

        let path_a = "subdir/a.txt";
        let content_a = "content a";

        let path_b = "b.txt";
        let content_b = "content b";

        fs::create_dir_all(src_path.join(path_a).parent().unwrap())
            .await
            .unwrap();
        fs::write(src_path.join(path_a), content_a).await.unwrap();

        fs::create_dir_all(src_path.join(path_b).parent().unwrap())
            .await
            .unwrap();
        fs::write(src_path.join(path_b), content_b).await.unwrap();

        let mut reports =
            migrate_and_collect_reports(&src_path, &dst_path, ConflictStrategy::Rename).await;
        reports.sort_by(|a, b| a.src.cmp(&b.src));

        assert_eq!(reports.len(), 2);

        assert_eq!(reports[0].src, src_path.join(path_b));
        assert_eq!(reports[0].dst, dst_path.join(path_b));
        assert_eq!(reports[0].result, Ok(()));

        assert_eq!(reports[1].src, src_path.join(path_a));
        assert_eq!(reports[1].dst, dst_path.join(path_a));
        assert_eq!(reports[1].result, Ok(()));

        assert_eq!(
            fs::read_to_string(dst_path.join(path_a)).await.unwrap(),
            content_a
        );
        assert_eq!(
            fs::read_to_string(dst_path.join(path_b)).await.unwrap(),
            content_b
        );
        assert!(!fs::try_exists(src_path).await.unwrap());
    }

    #[tokio::test]
    async fn migrate_conflict_keep_dst() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");

        let src_content = "src content";
        let dst_content = "dst content";

        fs::create_dir_all(&src_path).await.unwrap();
        fs::write(src_path.join("file.txt"), src_content)
            .await
            .unwrap();

        fs::create_dir_all(&dst_path).await.unwrap();
        fs::write(dst_path.join("file.txt"), dst_content)
            .await
            .unwrap();

        let reports =
            migrate_and_collect_reports(&src_path, &dst_path, ConflictStrategy::KeepDst).await;

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].src, src_path.join("file.txt"));
        assert_eq!(reports[0].dst, dst_path.join("file.txt"));
        assert!(reports[0].result.is_err());

        assert_eq!(
            fs::read_to_string(src_path.join("file.txt")).await.unwrap(),
            src_content,
        );
        assert_eq!(
            fs::read_to_string(dst_path.join("file.txt")).await.unwrap(),
            dst_content,
        );
    }

    #[tokio::test]
    async fn migrate_conflict_rename() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");

        let src_content = "src content";
        let dst_content = "dst content";

        fs::create_dir_all(&src_path).await.unwrap();
        fs::write(src_path.join("file.txt"), src_content)
            .await
            .unwrap();

        fs::create_dir_all(&dst_path).await.unwrap();
        fs::write(dst_path.join("file.txt"), dst_content)
            .await
            .unwrap();

        let reports =
            migrate_and_collect_reports(&src_path, &dst_path, ConflictStrategy::Rename).await;

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].src, src_path.join("file.txt"));
        assert_eq!(reports[0].dst, dst_path.join("file-1.txt"));
        assert_eq!(reports[0].result, Ok(()));

        assert_eq!(
            fs::read_to_string(dst_path.join("file.txt")).await.unwrap(),
            dst_content,
        );
        assert_eq!(
            fs::read_to_string(dst_path.join("file-1.txt"))
                .await
                .unwrap(),
            src_content,
        );
        assert!(!fs::try_exists(src_path).await.unwrap());
    }

    #[test]
    fn next_path_sanity_check() {
        assert_eq!(next_path(Path::new("foo")), Path::new("foo-1"));
        assert_eq!(next_path(Path::new("foo.txt")), Path::new("foo-1.txt"));
        assert_eq!(next_path(Path::new("foo-1.txt")), Path::new("foo-2.txt"));
        assert_eq!(next_path(Path::new("foo-10.txt")), Path::new("foo-11.txt"));
        assert_eq!(
            next_path(Path::new("foo/bar-42.txt")),
            Path::new("foo/bar-43.txt")
        );
        assert_eq!(next_path(Path::new("")), Path::new("-1"));
        assert_eq!(
            next_path(Path::new("foo-bar.txt")),
            Path::new("foo-bar-1.txt")
        );
    }

    async fn migrate_and_collect_reports(
        src: &Path,
        dst: &Path,
        conflict_strategy: ConflictStrategy,
    ) -> Vec<MigrateReport> {
        let (tx, rx) = mpsc::channel(1);
        let (_, reports) = future::join(
            migrate_dir(src, dst, conflict_strategy, tx),
            ReceiverStream::new(rx).collect(),
        )
        .await;

        reports
    }
}
