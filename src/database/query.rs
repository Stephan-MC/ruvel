use crate::database::connection::DatabaseConnection;
use crate::database::error::DatabaseError;
use crate::database::manager::DB;
use serde::de::DeserializeOwned;
use sqlx::{Column, Row};
use std::marker::PhantomData;

#[derive(Debug, Clone)]
pub enum QueryValue {
    Null,
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
}

impl QueryValue {
    pub fn from_value(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => QueryValue::Null,
            serde_json::Value::Bool(b) => QueryValue::Bool(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    QueryValue::Int(i)
                } else if let Some(f) = n.as_f64() {
                    QueryValue::Float(f)
                } else {
                    QueryValue::String(n.to_string())
                }
            }
            serde_json::Value::String(s) => QueryValue::String(s.clone()),
            other => QueryValue::String(other.to_string()),
        }
    }
}

impl<T: ToString> From<T> for QueryValue {
    fn from(val: T) -> Self {
        QueryValue::String(val.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct WhereClause {
    pub column: String,
    pub operator: String,
    pub value: QueryValue,
}

pub struct QueryBuilder<T> {
    table: String,
    primary_key: String,
    connection_name: Option<String>,
    wheres: Vec<WhereClause>,
    order_by: Option<(String, String)>,
    limit: Option<usize>,
    offset: Option<usize>,
    _phantom: PhantomData<T>,
}

impl<T> QueryBuilder<T> {
    pub fn new(table: impl Into<String>, primary_key: impl Into<String>, connection_name: Option<String>) -> Self {
        Self {
            table: table.into(),
            primary_key: primary_key.into(),
            connection_name,
            wheres: Vec::new(),
            order_by: None,
            limit: None,
            offset: None,
            _phantom: PhantomData,
        }
    }

    pub fn where_eq(mut self, column: impl Into<String>, value: impl Into<QueryValue>) -> Self {
        self.wheres.push(WhereClause {
            column: column.into(),
            operator: "=".into(),
            value: value.into(),
        });
        self
    }

    pub fn where_clause(
        mut self,
        column: impl Into<String>,
        operator: impl Into<String>,
        value: impl Into<QueryValue>,
    ) -> Self {
        self.wheres.push(WhereClause {
            column: column.into(),
            operator: operator.into(),
            value: value.into(),
        });
        self
    }

    pub fn order_by(mut self, column: impl Into<String>, direction: impl Into<String>) -> Self {
        self.order_by = Some((column.into(), direction.into()));
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    fn connection(&self) -> Result<DatabaseConnection, DatabaseError> {
        DB::connection(self.connection_name.as_deref())
    }

    fn build_select_sql(&self) -> (String, Vec<QueryValue>) {
        let mut sql = format!("SELECT * FROM {}", self.table);
        let mut params = Vec::new();

        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            let clauses: Vec<String> = self
                .wheres
                .iter()
                .map(|w| {
                    params.push(w.value.clone());
                    format!("{} {} ?", w.column, w.operator)
                })
                .collect();
            sql.push_str(&clauses.join(" AND "));
        }

        if let Some((col, dir)) = &self.order_by {
            sql.push_str(&format!(" ORDER BY {} {}", col, dir));
        }

        if let Some(limit) = self.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        (sql, params)
    }

    fn build_count_sql(&self) -> (String, Vec<QueryValue>) {
        let mut sql = format!("SELECT COUNT(*) as count FROM {}", self.table);
        let mut params = Vec::new();

        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            let clauses: Vec<String> = self
                .wheres
                .iter()
                .map(|w| {
                    params.push(w.value.clone());
                    format!("{} {} ?", w.column, w.operator)
                })
                .collect();
            sql.push_str(&clauses.join(" AND "));
        }

        (sql, params)
    }
}

fn row_to_json_sqlite(row: &sqlx::sqlite::SqliteRow) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for col in row.columns() {
        let name = col.name();
        // Try getting as basic types
        if let Ok(val) = row.try_get::<i64, _>(name) {
            map.insert(name.to_string(), serde_json::Value::Number(val.into()));
        } else if let Ok(val) = row.try_get::<f64, _>(name) {
            if let Some(num) = serde_json::Number::from_f64(val) {
                map.insert(name.to_string(), serde_json::Value::Number(num));
            } else {
                map.insert(name.to_string(), serde_json::Value::Null);
            }
        } else if let Ok(val) = row.try_get::<String, _>(name) {
            map.insert(name.to_string(), serde_json::Value::String(val));
        } else if let Ok(val) = row.try_get::<bool, _>(name) {
            map.insert(name.to_string(), serde_json::Value::Bool(val));
        } else {
            map.insert(name.to_string(), serde_json::Value::Null);
        }
    }
    serde_json::Value::Object(map)
}

fn row_to_json_mysql(row: &sqlx::mysql::MySqlRow) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for col in row.columns() {
        let name = col.name();
        if let Ok(val) = row.try_get::<i64, _>(name) {
            map.insert(name.to_string(), serde_json::Value::Number(val.into()));
        } else if let Ok(val) = row.try_get::<f64, _>(name) {
            if let Some(num) = serde_json::Number::from_f64(val) {
                map.insert(name.to_string(), serde_json::Value::Number(num));
            } else {
                map.insert(name.to_string(), serde_json::Value::Null);
            }
        } else if let Ok(val) = row.try_get::<String, _>(name) {
            map.insert(name.to_string(), serde_json::Value::String(val));
        } else if let Ok(val) = row.try_get::<bool, _>(name) {
            map.insert(name.to_string(), serde_json::Value::Bool(val));
        } else {
            map.insert(name.to_string(), serde_json::Value::Null);
        }
    }
    serde_json::Value::Object(map)
}

