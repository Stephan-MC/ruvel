use ruvel::container::Container;
use ruvel::database::DatabaseManager;
use ruvel::foundation::Application;

#[test]
fn test_application_base_bindings_and_aliases() {
    let app = Application::new("");

    // Test base bindings: container registered
    let container_res = app.container.make::<Container>();
    assert!(container_res.is_ok());

    // Test core aliases
    let app_by_alias = app.container.make_alias::<Container>("app");
    assert!(app_by_alias.is_ok());

    let db_by_alias = app.container.make_alias::<DatabaseManager>("db");
    assert!(db_by_alias.is_ok());

    // Test that base providers are registered
    assert_eq!(app.providers.len(), 4);
}
