/// Implement `sqlx::Executor` for the given type by delegating it to the `Deref` target.
macro_rules! impl_executor_by_deref {
    ($type:ty) => {
        impl<'t> sqlx::Executor<'t> for &'t mut $type {
            type Database = sqlx::Sqlite;

            fn fetch_many<'e, 'q: 'e, E>(
                self,
                query: E,
            ) -> futures_util::stream::BoxStream<
                'e,
                Result<
                    either::Either<sqlx::sqlite::SqliteQueryResult, sqlx::sqlite::SqliteRow>,
                    sqlx::Error,
                >,
            >
            where
                't: 'e,
                E: sqlx::Execute<'q, sqlx::Sqlite> + 'q,
            {
                (&mut **self).fetch_many(query)
            }

            fn fetch_optional<'e, 'q: 'e, E>(
                self,
                query: E,
            ) -> futures_util::future::BoxFuture<
                'e,
                Result<Option<sqlx::sqlite::SqliteRow>, sqlx::Error>,
            >
            where
                't: 'e,
                E: sqlx::Execute<'q, sqlx::Sqlite> + 'q,
            {
                (&mut **self).fetch_optional(query)
            }

            fn prepare_with<'e, 'q: 'e>(
                self,
                sql: &'q str,
                parameters: &'e [sqlx::sqlite::SqliteTypeInfo],
            ) -> futures_util::future::BoxFuture<
                'e,
                Result<sqlx::sqlite::SqliteStatement<'q>, sqlx::Error>,
            >
            where
                't: 'e,
            {
                (&mut **self).prepare_with(sql, parameters)
            }

            #[doc(hidden)]
            fn describe<'e, 'q: 'e>(
                self,
                query: &'q str,
            ) -> futures_util::future::BoxFuture<
                'e,
                Result<sqlx::Describe<sqlx::Sqlite>, sqlx::Error>,
            >
            where
                't: 'e,
            {
                (&mut **self).describe(query)
            }
        }
    };
}
