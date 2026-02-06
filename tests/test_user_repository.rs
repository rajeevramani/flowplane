// NOTE: This file requires PostgreSQL - disabled until Phase 4 of PostgreSQL migration
// To run these tests: cargo test --features postgres_tests
#![cfg(feature = "postgres_tests")]

//! Integration tests for user and team membership repositories
//!
//! Tests UserRepository and TeamMembershipRepository implementations
//! with SQLite database to ensure all CRUD operations work correctly.

mod common;

use common::test_db::TestDatabase;
use flowplane::auth::team::CreateTeamRequest;
use flowplane::auth::user::{NewUser, NewUserTeamMembership, UpdateUser, UserStatus};
use flowplane::domain::UserId;
use flowplane::storage::repositories::team::{SqlxTeamRepository, TeamRepository};
use flowplane::storage::repositories::{
    SqlxTeamMembershipRepository, SqlxUserRepository, TeamMembershipRepository, UserRepository,
};
use flowplane::storage::DbPool;

async fn create_test_pool() -> (TestDatabase, DbPool) {
    let test_db = TestDatabase::new("user_repository").await;
    let pool = test_db.pool.clone();

    // Create test teams to satisfy FK constraints
    let team_repo = SqlxTeamRepository::new(pool.clone());
    for team_name in &[
        "team-a",
        "team-alpha",
        "team-beta",
        "team-gamma",
        "team-delta",
        "team-epsilon",
        "team-zeta",
        "team-1",
        "team-2",
        "team-3",
        "cascade-team-1",
        "cascade-team-2",
        "cascade-team-3",
    ] {
        let _ = team_repo
            .create_team(CreateTeamRequest {
                name: team_name.to_string(),
                display_name: format!("Test Team {}", team_name),
                description: Some("Team for user repository tests".to_string()),
                owner_user_id: None,
                settings: None,
            })
            .await;
    }

    (test_db, pool)
}

// UserRepository tests

#[tokio::test]
async fn test_create_and_get_user() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "test@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Test User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    // Create user
    let created = repo.create_user(new_user).await.unwrap();

    assert_eq!(created.id, user_id);
    assert_eq!(created.email, "test@example.com");
    assert_eq!(created.name, "Test User");
    assert_eq!(created.status, UserStatus::Active);
    assert!(!created.is_admin);

    // Get user by ID
    let fetched = repo.get_user(&user_id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.email, created.email);
}

#[tokio::test]
async fn test_get_user_by_email() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "findme@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Find Me".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Find by email
    let found = repo.get_user_by_email("findme@example.com").await.unwrap().unwrap();
    assert_eq!(found.id, user_id);
    assert_eq!(found.email, "findme@example.com");

    // Not found
    let not_found = repo.get_user_by_email("notfound@example.com").await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_get_user_with_password() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let password_hash = "secure_hash_123".to_string();

    let new_user = NewUser {
        id: user_id.clone(),
        email: "auth@example.com".to_string(),
        password_hash: password_hash.clone(),
        name: "Auth User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Get with password
    let (user, hash) = repo.get_user_with_password("auth@example.com").await.unwrap().unwrap();

    assert_eq!(user.id, user_id);
    assert_eq!(hash, password_hash);
}

#[tokio::test]
async fn test_update_user() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "update@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Original Name".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Update user
    let update = UpdateUser {
        email: Some("newemail@example.com".to_string()),
        name: Some("Updated Name".to_string()),
        status: Some(UserStatus::Inactive),
        is_admin: Some(true),
    };

    let updated = repo.update_user(&user_id, update).await.unwrap();

    assert_eq!(updated.email, "newemail@example.com");
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(updated.status, UserStatus::Inactive);
    assert!(updated.is_admin);
}

#[tokio::test]
async fn test_partial_update_user() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "partial@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Original Name".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Only update name
    let update = UpdateUser {
        email: None,
        name: Some("Just Name Changed".to_string()),
        status: None,
        is_admin: None,
    };

    let updated = repo.update_user(&user_id, update).await.unwrap();

    assert_eq!(updated.email, "partial@example.com"); // Unchanged
    assert_eq!(updated.name, "Just Name Changed"); // Changed
    assert_eq!(updated.status, UserStatus::Active); // Unchanged
    assert!(!updated.is_admin); // Unchanged
}

#[tokio::test]
async fn test_update_password() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "changepw@example.com".to_string(),
        password_hash: "old_hash".to_string(),
        name: "Password Changer".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Update password
    repo.update_password(&user_id, "new_hash".to_string()).await.unwrap();

    // Verify new password
    let (_user, hash) = repo.get_user_with_password("changepw@example.com").await.unwrap().unwrap();
    assert_eq!(hash, "new_hash");
}

