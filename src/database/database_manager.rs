use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use crate::database::connection::DatabaseConnection;
use crate::database::error::DatabaseError;
use std::sync::OnceLock;

static GLOBAL_DB_MANAGER: OnceLock<RwLock<Option<Arc<DatabaseManager>>>> = OnceLock::new();

fn get_global_slot() -> &'static RwLock<Option<Arc<DatabaseManager>>> {
    GLOBAL_DB_MANAGER.get_or_init(|| RwLock::new(None))
}

#[derive(Default, Clone)]
pub struct DatabaseManager {
    connections: Arc<RwLock<HashMap<String, DatabaseConnection>>>,
    default_connection: Arc<RwLock<Option<String>>>,
}

impl DatabaseManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            default_connection: Arc::new(RwLock::new(None)),
        }
    }

    pub fn add_connection(&self, name: impl Into<String>, connection: DatabaseConnection) {
        let name_str = name.into();
        let mut conns = self.connections.write().unwrap();
        let mut def = self.default_connection.write().unwrap();
        if def.is_none() {
            *def = Some(name_str.clone());
        }
        conns.insert(name_str, connection);
    }

    pub fn set_default_connection(&self, name: impl Into<String>) {
        let mut def = self.default_connection.write().unwrap();
        *def = Some(name.into());
    }

    pub fn default_connection_name(&self) -> Option<String> {
        self.default_connection.read().unwrap().clone()
    }

    pub fn connection(&self, name: Option<&str>) -> Result<DatabaseConnection, DatabaseError> {
        let conns = self.connections.read().unwrap();
        let conn_name = match name {
            Some(n) => n.to_string(),
            None => {
                let def = self.default_connection.read().unwrap();
                def.clone().ok_or(DatabaseError::NoDefaultConnection)?
            }
        };

        conns.get(&conn_name)
            .cloned()
            .ok_or_else(|| DatabaseError::ConnectionNotFound(conn_name))
    }

    pub fn register_global(manager: Arc<DatabaseManager>) {
        let slot = get_global_slot();
        let mut lock = slot.write().unwrap();
        *lock = Some(manager);
    }

    pub fn global() -> Result<Arc<DatabaseManager>, DatabaseError> {
        let slot = get_global_slot();
        let lock = slot.read().unwrap();
        lock.clone().ok_or(DatabaseError::NoDefaultConnection)
    }
}

pub struct DB;

impl DB {
    pub fn connection(name: Option<&str>) -> Result<DatabaseConnection, DatabaseError> {
        DatabaseManager::global()?.connection(name)
    }

    pub fn default_connection() -> Result<DatabaseConnection, DatabaseError> {
        Self::connection(None)
    }
}
