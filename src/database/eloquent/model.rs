use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Serialize;
use crate::database::connection::DatabaseConnection;
use crate::database::error::{DatabaseError, ModelNotFoundException};
use crate::database::database_manager::DB;
use crate::database::query::{QueryBuilder, QueryValue};
use crate::http::{Responsable, Response, Status};

#[async_trait]
pub trait Model: Serialize + DeserializeOwned + Send + Sync + Unpin + 'static {
    fn table() -> &'static str;

    fn primary_key() -> &'static str {
        "id"
    }

    fn connection() -> Option<&'static str> {
        None
    }

    fn query() -> QueryBuilder<Self>
    where
        Self: Sized,
    {
        QueryBuilder::new(Self::table(), Self::primary_key(), Self::connection().map(|s| s.to_string()))
    }

    async fn all() -> Result<Vec<Self>, DatabaseError>
    where
        Self: Sized,
    {
        Self::query().get().await
    }

    async fn find(id: impl Into<QueryValue> + Send) -> Result<Option<Self>, DatabaseError>
    where
        Self: Sized,
    {
        Self::query().where_eq(Self::primary_key(), id).first().await
    }

    async fn find_or_fail(id: impl Into<QueryValue> + Send) -> Result<Self, DatabaseError>
    where
        Self: Sized,
    {
        let id_val = id.into();
        Self::find(id_val)
            .await?
            .ok_or_else(|| ModelNotFoundException::new(Self::table()).into())
    }

    fn where_clause(
        column: impl Into<String>,
        operator: impl Into<String>,
        value: impl Into<QueryValue>,
    ) -> QueryBuilder<Self>
    where
        Self: Sized,
    {
        Self::query().where_clause(column, operator, value)
    }

    fn where_eq(
        column: impl Into<String>,
        value: impl Into<QueryValue>,
    ) -> QueryBuilder<Self>
    where
        Self: Sized,
    {
        Self::query().where_eq(column, value)
    }

    async fn count() -> Result<i64, DatabaseError>
    where
        Self: Sized,
    {
        Self::query().count().await
    }

    async fn create(model: Self) -> Result<Self, DatabaseError>
    where
        Self: Sized,
    {
        let conn = DB::connection(Self::connection())?;
        let table = Self::table();
        let value = serde_json::to_value(&model)?;
        let obj = value.as_object().ok_or_else(|| {
            DatabaseError::Configuration("Model serialization must result in a JSON object".into())
        })?;

        let mut columns = Vec::new();
        let mut placeholders = Vec::new();
        let mut values = Vec::new();

        for (k, v) in obj {
            // If primary key is null or missing, let DB auto-increment
            if k == Self::primary_key() && v.is_null() {
                continue;
            }
            columns.push(k.clone());
            placeholders.push("?");
            values.push(QueryValue::from_value(v));
        }

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table,
            columns.join(", "),
            placeholders.join(", ")
        );

        match conn {
            DatabaseConnection::Sqlite(pool) => {
                let mut q = sqlx::query(&sql);
                for v in values {
                    q = match v {
                        QueryValue::Null => q.bind(None::<String>),
                        QueryValue::Int(i) => q.bind(i),
                        QueryValue::Float(f) => q.bind(f),
                        QueryValue::String(s) => q.bind(s),
                        QueryValue::Bool(b) => q.bind(b),
                    };
                }
                let res = q.execute(&pool).await?;
                let last_id = res.last_insert_rowid();

                if let Some(created) = Self::find(last_id).await? {
                    return Ok(created);
                }
            }
            DatabaseConnection::MySql(pool) => {
                let mut q = sqlx::query(&sql);
                for v in values {
                    q = match v {
                        QueryValue::Null => q.bind(None::<String>),
                        QueryValue::Int(i) => q.bind(i),
                        QueryValue::Float(f) => q.bind(f),
                        QueryValue::String(s) => q.bind(s),
                        QueryValue::Bool(b) => q.bind(b),
                    };
                }
                let res = q.execute(&pool).await?;
                let last_id = res.last_insert_id();

                if let Some(created) = Self::find(last_id as i64).await? {
                    return Ok(created);
                }
            }
        }

        Ok(model)
    }

    async fn save(&self) -> Result<Self, DatabaseError>
    where
        Self: Sized + Clone,
    {
        let conn = DB::connection(Self::connection())?;
        let table = Self::table();
        let pk = Self::primary_key();
        let value = serde_json::to_value(self)?;
        let obj = value.as_object().ok_or_else(|| {
            DatabaseError::Configuration("Model serialization must result in a JSON object".into())
        })?;

        let pk_val = obj.get(pk);
        let has_pk = match pk_val {
            Some(serde_json::Value::Null) | None => false,
            _ => true,
        };

        if !has_pk {
            return Self::create(self.clone()).await;
        }

        let pk_query_val = QueryValue::from_value(pk_val.unwrap());

        let mut set_clauses = Vec::new();
        let mut values = Vec::new();

        for (k, v) in obj {
            if k == pk {
                continue;
            }
            set_clauses.push(format!("{} = ?", k));
            values.push(QueryValue::from_value(v));
        }

        let sql = format!(
            "UPDATE {} SET {} WHERE {} = ?",
            table,
            set_clauses.join(", "),
            pk
        );

        match conn {
            DatabaseConnection::Sqlite(pool) => {
                let mut q = sqlx::query(&sql);
                for v in values {
                    q = match v {
                        QueryValue::Null => q.bind(None::<String>),
                        QueryValue::Int(i) => q.bind(i),
                        QueryValue::Float(f) => q.bind(f),
                        QueryValue::String(s) => q.bind(s),
                        QueryValue::Bool(b) => q.bind(b),
                    };
                }
                q = match pk_query_val.clone() {
                    QueryValue::Null => q.bind(None::<String>),
                    QueryValue::Int(i) => q.bind(i),
                    QueryValue::Float(f) => q.bind(f),
                    QueryValue::String(s) => q.bind(s),
                    QueryValue::Bool(b) => q.bind(b),
                };
                q.execute(&pool).await?;
            }
            DatabaseConnection::MySql(pool) => {
                let mut q = sqlx::query(&sql);
                for v in values {
                    q = match v {
                        QueryValue::Null => q.bind(None::<String>),
                        QueryValue::Int(i) => q.bind(i),
                        QueryValue::Float(f) => q.bind(f),
                        QueryValue::String(s) => q.bind(s),
                        QueryValue::Bool(b) => q.bind(b),
                    };
                }
                q = match pk_query_val.clone() {
                    QueryValue::Null => q.bind(None::<String>),
                    QueryValue::Int(i) => q.bind(i),
                    QueryValue::Float(f) => q.bind(f),
                    QueryValue::String(s) => q.bind(s),
                    QueryValue::Bool(b) => q.bind(b),
                };
                q.execute(&pool).await?;
            }
        }

        Ok(self.clone())
    }

    async fn delete(&self) -> Result<bool, DatabaseError>
    where
        Self: Sized,
    {
        let pk = Self::primary_key();
        let value = serde_json::to_value(self)?;
        let obj = value.as_object().ok_or_else(|| {
            DatabaseError::Configuration("Model serialization must result in a JSON object".into())
        })?;

        let pk_val = match obj.get(pk) {
            Some(v) if !v.is_null() => QueryValue::from_value(v),
            _ => return Ok(false),
        };

        let affected = Self::query().where_eq(pk, pk_val).delete_query().await?;
        Ok(affected > 0)
    }
}

