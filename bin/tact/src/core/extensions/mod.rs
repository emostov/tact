//! Agent extensions exposed to the core runtime.

mod claude_bash;
mod current_session;
mod mcp;
pub(crate) mod sessions;
mod skills;

pub(super) use claude_bash::WorkspaceBash;
pub(super) use current_session::CurrentSessionTool;
pub(super) use mcp::provider as mcp_provider;
pub(crate) use skills::Skill;
pub(super) use skills::SkillCatalog;
