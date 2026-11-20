use codex_core::McpRefreshReason;
use codex_core::ThreadManager;
use codex_core_plugins::EffectivePluginsChange;
use std::sync::Arc;

pub(crate) fn effective_plugins_changed_callback(
    thread_manager: Arc<ThreadManager>,
) -> Arc<dyn Fn(EffectivePluginsChange) + Send + Sync> {
    Arc::new(move |_| {
        thread_manager.plugins_manager().clear_cache();
        thread_manager.skills_service().clear_cache();
        let thread_manager = Arc::clone(&thread_manager);
        tokio::spawn(async move {
            thread_manager
                .invalidate_mcp_runtimes_for(McpRefreshReason::EffectivePluginsChanged)
                .await;
            thread_manager.refresh_hook_runtimes().await;
        });
    })
}
