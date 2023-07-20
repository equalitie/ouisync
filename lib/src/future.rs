//! Future and Stream combinators// TODO: move this to some generic utils module.

use futures_util::{Stream, TryStreamExt};
use std::{future, iter};

pub(crate) async fn try_collect_into<S, D, T, E>(src: S, dst: &mut D) -> Result<(), E>
where
    S: Stream<Item = Result<T, E>>,
    D: Extend<T>,
{
    src.try_for_each(|item| {
        dst.extend(iter::once(item));
        future::ready(Ok(()))
    })
    .await
}