#[tokio::test]
async fn test_list_users() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    // Create multiple users
    for i in 1..=5 {
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id,
            email: format!("user{}@example.com", i),
            password_hash: "hash123".to_string(),
            name: format!("User {}", i),
            status: UserStatus::Active,
            is_admin: false,
        };
        repo.create_user(new_user).await.unwrap();
    }

    // List all users
    let users = repo.list_users(10, 0).await.unwrap();
    assert_eq!(users.len(), 5);

    // Test pagination
    let page1 = repo.list_users(2, 0).await.unwrap();
    assert_eq!(page1.len(), 2);

    let page2 = repo.list_users(2, 2).await.unwrap();
    assert_eq!(page2.len(), 2);
}

#[tokio::test]
async fn test_count_users() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let count_before = repo.count_users().await.unwrap();
    assert_eq!(count_before, 0);

    // Create users with different statuses
    for i in 1..=3 {
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id,
            email: format!("active{}@example.com", i),
            password_hash: "hash123".to_string(),
            name: format!("Active User {}", i),
            status: UserStatus::Active,
            is_admin: false,
        };
        repo.create_user(new_user).await.unwrap();
    }

    for i in 1..=2 {
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id,
            email: format!("inactive{}@example.com", i),
            password_hash: "hash123".to_string(),
            name: format!("Inactive User {}", i),
            status: UserStatus::Inactive,
            is_admin: false,
        };
        repo.create_user(new_user).await.unwrap();
    }

    // Total count
    let total = repo.count_users().await.unwrap();
    assert_eq!(total, 5);

    // Count by status
    let active_count = repo.count_users_by_status(UserStatus::Active).await.unwrap();
    assert_eq!(active_count, 3);

    let inactive_count = repo.count_users_by_status(UserStatus::Inactive).await.unwrap();
    assert_eq!(inactive_count, 2);
}

#[tokio::test]
async fn test_delete_user() {
    let (_db, pool) = create_test_pool().await;
    let repo = SqlxUserRepository::new(pool);

    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "delete@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "To Delete".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };

    repo.create_user(new_user).await.unwrap();

    // Verify user exists
    let exists = repo.get_user(&user_id).await.unwrap();
    assert!(exists.is_some());

    // Delete user
    repo.delete_user(&user_id).await.unwrap();

    // Verify user is deleted
    let deleted = repo.get_user(&user_id).await.unwrap();
    assert!(deleted.is_none());
}

// TeamMembershipRepository tests

#[tokio::test]
async fn test_create_and_get_membership() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user first
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "member@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Team Member".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    // Create membership
    let membership_id = "membership-1".to_string();
    let new_membership = NewUserTeamMembership {
        id: membership_id.clone(),
        user_id: user_id.clone(),
        team: "team-a".to_string(),
        scopes: vec!["clusters:read".to_string(), "routes:write".to_string()],
    };

    let created = membership_repo.create_membership(new_membership).await.unwrap();

    assert_eq!(created.id, membership_id);
    assert_eq!(created.user_id, user_id);
    assert_eq!(created.team, "team-a");
    assert_eq!(created.scopes.len(), 2);

    // Get membership
    let fetched = membership_repo.get_membership(&membership_id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
}

#[tokio::test]
async fn test_list_user_memberships() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "multi-team@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Multi Team User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    // Create memberships for different teams
    for i in 1..=3 {
        let membership = NewUserTeamMembership {
            id: format!("membership-{}", i),
            user_id: user_id.clone(),
            team: format!("team-{}", i),
            scopes: vec!["clusters:read".to_string()],
        };
        membership_repo.create_membership(membership).await.unwrap();
    }

    // List all memberships for user
    let memberships = membership_repo.list_user_memberships(&user_id).await.unwrap();
    assert_eq!(memberships.len(), 3);
}

#[tokio::test]
async fn test_list_team_members() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create multiple users
    for i in 1..=3 {
        let user_id = UserId::new();
        let new_user = NewUser {
            id: user_id.clone(),
            email: format!("teamuser{}@example.com", i),
            password_hash: "hash123".to_string(),
            name: format!("Team User {}", i),
            status: UserStatus::Active,
            is_admin: false,
        };
        user_repo.create_user(new_user).await.unwrap();

        // Add to team-alpha
        let membership = NewUserTeamMembership {
            id: format!("membership-alpha-{}", i),
            user_id: user_id.clone(),
            team: "team-alpha".to_string(),
            scopes: vec!["clusters:read".to_string()],
        };
        membership_repo.create_membership(membership).await.unwrap();
    }

    // List all members of team-alpha
    let members = membership_repo.list_team_members("team-alpha").await.unwrap();
    assert_eq!(members.len(), 3);
}

