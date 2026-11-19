use super::ExecPolicyManager;
use codex_execpolicy::Policy;
use codex_execpolicy::RequirementsExecPolicy;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllowPrefixRules {
    Honor,
    IgnoreForCyberModel,
}

impl ExecPolicyManager {
    pub(crate) fn current_for_environment(
        &self,
        _environment_policy: Option<&RequirementsExecPolicy>,
        _allow_prefix_rules: AllowPrefixRules,
    ) -> Arc<Policy> {
        self.current()
    }

    pub(crate) fn current_for_prefix_rules(&self, _allow: AllowPrefixRules) -> Arc<Policy> {
        self.current()
    }
}
