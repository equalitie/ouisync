use std::{
    env, io,
    path::{Path, PathBuf},
};

use fs_util::WalkDir;
use futures_util::future;
use tokio::{fs, sync::mpsc};
use tokio_stream::StreamExt;

use crate::defaults;

const OLD_APP_ID: &str = "ouisync";

/// Migrate config files from the old config directory to the new one.
pub(crate) async fn migrate_config_dir() {
    let new = defaults::config_dir();
    let Some(old) = dirs::config_dir().map(|base| base.join(OLD_APP_ID)) else {
        return;
    };

    let (report_tx, report_rx) = mpsc::channel(1);

    future::join(migrate_dir(&old, &new, report_tx), log_reports(report_rx)).await;
}

/// Check whether the old store directory still exists and if so, print a warning.
pub(crate) async fn check_store_dir(new: &[PathBuf]) {
    let Some(old) = dirs::data_dir().map(|base| base.join(OLD_APP_ID)) else {
        return;
    };

    if new.contains(&old) {
        return;
    }

    let Some(new) = new.first() else {
        return;
    };

    if !fs::metadata(&old)
        .await
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
    {
        return;
    }

    let exe_name = env::current_exe()
        .ok()
        .as_deref()
        .and_then(|s| s.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("ouisync")
        .to_owned();

    tracing::warn!(
        "The default store directory was changed in order to unify the CLI and GUI variants of \
         Ouisync. The new directory is '{1}' but the old one at '{0}' still exists. \
         No repositories from the old directory will be loaded. To silence this warning move all \
         files from the old directory to the new one and delete the old one. Alternatively, to \
         keep using the old directory, add it to the current store directories by running the command \
         '{2} store-dirs insert {0}'",
        old.display(),
        new.display(),
        exe_name,
    );
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

async fn migrate_dir(src: &Path, dst: &Path, report_tx: mpsc::Sender<MigrateReport>) {
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

        match fs_util::safe_move(src_entry_path, &dst_entry_path).await {
            Ok(()) => {
                report_tx
                    .send(MigrateReport::ok(src_entry_path, &dst_entry_path))
                    .await
                    .ok();
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
            }
        }
    }
}

#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use tokio::fs;
    use tokio_stream::wrappers::ReceiverStream;

    use super::*;

    #[tokio::test]
    async fn src_missing() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");

        let reports = migrate_and_collect_reports(&src_path, &dst_path).await;
        assert!(reports.is_empty());

        assert!(!fs::try_exists(src_path).await.unwrap());
        assert!(!fs::try_exists(dst_path).await.unwrap());
    }

    #[tokio::test]
    async fn file() {
        let temp_dir = TempDir::new().unwrap();
        let src_path = temp_dir.path().join("src");
        let dst_path = temp_dir.path().join("dst");
        let content = "hello world";

        fs::create_dir_all(&src_path).await.unwrap();
        fs::write(src_path.join("file.txt"), content).await.unwrap();

        let reports = migrate_and_collect_reports(&src_path, &dst_path).await;

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
    async fn subdirectory() {
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

        let mut reports = migrate_and_collect_reports(&src_path, &dst_path).await;
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
    async fn conflict() {
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

        let reports = migrate_and_collect_reports(&src_path, &dst_path).await;

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

    async fn migrate_and_collect_reports(src: &Path, dst: &Path) -> Vec<MigrateReport> {
        let (tx, rx) = mpsc::channel(1);
        let (_, reports) =
            future::join(migrate_dir(src, dst, tx), ReceiverStream::new(rx).collect()).await;

        reports
    }
}
