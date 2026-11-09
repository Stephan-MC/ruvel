use crate::database::error::DatabaseError;
use sqlx::{MySqlPool, SqlitePool};

#[derive(Clone)]
pub enum DatabaseConnection {
    Sqlite(SqlitePool),
    MySql(MySqlPool),
}

impl DatabaseConnection {
    pub async fn connect_sqlite(url: &str) -> Result<Self, DatabaseError> {
        let pool = SqlitePool::connect(url).await?;
        Ok(DatabaseConnection::Sqlite(pool))
    }

    pub async fn connect_mysql(url: &str) -> Result<Self, DatabaseError> {
        let pool = MySqlPool::connect(url).await?;
        Ok(DatabaseConnection::MySql(pool))
    }

    pub async fn connect_mariadb(url: &str) -> Result<Self, DatabaseError> {
        Self::connect_mysql(url).await
    }
}
