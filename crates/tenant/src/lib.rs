//! Tenant management: per-tenant registry, quota enforcement, and resource metering.

pub mod error;
pub mod manager;
pub mod meter;
pub mod registry;
pub mod supervisor;
pub mod tenant;

pub(crate) mod events;
pub(crate) mod session_entry;

pub use agent_core::AgentEvent;
pub use error::TenantError;
pub use manager::{CreateSessionParams, SessionInfo, SessionUpdates, TenantManager, WebhookConfig, WaitResult};
pub use meter::CostTracker;
pub use registry::TenantRegistry;
pub use supervisor::{QuotaStatus, SessionGuard, TenantSupervisor};
pub use tenant::{QuotaCheck, Tenant, TenantQuota};
