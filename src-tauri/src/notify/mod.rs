//! Notification subsystem — SQLite-backed team event store.
//! Webhook dispatch has been removed per the plan.
pub mod store;

pub use store::{Store, TeamEventRow};