impl<T: DeserializeOwned + Send + Unpin> QueryBuilder<T> {
    pub async fn get(&self) -> Result<Vec<T>, DatabaseError> {
        let (sql, params) = self.build_select_sql();
        let conn = self.connection()?;

        let mut results = Vec::new();
        match conn {
            DatabaseConnection::Sqlite(pool) => {
                let mut query = sqlx::query(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let rows = query.fetch_all(&pool).await?;
                for row in rows {
                    let json_val = row_to_json_sqlite(&row);
                    let item: T = serde_json::from_value(json_val)?;
                    results.push(item);
                }
            }
            DatabaseConnection::MySql(pool) => {
                let mut query = sqlx::query(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let rows = query.fetch_all(&pool).await?;
                for row in rows {
                    let json_val = row_to_json_mysql(&row);
                    let item: T = serde_json::from_value(json_val)?;
                    results.push(item);
                }
            }
        }

        Ok(results)
    }

    pub async fn first(&self) -> Result<Option<T>, DatabaseError> {
        let limited = QueryBuilder {
            table: self.table.clone(),
            primary_key: self.primary_key.clone(),
            connection_name: self.connection_name.clone(),
            wheres: self.wheres.clone(),
            order_by: self.order_by.clone(),
            limit: Some(1),
            offset: self.offset,
            _phantom: PhantomData,
        };
        let list = limited.get().await?;
        Ok(list.into_iter().next())
    }

    pub async fn count(&self) -> Result<i64, DatabaseError> {
        let (sql, params) = self.build_count_sql();
        let conn = self.connection()?;

        match conn {
            DatabaseConnection::Sqlite(pool) => {
                let mut query = sqlx::query_scalar::<_, i64>(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let count = query.fetch_one(&pool).await?;
                Ok(count)
            }
            DatabaseConnection::MySql(pool) => {
                let mut query = sqlx::query_scalar::<_, i64>(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let count = query.fetch_one(&pool).await?;
                Ok(count)
            }
        }
    }

    pub async fn delete_query(&self) -> Result<u64, DatabaseError> {
        let mut sql = format!("DELETE FROM {}", self.table);
        let mut params = Vec::new();

        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            let clauses: Vec<String> = self
                .wheres
                .iter()
                .map(|w| {
                    params.push(w.value.clone());
                    format!("{} {} ?", w.column, w.operator)
                })
                .collect();
            sql.push_str(&clauses.join(" AND "));
        }

        let conn = self.connection()?;
        match conn {
            DatabaseConnection::Sqlite(pool) => {
                let mut query = sqlx::query(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let res = query.execute(&pool).await?;
                Ok(res.rows_affected())
            }
            DatabaseConnection::MySql(pool) => {
                let mut query = sqlx::query(&sql);
                for p in params {
                    query = match p {
                        QueryValue::Null => query.bind(None::<String>),
                        QueryValue::Int(i) => query.bind(i),
                        QueryValue::Float(f) => query.bind(f),
                        QueryValue::String(s) => query.bind(s),
                        QueryValue::Bool(b) => query.bind(b),
                    };
                }
                let res = query.execute(&pool).await?;
                Ok(res.rows_affected())
            }
        }
    }
}
