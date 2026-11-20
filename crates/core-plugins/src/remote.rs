pub const REMOTE_GLOBAL_MARKETPLACE_NAME: &str = "openai-curated-remote";
pub const REMOTE_CREATED_BY_ME_MARKETPLACE_NAME: &str = "created-by-me-remote";
pub const REMOTE_WORKSPACE_MARKETPLACE_NAME: &str = "workspace-directory";
pub const REMOTE_WORKSPACE_SHARED_WITH_ME_MARKETPLACE_NAME: &str = "workspace-shared-with-me";
pub const REMOTE_WORKSPACE_SHARED_WITH_ME_PRIVATE_MARKETPLACE_NAME: &str =
    "workspace-shared-with-me-private";
pub const REMOTE_WORKSPACE_SHARED_WITH_ME_UNLISTED_MARKETPLACE_NAME: &str =
    "workspace-shared-with-me-unlisted";

pub fn is_valid_remote_plugin_id(plugin_id: &str) -> bool {
    !plugin_id.is_empty()
        && plugin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '~')
}
