use crate::container::Container;
use crate::database::DatabaseManager;
use crate::routing::Router;
use crate::support::ServiceProvider;
use std::sync::Arc;

pub struct DatabaseServiceProvider;

impl ServiceProvider for DatabaseServiceProvider {
    fn register(&self, container: &mut Container) {
        let db_manager = Arc::new(DatabaseManager::new());
        DatabaseManager::register_global(Arc::clone(&db_manager));
        container.instance((*db_manager).clone());
    }

    fn boot(&self, container: &Container, _router: &mut Router) {
        if let Ok(manager) = container.make::<DatabaseManager>() {
            DatabaseManager::register_global(manager);
        }
    }
}
