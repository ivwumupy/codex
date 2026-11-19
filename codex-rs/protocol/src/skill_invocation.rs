use crate::protocol::SkillScope;
use codex_utils_path_uri::PathUri;
use serde::Serialize;
#[derive(Clone, Debug)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub location: SkillInvocationLocation,
    pub plugin_id: Option<String>,
    pub remote_plugin_id: Option<String>,
    pub invocation_type: InvocationType,
}

#[derive(Clone, Debug)]
pub enum SkillInvocationLocation {
    Host {
        path: PathUri,
        scope: SkillScope,
    },
    Resource {
        id: String,
        skill_id: Option<String>,
        scope: Option<SkillScope>,
    },
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InvocationType {
    Explicit,
    Implicit,
}
