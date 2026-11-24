#![cfg(test)]

use chrono::{DateTime, Utc};
pub use lucidos_engine::core::changes_projection::*;
use sqlx::PgPool;
use uuid::Uuid;

#[path = "../../../lucidos-core/src/core/changes_projection_tests/shared_fixtures.rs"]
mod cp_helpers;
#[path = "changes_projection_tests/queries.rs"]
mod queries_tests;
