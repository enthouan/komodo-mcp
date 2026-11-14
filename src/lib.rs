//! Read-only Core inventory, bounded fleet observations, and safe reporting.

pub mod build_configuration;
pub mod builders;
pub mod capabilities;
pub mod client;
pub mod config;
pub mod configuration;
pub mod deletion;
pub mod deployment_configuration;
pub mod diagnostics;
pub mod error;
pub mod health;
pub mod history;
pub mod inventory;
pub mod logs;
pub mod operations;
pub mod recovery;
pub mod repositories;
pub mod server;
pub mod server_configuration;
pub mod stack_configuration;
pub mod tags;
pub mod triage;
pub mod wait;
