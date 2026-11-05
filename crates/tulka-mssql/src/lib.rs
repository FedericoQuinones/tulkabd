//! SQL Server driver for tulkabd, built on the vendored Tiberius fork.

mod batches;
mod convert;
pub mod guard;
mod lexer;
mod meta;
pub mod readonly;
mod session;

// Lets the shared test support code name this crate the way integration tests do.
#[cfg(test)]
extern crate self as tulka_mssql;

use async_trait::async_trait;
use tulka_core::{ConnectParams, Driver, DriverError, GuardFinding, ReadOnlyViolation, Session};

pub use batches::{split_batches, Batch};

/// The SQL Server driver.
pub struct MssqlDriver;

#[async_trait]
impl Driver for MssqlDriver {
    fn id(&self) -> &'static str {
        "mssql"
    }

    async fn connect(&self, p: &ConnectParams) -> Result<Box<dyn Session>, DriverError> {
        Ok(Box::new(session::connect(p).await?))
    }

    fn guard_findings(&self, sql: &str) -> Vec<GuardFinding> {
        guard::findings(sql)
    }

    fn read_only_violation(&self, sql: &str) -> Option<ReadOnlyViolation> {
        readonly::violation(sql)
    }
}
