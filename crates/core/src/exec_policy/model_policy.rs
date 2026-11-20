use super::ExecPolicyManager;
use codex_execpolicy::Policy;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AllowPrefixRules {
    Honor,
    IgnoreForCyberModel,
}

impl ExecPolicyManager {
    pub(crate) fn current_for_prefix_rules(&self, _allow: AllowPrefixRules) -> Arc<Policy> {
        self.current()
    }
}
