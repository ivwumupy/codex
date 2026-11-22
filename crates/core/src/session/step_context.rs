//! Request-scoped settings and capabilities, with access to current session grants.

use std::sync::Arc;

use crate::agents_md::LoadedAgentsMd;
use crate::config::TokenBudgetConfig;
use crate::environment_selection::TurnEnvironmentSnapshot;
use crate::session::step_settings::ResolvedStepSettings;
use crate::session::turn_context::TurnContext;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::router::ToolRouter;
use codex_extension_api::ExtensionData;
use codex_features::Feature;
use codex_file_system::EnvironmentAccess;
use codex_mcp::McpBinding;
use codex_otel::SessionTelemetry;
use codex_plugin::ExecutorPluginHookSource;
use codex_protocol::items::ModelInvocationContext;
use codex_protocol::protocol::TurnContextItem;
use tokio_util::sync::CancellationToken;

/// Request-scoped state that may change between model sampling requests.
#[derive(Clone)]
pub(crate) struct StepContext {
    /// Input prefix captured for this normal inference attempt, acknowledged with its output.
    pub(crate) sampled_through_sequence: Option<u64>,
    /// Final serving value for this invocation; retained by its code cell across waits.
    pub(crate) serving_assessment: Option<Arc<crate::guardian::ServingAssessment>>,
    pub(crate) turn: Arc<TurnContext>,
    /// Preempts this request and yields its code-mode observations when user input arrives.
    pub(crate) preempt: Option<CancellationToken>,
    /// One immutable settings version captured before request preparation.
    pub(crate) settings: Arc<ResolvedStepSettings>,
    /// Frozen turn preferences resolved against this step's captured model.
    pub(crate) token_budget: Option<TokenBudgetConfig>,
    /// Telemetry context tagged with this sampling request's model.
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) environments: TurnEnvironmentSnapshot,
    /// Child bindings captured with this step; configured-only when opted in.
    pub(crate) inherited_environments: TurnEnvironmentSnapshot,
    /// Admitted executor hooks bound to this step's discovery, plugin policy, and MCP routing.
    pub(crate) executor_hook_sources: Vec<ExecutorPluginHookSource>,
    /// Keeps the extension inputs used to build this step's tools for its World State as well.
    pub(crate) extension_data: Arc<ExtensionData>,
    /// The exact MCP connections, configuration, and catalog captured for this step.
    pub(crate) mcp: Arc<McpBinding>,
    /// The finalized tool plan advertised and executed for this exact sampling request.
    pub(crate) tool_router: Arc<ToolRouter>,
    /// The canonical AGENTS.md value observed with this environment snapshot.
    pub(crate) loaded_agents_md: Option<Arc<LoadedAgentsMd>>,
}

impl StepContext {
    /// Feature that selects the tool layout of new windows on this route.
    pub(crate) fn incremental_tools_feature(&self) -> Option<Feature> {
        self.settings
            .model_info
            .use_responses_lite
            .then_some(Feature::IncrementalTools)
    }

    /// Whether new context windows should record tool declarations in history.
    pub(crate) fn incremental_tools_enabled(&self) -> bool {
        self.incremental_tools_feature()
            .is_some_and(|feature| self.turn.config.features.enabled(feature))
    }

    /// Pairs the step's environments with access using current session grants.
    pub(crate) fn environments(&self) -> Vec<(&TurnEnvironment, impl EnvironmentAccess + '_)> {
        self.environments
            .turn_environments()
            .map(|environment| (environment, self.fs_accessor(environment)))
            .collect()
    }

    pub(crate) fn fs_accessor<'a>(
        &self,
        environment: &'a TurnEnvironment,
    ) -> impl EnvironmentAccess + 'a + use<'a> {
        let grants = self
            .turn
            .granted_permissions(&environment.selection.environment_id);
        environment.fs_accessor(grants)
    }

    /// Persist the context captured for this request, even after a live update.
    pub(crate) fn to_turn_context_item(&self) -> TurnContextItem {
        self.turn.to_turn_context_item()
    }

    pub(crate) fn model_context(&self) -> ModelInvocationContext {
        ModelInvocationContext {
            model_slug: self.settings.model_info.slug.clone(),
            reasoning_effort: self
                .settings
                .effective_reasoning_effort()
                .map(|effort| effort.to_string()),
        }
    }
}
