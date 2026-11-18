//! Validate a trusted host's policy snapshot against the destination's managed requirements.

use super::ExecPolicyManager;
use codex_config::ConfigLayerStack;
use codex_execpolicy::Policy;
use codex_execpolicy::RequirementsExecPolicy;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CodexResult;
use std::sync::Arc;

/// Point-in-time session policy and the managed requirements it was loaded with.
/// A trusted host transports these together and reapplies them on cold resume.
pub(crate) struct InheritedExecPolicy {
    pub(crate) policy: Arc<Policy>,
    pub(crate) source_requirements: Option<Arc<RequirementsExecPolicy>>,
}

impl ExecPolicyManager {
    pub(crate) async fn from_inherited(
        snapshot: &InheritedExecPolicy,
        stack: &ConfigLayerStack,
    ) -> CodexResult<Self> {
        if stack.requirements().exec_policy.is_some()
            || snapshot.source_requirements.is_some()
            || !snapshot.policy.rules().is_empty()
            || !snapshot.policy.network_rules().is_empty()
            || !snapshot.policy.host_executables().is_empty()
        {
            return Err(CodexErr::InvalidRequest(
                "execution policies are unsupported in this YOLO-only build".to_string(),
            ));
        }
        Ok(Self)
    }
}