#[tokio::test]
async fn test_get_user_team_membership() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "specific@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Specific User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    // Create membership for team-beta
    let membership = NewUserTeamMembership {
        id: "membership-beta".to_string(),
        user_id: user_id.clone(),
        team: "team-beta".to_string(),
        scopes: vec!["admin:all".to_string()],
    };
    membership_repo.create_membership(membership).await.unwrap();

    // Get specific membership
    let found =
        membership_repo.get_user_team_membership(&user_id, "team-beta").await.unwrap().unwrap();

    assert_eq!(found.user_id, user_id);
    assert_eq!(found.team, "team-beta");

    // Not found for different team
    let not_found = membership_repo.get_user_team_membership(&user_id, "team-gamma").await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_update_membership_scopes() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user and membership
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "updatescopes@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Update Scopes User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    let membership_id = "membership-update".to_string();
    let membership = NewUserTeamMembership {
        id: membership_id.clone(),
        user_id: user_id.clone(),
        team: "team-delta".to_string(),
        scopes: vec!["clusters:read".to_string()],
    };
    membership_repo.create_membership(membership).await.unwrap();

    // Update scopes
    let new_scopes =
        vec!["clusters:read".to_string(), "clusters:write".to_string(), "admin:all".to_string()];

    let updated =
        membership_repo.update_membership_scopes(&membership_id, new_scopes.clone()).await.unwrap();

    assert_eq!(updated.scopes, new_scopes);
}

#[tokio::test]
async fn test_delete_membership() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user and membership
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "deletemembership@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Delete Membership User".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    let membership_id = "membership-delete".to_string();
    let membership = NewUserTeamMembership {
        id: membership_id.clone(),
        user_id: user_id.clone(),
        team: "team-epsilon".to_string(),
        scopes: vec!["clusters:read".to_string()],
    };
    membership_repo.create_membership(membership).await.unwrap();

    // Verify exists
    let exists = membership_repo.get_membership(&membership_id).await.unwrap();
    assert!(exists.is_some());

    // Delete membership
    membership_repo.delete_membership(&membership_id).await.unwrap();

    // Verify deleted
    let deleted = membership_repo.get_membership(&membership_id).await.unwrap();
    assert!(deleted.is_none());
}

#[tokio::test]
async fn test_delete_user_team_membership() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "deleteuserteam@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Delete User Team".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    // Create membership
    let membership = NewUserTeamMembership {
        id: "membership-zeta".to_string(),
        user_id: user_id.clone(),
        team: "team-zeta".to_string(),
        scopes: vec!["clusters:read".to_string()],
    };
    membership_repo.create_membership(membership).await.unwrap();

    // Delete by user_id and team
    membership_repo.delete_user_team_membership(&user_id, "team-zeta").await.unwrap();

    // Verify deleted
    let deleted = membership_repo.get_user_team_membership(&user_id, "team-zeta").await.unwrap();
    assert!(deleted.is_none());
}

#[tokio::test]
async fn test_delete_user_cascades_to_memberships() {
    let (_db, pool) = create_test_pool().await;
    let user_repo = SqlxUserRepository::new(pool.clone());
    let membership_repo = SqlxTeamMembershipRepository::new(pool);

    // Create user
    let user_id = UserId::new();
    let new_user = NewUser {
        id: user_id.clone(),
        email: "cascade@example.com".to_string(),
        password_hash: "hash123".to_string(),
        name: "Cascade Test".to_string(),
        status: UserStatus::Active,
        is_admin: false,
    };
    user_repo.create_user(new_user).await.unwrap();

    // Create multiple memberships
    for i in 1..=3 {
        let membership = NewUserTeamMembership {
            id: format!("cascade-membership-{}", i),
            user_id: user_id.clone(),
            team: format!("cascade-team-{}", i),
            scopes: vec!["clusters:read".to_string()],
        };
        membership_repo.create_membership(membership).await.unwrap();
    }

    // Verify memberships exist
    let memberships_before = membership_repo.list_user_memberships(&user_id).await.unwrap();
    assert_eq!(memberships_before.len(), 3);

    // Delete user
    user_repo.delete_user(&user_id).await.unwrap();

    // Verify memberships are cascade deleted
    let memberships_after = membership_repo.list_user_memberships(&user_id).await.unwrap();
    assert_eq!(memberships_after.len(), 0);
}
