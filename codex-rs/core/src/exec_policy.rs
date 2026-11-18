use crate::config::Config;
use codex_execpolicy::Policy;
use codex_execpolicy::RequirementsExecPolicy;
use codex_protocol::protocol::AskForApproval;
use std::sync::Arc;

mod inheritance;
mod model_policy;

pub(crate) use inheritance::InheritedExecPolicy;
pub(crate) use model_policy::AllowPrefixRules;

#[derive(Default)]
pub(crate) struct ExecPolicyManager;

impl ExecPolicyManager {
    pub(crate) fn current(&self) -> Arc<Policy> {
        Arc::new(Policy::empty())
    }

    pub(crate) fn snapshot(&self) -> (Arc<Policy>, Option<Arc<RequirementsExecPolicy>>) {
        (self.current(), None)
    }
}

pub(crate) fn child_uses_parent_exec_policy(_parent: &Config, _child: &Config) -> bool {
    true
}

/// Returns a rejection reason when `approval_policy` disallows surfacing the
/// current prompt to the user.
///
/// `prompt_is_rule` distinguishes policy-rule prompts from sandbox/escalation
/// prompts so granular `rules` and `sandbox_approval` settings are honored
/// independently. When both are present, policy-rule prompts take precedence.
pub(crate) fn prompt_is_rejected_by_policy(
    approval_policy: AskForApproval,
    prompt_is_rule: bool,
) -> Option<&'static str> {
    match approval_policy {
        AskForApproval::Never => Some("approval prompts are disabled"),
        AskForApproval::OnRequest => None,
        AskForApproval::UnlessTrusted => None,
        AskForApproval::Granular(granular_config) => {
            if prompt_is_rule {
                if !granular_config.allows_rules_approval() {
                    Some("rule approval prompts are disabled")
                } else {
                    None
                }
            } else if !granular_config.allows_sandbox_approval() {
                Some("sandbox approval prompts are disabled")
            } else {
                None
            }
        }
    }
}
