use ruvel::database::eloquent::Model;
use ruvel::database::{DatabaseConnection, DatabaseError};
use ruvel::foundation::Application;
use ruvel::http::{Responsable, Status};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct User {
    id: Option<i64>,
    name: String,
    email: String,
}

impl Model for User {
    fn table() -> &'static str {
        "users"
    }
}

#[tokio::test]
async fn test_database_manager_and_eloquent_crud() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE users (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            email TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = Application::configure("").create();
    let db = app.make::<ruvel::database::DatabaseManager>().expect("DatabaseManager registered by DatabaseServiceProvider");
    db.add_connection("sqlite", DatabaseConnection::Sqlite(pool));
    db.set_default_connection("sqlite");

    // 1. Create records
    let user1 = User::create(User {
        id: None,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    })
    .await
    .expect("Failed to create user1");

    assert_eq!(user1.id, Some(1));
    assert_eq!(user1.name, "Alice");

    let user2 = User::create(User {
        id: None,
        name: "Bob".to_string(),
        email: "bob@example.com".to_string(),
    })
    .await
    .expect("Failed to create user2");

    assert_eq!(user2.id, Some(2));
    assert_eq!(user2.name, "Bob");

    // 2. Count
    let count = User::count().await.expect("Failed to count users");
    assert_eq!(count, 2);

    // 3. All
    let all = User::all().await.expect("Failed to fetch all users");
    assert_eq!(all.len(), 2);

    // 4. Find
    let found = User::find(1).await.expect("Failed to find user");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Alice");

    let not_found = User::find(999).await.expect("Failed to query user 999");
    assert!(not_found.is_none());

    // 5. Find or fail
    let found_fail = User::find_or_fail(1).await.expect("Should find user 1");
    assert_eq!(found_fail.name, "Alice");

    let err_fail = User::find_or_fail(999).await;
    match err_fail {
        Err(DatabaseError::ModelNotFound(e)) => {
            assert_eq!(e.model, "users");
        }
        _ => panic!("Expected ModelNotFound error"),
    }

    // 6. Query builder with where clause
    let alice = User::where_eq("name", "Alice")
        .first()
        .await
        .expect("Query failed");
    assert!(alice.is_some());
    assert_eq!(alice.unwrap().email, "alice@example.com");

    // 7. Update via save()
    let mut user_to_update = user1.clone();
    user_to_update.name = "Alice Updated".to_string();
    let updated = user_to_update.save().await.expect("Failed to update user");
    assert_eq!(updated.name, "Alice Updated");

    let re_fetched = User::find(1).await.unwrap().unwrap();
    assert_eq!(re_fetched.name, "Alice Updated");

    // 8. Delete
    let deleted = user2.delete().await.expect("Failed to delete user");
    assert!(deleted);
    let count_after_delete = User::count().await.unwrap();
    assert_eq!(count_after_delete, 1);

    // 9. Responsable serialization
    let res = user_to_update.to_response();
    assert_eq!(res.status, Status::Ok);
    let body_str = String::from_utf8(res.body).unwrap();
    assert!(body_str.contains("Alice Updated"));

    let not_found_res: Result<Option<User>, DatabaseError> = Ok(None);
    let not_found_resp = not_found_res.to_response();
    assert_eq!(not_found_resp.status, Status::NotFound);
}