// Implement Responsable for Model structs
impl<M: Model> Responsable for M {
    fn to_response(self) -> Response {
        match serde_json::to_string(&self) {
            Ok(json) => Response::ok().json(json),
            Err(e) => Response::new(Status::InternalServerError).json(format!(r#"{{"error":"{}"}}"#, e)),
        }
    }
}

// Implement Responsable for Result<M, DatabaseError> and Result<Option<M>, DatabaseError>
impl<M: Model> Responsable for Result<Option<M>, DatabaseError> {
    fn to_response(self) -> Response {
        match self {
            Ok(Some(model)) => model.to_response(),
            Ok(None) => Response::not_found().json(r#"{"message":"Not Found"}"#),
            Err(DatabaseError::ModelNotFound(e)) => {
                Response::not_found().json(format!(r#"{{"error":"{}"}}"#, e))
            }
            Err(e) => Response::new(Status::InternalServerError).json(format!(r#"{{"error":"{}"}}"#, e)),
        }
    }
}

impl<M: Model> Responsable for Result<M, DatabaseError> {
    fn to_response(self) -> Response {
        match self {
            Ok(model) => model.to_response(),
            Err(DatabaseError::ModelNotFound(e)) => {
                Response::not_found().json(format!(r#"{{"error":"{}"}}"#, e))
            }
            Err(e) => Response::new(Status::InternalServerError).json(format!(r#"{{"error":"{}"}}"#, e)),
        }
    }
}

impl<M: Model> Responsable for Result<Vec<M>, DatabaseError> {
    fn to_response(self) -> Response {
        match self {
            Ok(models) => match serde_json::to_string(&models) {
                Ok(json) => Response::ok().json(json),
                Err(e) => Response::new(Status::InternalServerError).json(format!(r#"{{"error":"{}"}}"#, e)),
            },
            Err(e) => Response::new(Status::InternalServerError).json(format!(r#"{{"error":"{}"}}"#, e)),
        }
    }
}
