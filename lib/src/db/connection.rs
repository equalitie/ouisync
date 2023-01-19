use either::Either;
use futures_util::{future::BoxFuture, stream::BoxStream};
use ref_cast::RefCast;
use sqlx::{
    sqlite::{SqliteConnection, SqliteQueryResult, SqliteRow, SqliteStatement, SqliteTypeInfo},
    Describe, Error, Execute, Executor, Sqlite,
};

/// Database connection
#[derive(Debug, RefCast)]
#[repr(transparent)]
pub(crate) struct Connection(pub(super) SqliteConnection);

impl<'t> Executor<'t> for &'t mut Connection {
    type Database = Sqlite;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<SqliteQueryResult, SqliteRow>, Error>>
    where
        't: 'e,
        E: Execute<'q, Sqlite>,
    {
        self.0.fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<SqliteRow>, Error>>
    where
        't: 'e,
        E: Execute<'q, Sqlite>,
    {
        self.0.fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [SqliteTypeInfo],
    ) -> BoxFuture<'e, Result<SqliteStatement<'q>, Error>>
    where
        't: 'e,
    {
        self.0.prepare_with(sql, parameters)
    }

    #[doc(hidden)]
    fn describe<'e, 'q: 'e>(self, query: &'q str) -> BoxFuture<'e, Result<Describe<Sqlite>, Error>>
    where
        't: 'e,
    {
        self.0.describe(query)
    }
}
