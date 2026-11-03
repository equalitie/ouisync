use std::{fmt, future};

use ouisync::{Event, Repository, VersionVector};
use tokio::{select, sync::broadcast};
use tracing::{Instrument, Span};

/// Observes a repository for changes and prints repository statistics to the logs.
pub(crate) struct RepositoryObserver<'a> {
    repo: &'a Repository,
    span: Span,
}

impl<'a> RepositoryObserver<'a> {
    pub fn new(repo: &'a Repository) -> Self {
        let span = Span::current();

        Self { repo, span }
    }

    pub async fn run(&self) {
        let mut repo_rx = self.repo.subscribe();

        loop {
            self.report().await;

            match repo_rx.recv().await {
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => (),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    pub async fn run_until<F: IntoFuture>(&self, f: F) -> F::Output {
        select! {
            result = f => result,
            _ = async {
                self.run().await;
                future::pending().await
            } => unreachable!(),
        }
    }

    async fn report(&self) {
        let progress = self.repo.sync_progress().await.unwrap();
        let vv = self.repo.get_merged_version_vector().await.unwrap();

        tracing::info!(
            target: "ouisync-test",
            parent: &self.span,
            "progress: {}, blocks: {}/{}, vv: {}",
            progress.percent(),
            progress.value,
            progress.total,
            VersionVectorDisplay(&vv),
        )
    }
}

struct VersionVectorDisplay<'a>(&'a VersionVector);

impl fmt::Display for VersionVectorDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for (index, (_, version)) in self.0.entries().enumerate() {
            if index > 0 {
                write!(f, ", ")?;
            }

            write!(f, "{version}")?;
        }
        write!(f, "]")?;

        Ok(())
    }
}
