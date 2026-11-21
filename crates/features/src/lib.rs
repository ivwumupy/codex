//! Centralized feature flags and metadata.
//!
//! This crate defines the feature registry plus the logic used to resolve an
//! effective feature set from config-like inputs.

use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WarningEvent;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use toml::Table;

mod feature_configs;
mod runtime_auth;
pub use feature_configs::AsyncCompactionConfigToml;
pub use feature_configs::CodeModeConfigToml;
pub use feature_configs::CodeModeHostConfigToml;
pub use feature_configs::ContextManagementConfigToml;
pub use feature_configs::CurrentTimeReminderConfigToml;
pub use feature_configs::CurrentTimeReminderDeliveryMode;
pub use feature_configs::CurrentTimeSource;
pub use feature_configs::GuardianElasticSectionConfigToml;
pub use feature_configs::GuardianV2ConfigToml;
pub use feature_configs::GuardianV2ReviewScopeConfigToml;
pub use feature_configs::GuardianV2TranscriptConfigToml;
pub use feature_configs::GuardianV2TranscriptSource;
pub use feature_configs::GuardianV3ConfigToml;
pub use feature_configs::MultiAgentV2ConfigToml;
pub use feature_configs::NetworkProxyConfigToml;
pub use feature_configs::NetworkProxyDomainPermissionToml;
pub use feature_configs::NetworkProxyModeToml;
pub use feature_configs::NetworkProxyUnixSocketPermissionToml;
pub use feature_configs::NonPrefixedMcpToolNamesConfigToml;
pub use feature_configs::RemoteMessageBoardConfigToml;
pub use feature_configs::RolloutBudgetConfigToml;
pub use feature_configs::SleepToolConfigToml;
pub use feature_configs::SleepToolMode;
pub use feature_configs::TokenBudgetConfigToml;
pub use feature_configs::ToolRegistryConfigToml;
pub use runtime_auth::RuntimeAgentInstancesConfigToml;
pub use runtime_auth::RuntimeAuthMode;

/// High-level lifecycle stage for a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Features that are still under development, not ready for external use
    UnderDevelopment,
    /// Experimental features made available to users through the `/experimental` menu
    Experimental {
        name: &'static str,
        menu_description: &'static str,
        announcement: &'static str,
    },
    /// Stable features. The feature flag is kept for ad-hoc enabling/disabling
    Stable,
}

impl Stage {
    pub fn experimental_menu_name(self) -> Option<&'static str> {
        match self {
            Stage::Experimental { name, .. } => Some(name),
            Stage::UnderDevelopment | Stage::Stable => None,
        }
    }

    pub fn experimental_menu_description(self) -> Option<&'static str> {
        match self {
            Stage::Experimental {
                menu_description, ..
            } => Some(menu_description),
            Stage::UnderDevelopment | Stage::Stable => None,
        }
    }

    pub fn experimental_announcement(self) -> Option<&'static str> {
        match self {
            Stage::Experimental {
                announcement: "", ..
            } => None,
            Stage::Experimental { announcement, .. } => Some(announcement),
            Stage::UnderDevelopment | Stage::Stable => None,
        }
    }
}

/// Unique features toggled via configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// Preview consumer five-hour and weekly allowance history.
    AnalyticsPlanHistory,
    /// Discover model catalogs for OpenAI API-key authentication.
    ApiKeyModelDiscovery,
    /// Forward explicit programs with builtin OpenAI API keys.
    ApiKeyCyberAccessPrograms,
    /// Enable Daybreak controls and automatic access-program selection in CLI clients.
    CliDaybreak,
    /// Capture the saved Daybreak choice for a native helper and its turns.
    DaybreakProgramPropagation,
    /// Restrict CLI resume of attributed local threads to their creator.
    CliResumeAccountCheck,
    // Stable.
    /// Enable the default shell tool.
    ShellTool,
    /// Enable the built-in local image viewer.
    ViewImage,
    /// Allow registration of the built-in sleep tool.
    SleepTool,
    /// Enable Claude-style lifecycle hooks loaded from hooks.json files.
    CodexHooks,
    /// Store CLI auth in the encrypted local secrets backend when keyring storage is selected.
    SecretAuthStorage,

    /// Automatically start the shared local daemon for eligible interactive launches.
    DaemonAutoStart,
    /// Allow the TUI to discover and select among multiple conversation backends.
    TuiMultiBackend,
    /// Enable workspace behavior for side conversations in the owned TUI.
    TuiWorkspace,
    /// Enable experimental MCP status line providers.
    TuiExtensionsStatusLine,
    /// Enable negotiated zstd compression for app-server WebSocket and UDS connections.
    AppServerWebSocketCompression,

    // Experimental
    /// Copy files between explicitly selected execution environments.
    MultiEnvCopyFile,
    /// Send per-content-entry classifications in internal Responses metadata.
    ContentItemKinds,
    /// Render client-supplied untrusted context as standalone named tool outputs.
    UntrustedAsToolOutput,
    /// Record model-attempted tool calls in internal Responses metadata.
    ExecutedToolCallMetadata,
    /// Enable JavaScript code mode backed by the standalone host process.
    CodeMode,
    /// Expose ranked tool discovery inside JavaScript code mode.
    CodeModeToolSearch,
    /// Run JavaScript code mode in the standalone host process.
    CodeModeHost,
    /// Truncate output in the process-owned code-mode host before sending it to Core.
    CodeModeHostOutputTruncation,
    /// Establish the code-mode host connection during session startup.
    CodeModePrewarm,
    /// Terminate active code mode cells when their turn is interrupted.
    CodeModeInterrupt,
    /// Restrict model-visible tools to code mode entrypoints (`exec`, `wait`).
    CodeModeOnly,
    /// Keep eligible MCP/app and dynamic tools deferred in exec, only in Code Mode Only.
    /// Ignores deferLoading, omit_tools_from, and namespace skip settings.
    CodeModeOnlyStrictThirdPartyTools,
    /// Use the single unified PTY-backed exec tool.
    UnifiedExec,
    /// Allow unified exec commands to allocate an interactive terminal.
    UnifiedExecTty,
    /// Add terminal-specific visualization guidance to TUI developer instructions.
    TerminalVisualizationInstructions,
    /// Stream structured progress while apply_patch input is being generated.
    ApplyPatchStreamingEvents,
    /// Allow exec tools to request additional permissions while staying sandboxed.
    ExecPermissionApprovals,
    /// Require approval before writing input to escalated unified-exec terminals.
    WriteStdinApproval,
    /// Expose the built-in request_permissions tool.
    RequestPermissionsTool,
    /// Expose the extension-backed standalone web search tool.
    StandaloneWebSearch,
    /// Let the host authorize MCP search independently of native search through per-turn policy.
    HostedSearchPolicy,
    /// Experimental shell snapshotting.
    ShellSnapshot,
    /// Restore bundled tools to PATH after Codex's login shell starts.
    LoginShellPackagePath,
    /// Expose the selected PowerShell execution host's bounded major/minor version.
    PowerShellShellVersion,
    /// Keep policy-filtered shell snapshots entirely in executor memory.
    ShellSnapshotV2,
    /// Allow turns to start while selected executors are still starting.
    DeferredExecutor,
    /// Use the current working directory for turn diff display paths.
    CwdRelativeTurnDiffs,
    /// Enable runtime metrics snapshots via a manual reader.
    RuntimeMetrics,
    /// Capture internal direct-tool dependency paths for turn attribution.
    CriticalPathTiming,
    /// Enable startup memory extraction and file-backed memory consolidation.
    MemoryTool,
    /// Enable importing project-scoped memory from external agents.
    ExternalAgentMemoryImport,
    /// Compress cold local thread-store rollout files, including shared histories.
    /// Requires every reader of the Codex home to support compressed shared histories.
    LocalThreadStoreCompression,
    /// Migrate legacy local rollout files to paginated history in the background.
    BackgroundPaginatedRolloutMigration,
    /// Enable the Chronicle sidecar for passive screen-context memories.
    Chronicle,
    /// Compress request bodies (zstd) when sending streaming requests to codex-backend.
    EnableRequestCompression,
    /// Keep active sampling turns alive until a failed network connection recovers.
    UnboundedConnectionRetries,
    /// Start the managed network proxy for sandboxed sessions.
    NetworkProxy,
    /// Mask credentials and inject them through an already enabled network proxy.
    CredentialMasking,
    /// Respect host system proxy settings for Codex-owned network clients.
    RespectSystemProxy,
    /// Retry eligible bootstrap requests through the system proxy after normal routing fails.
    SystemProxyFallback,
    /// Enable task-path-based multi-agent routing.
    MultiAgentV2,
    /// Keep spawn model choices in append-only context instead of tool descriptions.
    ModelCatalogInContext,
    /// Inherit client-defined dynamic tools in fresh V2 subagents.
    MultiAgentV2DynamicTools,
    /// Inherit configured thread environments instead of temporary turn selections.
    SubagentThreadEnvironments,
    /// Use each subagent model's default context window and auto-compaction threshold.
    SubagentDefaultContextLimits,
    /// Keep sampling through reasoning and commentary boundaries when agent mail arrives.
    /// Pending mail is delivered at the next normal input boundary instead.
    DeferMailboxPreemption,
    /// Allow agent messages to wake idle root sessions after a completed turn.
    WakeOnAgentMessages,
    /// Preempt responses and yield foreground code-mode observations on new user input.
    InstantInterrupt,
    /// Interrupt upstream inference when a turn is cancelled.
    InterruptOnCancel,
    /// Enable shared discussion tools for an agent tree.
    AgentMessageBoard,
    /// Expose a tree-scoped in-memory message board through HTTP MCP.
    AgentMessageBoardMcp,
    /// Enable apps.
    Apps,
    /// Route first-party ChatGPT requests through PSP.
    Psp,
    /// Enable MCP apps.
    EnableMcpApps,
    /// Enable MCP protocol version 2026-07-28 support.
    Mcp20260728,
    /// Honor ordinary MCP server tool-list notifications within a turn.
    McpToolListChanged,
    /// Discover and share eligible local stdio connections across chats.
    McpConnectionSharing,
    /// Defer optional stdio server startup in regular chats with a warm tool catalog.
    McpLazyStdioStartup,
    /// Enable MCP protocol version 2026-07-28 for the host-owned Codex Apps server.
    CodexAppsMcp20260728,
    /// Let host-owned Apps catalog reads overlap the initial tool inventory.
    AppsEarlyResourceDiscovery,
    /// Let RMCP coordinate OAuth refresh through the configured credential store.
    McpOAuthRefreshCoordination,
    /// Enable enterprise refresh-token authorization for configured MCP resources.
    UseXaa,
    /// Describe deferred tool namespaces in the model-visible world state.
    DeferredToolWorldState,
    /// Publish retained command sessions through incremental world state.
    CommandSessionsWorldState,
    /// Track top-level tool definitions in world state and emit incremental context updates.
    IncrementalTools,
    /// Suppress the model ID announcement in incremental context after a model switch.
    SuppressModelChangeAnnouncements,
    /// Preserve inherited tool declarations in MCP-disabled prediction forks.
    PredictionPreserveParentTools,
    /// Diff CodeModeOnly exec definitions within the incremental tool catalog.
    IncrementalExecTools,
    /// Expose MCP model-visible namespaces without the legacy `mcp__` prefix.
    NonPrefixedMcpToolNames,
    /// Enable discoverable tool suggestions for apps.
    ToolSuggest,
    /// Include recommended plugins in model-visible context.
    RecommendedPlugins,
    /// Enable plugins.
    Plugins,
    /// Use concise developer hints for explicitly selected plugins.
    SimplifiedSelectionHints,
    /// Discover selected-root plugin and skill manifests through one high-level exec-server RPC.
    ExecutorCapabilityDiscovery,
    /// Acquire V2 executor inventories without publishing or activating their capabilities.
    ExecutorCapabilityDiscoveryV2,
    /// Skip host skill snapshots when no registered contributor requires them.
    SkipHostSkillDiscovery,
    /// Allow the in-app browser pane in desktop apps.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    InAppBrowser,
    /// Allow websites to open and customize annotation tools in desktop apps.
    /// Ordinary user-driven annotation is independent of this gate.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    BrowserAnnotationApi,
    /// Allow the in-app chat pane in desktop apps.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    InAppChat,
    /// Allow desktop apps to run local automations.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    InAppLocalAutomation,
    /// Allow desktop apps to perform in-app updates.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    InAppUpdates,
    /// Allow Browser Use agent integration in desktop apps.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    BrowserUse,
    /// Allow Browser Use integration to access the full Chrome DevTools Protocol surface.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    BrowserUseFullCdpAccess,
    /// Allow Browser Use integration with external browsers.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    BrowserUseExternal,
    /// Allow Codex Computer Use.
    ///
    /// Requirements-only gate: this should be set from requirements, not user config.
    ComputerUse,


    /// Enable extension-backed image generation.
    ImageGeneration,
    /// Allow the image-generation tool to request a backend-assembled GIF.
    ImageGenerationAnimation,
    /// Omit inline image and audio content from app-server item notifications.
    OmitAppServerNotificationMedia,
    /// Tell the model when a prompt image was resized and include its dimensions.
    ImageResizeNotice,
    /// Apply one shared pixel and token budget to every image, regardless of legacy detail hints.
    UnifiedImageBudget,
    /// Request sequential cutoff reasoning summary delivery.
    ConcurrentReasoningSummaries,
    /// Request encrypted sampled output for token-preserving replay.
    OutputTokenReplay,
    /// Allow prompting and installing missing MCP dependencies.
    SkillMcpDependencyInstall,
    /// Omit descriptions from the model-visible available-skills catalog.
    OmitSkillDescriptions,
    /// Run cheap skill-search methods in shadow mode and emit experiment metrics.
    SkillSearch,
    /// Append skill additions and edits within a cumulative context budget.
    SkillCatalogDeltas,
    /// Enable the unified mention popup used by default in the TUI.
    MentionsV2,
    /// Allow request_user_input in Default collaboration mode.
    DefaultModeRequestUserInput,
    /// Allow root agents to send async user messages without model catalog support.
    SendMessageToUserAsync,
    /// Enable automatic review for approval prompts.
    GuardianApproval,
    /// Include standalone additional-context tool outputs in Guardian review history.
    GuardianIncludeAdditionalContext,
    /// Forward private reviewer items and interruptions to raw-event observers.
    GuardianRecordedItems,
    /// Reuse encrypted parent compaction when restarting Guardian review sessions.
    /// When disabled, retain an independent review transcript across parent compaction.
    GuardianReuseParentCompaction,
    /// Limit worker Guardian root evidence to preceding root communication windows.
    GuardianRootHandoffContext,
    /// Review state-changing MAv2 calls and preserve native calls in Guardian history.
    GuardianNativeMultiAgentHistory,
    /// Include completed node_repl or cua_repl Code Mode responses in Guardian reviews.
    GuardianEnhancedNodeReplTranscripts,
    /// Include completed node_repl or cua_repl Code Mode response images in Guardian reviews.
    GuardianNodeReplTranscriptImages,
    /// Trust connector identities from the plugin service orchestrator in Guardian V2.
    GuardianTrustOrchestratorConnectors,
    /// Give Guardian access to the root conversation's message history tools.
    GuardianConversationHistoryTools,
    /// Resample a synchronous Guardian denial once before publishing it.
    GuardianDenialReconsideration,
    /// Enable Guardian V2 automatic approval reviews.
    GuardianV2,
    /// Run Decisions alongside Guardian V2 for measurement without changing approvals.
    GuardianV2DecisionsComparison,
    /// Enable Guardian V3 integrations.
    GuardianV3,
    /// Enable persisted thread goals and automatic goal continuation.
    Goals,
    /// Add current context-window metadata to model-visible context.
    TokenBudget,
    /// Enables experimental context management.
    ContextManagement,
    /// Track and report a shared token budget across a session's agent threads.
    RolloutBudget,
    /// Append trusted response configuration items when the selected reasoning effort changes.
    ReasoningEffortOverride,
    /// Add current-time reminders to model-visible context.
    CurrentTimeReminder,
    /// Report failed clock reads to the model without failing the turn.
    NonfatalClockReadErrors,
    /// Route MCP tool approval prompts through the MCP elicitation request path.
    ToolCallMcpElicitation,
    /// Prompt Codex Apps connector auth failures through MCP URL elicitations.
    AuthElicitation,
    /// Offer custom gateway setup during TUI sign-in onboarding.
    GatewaySetupWizard,
    /// Enable native artifact tools.
    Artifact,
    /// Enable Fast mode selection in the TUI and request layer.
    FastMode,
    /// Enable Ultra Fast mode independently of Fast mode.
    UltrafastMode,
    /// Enable explicitly requested model changes for later step captures.
    StepModelSwitching,
    /// Prevent idle system sleep while a turn is actively running.
    PreventIdleSleep,
    /// Prepare remote compaction in the background before the foreground limit is reached.
    AsyncCompaction,
    /// Restrict background compaction to the root session.
    AsyncCompactionRootOnly,
    /// Include retained images in the remote compaction context budget.
    CompactionImageBudget,
    /// Retain client-authored developer messages across compacted context windows.
    RetainClientDeveloperMessages,
    /// Honor client-requested tool-output retention across context windows.
    RetainClientToolOutputs,
    /// Use Agent Identity for ChatGPT-authenticated sessions.
    UseAgentIdentity,
    /// Register an ephemeral runtime for OpenAI request authentication.
    RuntimeAgentInstances,
    /// Enable workspace dependency support.
    WorkspaceDependencies,


}

impl Feature {
    pub fn key(self) -> &'static str {
        self.info().key
    }

    pub fn stage(self) -> Stage {
        self.info().stage
    }

    pub fn default_enabled(self) -> bool {
        self.info().default_enabled
    }

    fn info(self) -> &'static FeatureSpec {
        FEATURES
            .iter()
            .find(|spec| spec.id == self)
            .unwrap_or_else(|| unreachable!("missing FeatureSpec for {self:?}"))
    }
}

/// Holds the effective set of enabled features.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Features {
    enabled: BTreeSet<Feature>,
}

impl Features {
    /// Starts with built-in defaults.
    pub fn with_defaults() -> Self {
        let mut set = BTreeSet::new();
        for spec in FEATURES {
            if spec.default_enabled {
                set.insert(spec.id);
            }
        }
        Self {
            enabled: set,
        }
    }

    pub fn enabled(&self, f: Feature) -> bool {
        self.enabled.contains(&f)
    }

    /// Whether to advertise GIF output in the image-generation tool.
    pub fn image_generation_animation_enabled(&self) -> bool {
        self.enabled(Feature::ImageGenerationAnimation)
    }

    /// Whether a routing tier is enabled, independently of model catalog support.
    pub fn service_tier_enabled(&self, service_tier: &str) -> bool {
        match service_tier {
            "flex" => true,
            "ultrafast" => self.enabled(Feature::UltrafastMode),
            _ => self.enabled(Feature::FastMode),
        }
    }

    /// Returns whether persistent execution is enabled for the selected effort.
    pub fn persistent_execution_enabled(&self, reasoning_effort: Option<&ReasoningEffort>) -> bool {
        reasoning_effort == Some(&ReasoningEffort::Persistent)
    }

    pub fn apps_enabled_for_auth(&self, has_chatgpt_auth: bool) -> bool {
        self.enabled(Feature::Apps) && has_chatgpt_auth
    }

    pub fn plugin_recommendations_enabled(&self) -> bool {
        self.enabled(Feature::Apps)
            && self.enabled(Feature::Plugins)
            && (self.enabled(Feature::ToolSuggest) || self.enabled(Feature::RecommendedPlugins))
    }

    pub fn enable(&mut self, f: Feature) -> &mut Self {
        self.enabled.insert(f);
        self
    }

    pub fn disable(&mut self, f: Feature) -> &mut Self {
        self.enabled.remove(&f);
        self
    }

    pub fn set_enabled(&mut self, f: Feature, enabled: bool) -> &mut Self {
        if enabled {
            self.enable(f)
        } else {
            self.disable(f)
        }
    }

    pub fn apply_map(&mut self, entries: &BTreeMap<String, bool>) {
        for (key, enabled) in entries {
            if let Some(feature) = feature_for_key(key) {
                self.set_enabled(feature, *enabled);
            } else {
                tracing::warn!("unknown feature key in config: {key}");
            }
        }
    }

    pub fn from_sources(base: Option<&FeaturesToml>, profile: Option<&FeaturesToml>) -> Self {
        let mut features = Self::with_defaults();
        for source in [base, profile].into_iter().flatten() {
            features.apply_toml(source);
        }
        features.normalize_dependencies();
        features
    }

    pub fn enabled_features(&self) -> Vec<Feature> {
        self.enabled.iter().copied().collect()
    }

    pub fn normalize_dependencies(&mut self) {
        if self.enabled(Feature::CodeModeOnly) && !self.enabled(Feature::CodeMode) {
            self.enable(Feature::CodeMode);
        }
    }
}

pub fn feature_for_key(key: &str) -> Option<Feature> {
    FEATURES
        .iter()
        .find(|spec| spec.key == key)
        .map(|spec| spec.id)
}

/// Returns `true` if the provided string matches a known `[features]` key.
pub fn is_known_feature_key(key: &str) -> bool {
    key == "tool_registry" || feature_for_key(key).is_some()
}

/// Deserializable features table for TOML.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
pub struct FeaturesToml {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_registry: Option<ToolRegistryConfigToml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_mode: Option<FeatureToml<CodeModeConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_mode_host: Option<FeatureToml<CodeModeHostConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_prefixed_mcp_tool_names: Option<FeatureToml<NonPrefixedMcpToolNamesConfigToml>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "feature_configs::deserialize_guardian_v2_feature"
    )]
    pub guardianv2: Option<FeatureToml<GuardianV2ConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardianv3: Option<FeatureToml<GuardianV3ConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_agent_v2: Option<FeatureToml<MultiAgentV2ConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<FeatureToml<TokenBudgetConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_compaction: Option<FeatureToml<AsyncCompactionConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<FeatureToml<ContextManagementConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollout_budget: Option<FeatureToml<RolloutBudgetConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_time_reminder: Option<FeatureToml<CurrentTimeReminderConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep_tool: Option<FeatureToml<SleepToolConfigToml>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_agent_instances: Option<FeatureToml<RuntimeAgentInstancesConfigToml>>,
    pub network_proxy: Option<FeatureToml<NetworkProxyConfigToml>>,
    /// Boolean feature toggles keyed by feature name.
    #[serde(flatten)]
    entries: BTreeMap<String, bool>,
}

impl Features {
    fn apply_toml(&mut self, features: &FeaturesToml) {
        let entries = features.entries();
        self.apply_map(&entries);
    }
}

impl FeaturesToml {
    pub fn entries(&self) -> BTreeMap<String, bool> {
        let mut entries = self.entries.clone();
        if let Some(enabled) = self
            .async_compaction
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::AsyncCompaction.key().to_string(), enabled);
        }
        if let Some(enabled) = self.code_mode.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::CodeMode.key().to_string(), enabled);
        }
        if let Some(enabled) = self.code_mode_host.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::CodeModeHost.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .non_prefixed_mcp_tool_names
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::NonPrefixedMcpToolNames.key().to_string(), enabled);
        }
        if let Some(enabled) = self.guardianv2.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::GuardianV2.key().to_string(), enabled);
        }
        if let Some(enabled) = self.guardianv3.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::GuardianV3.key().to_string(), enabled);
        }
        if let Some(enabled) = self.multi_agent_v2.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::MultiAgentV2.key().to_string(), enabled);
        }
        if let Some(enabled) = self.token_budget.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::TokenBudget.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .context_management
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::ContextManagement.key().to_string(), enabled);
        }
        if let Some(enabled) = self.rollout_budget.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::RolloutBudget.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .current_time_reminder
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::CurrentTimeReminder.key().to_string(), enabled);
        }
        if let Some(enabled) = self.network_proxy.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::NetworkProxy.key().to_string(), enabled);
        }
        if let Some(enabled) = self.sleep_tool.as_ref().and_then(FeatureToml::enabled) {
            entries.insert(Feature::SleepTool.key().to_string(), enabled);
        }
        if let Some(enabled) = self
            .runtime_agent_instances
            .as_ref()
            .and_then(FeatureToml::enabled)
        {
            entries.insert(Feature::RuntimeAgentInstances.key().to_string(), enabled);
        }
        entries
    }
}

impl From<BTreeMap<String, bool>> for FeaturesToml {
    fn from(entries: BTreeMap<String, bool>) -> Self {
        Self {
            entries,
            ..Default::default()
        }
    }
}

// To be used for features that need more configuration than just enabled/disabled and
// require a custom config struct under `[features]`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[serde(untagged)]
pub enum FeatureToml<T> {
    Enabled(bool),
    Config(T),
}

impl<T: FeatureConfig> FeatureToml<T> {
    pub fn enabled(&self) -> Option<bool> {
        match self {
            Self::Enabled(enabled) => Some(*enabled),
            Self::Config(config) => config.enabled(),
        }
    }
}

// A trait to be implemented by custom feature config structs when defining a feature that needs more configuration than
// just enabled/disabled.
pub trait FeatureConfig {
    fn enabled(&self) -> Option<bool>;
}

/// Single, easy-to-read registry of all feature definitions.
#[derive(Debug, Clone, Copy)]
pub struct FeatureSpec {
    pub id: Feature,
    pub key: &'static str,
    pub stage: Stage,
    pub default_enabled: bool,
}

pub const FEATURES: &[FeatureSpec] = &[
    FeatureSpec {
        id: Feature::AnalyticsPlanHistory,
        key: "analytics_plan_history",
        stage: Stage::Experimental {
            name: "Analytics plan history",
            menu_description: "Preview five-hour and weekly allowance history for consumer accounts in /analytics.",
            announcement: "",
        },
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CliResumeAccountCheck,
        key: "cli_resume_account_check",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DaemonAutoStart,
        key: "daemon_auto_start",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::TuiWorkspace,
        key: "tui_workspace",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::TuiMultiBackend,
        key: "tui_multi_backend",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::TuiExtensionsStatusLine,
        key: "tui_extensions_statusline",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AppServerWebSocketCompression,
        key: "app_server_websocket_compression",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    // Stable features.
    FeatureSpec {
        id: Feature::ShellTool,
        key: "shell_tool",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ViewImage,
        key: "view_image",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::MultiEnvCopyFile,
        key: "multi_env_copy_file",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SleepTool,
        key: "sleep_tool",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::SecretAuthStorage,
        key: "secret_auth_storage",
        stage: Stage::Stable,
        default_enabled: cfg!(windows),
    },
    FeatureSpec {
        id: Feature::UnifiedExec,
        key: "unified_exec",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UnifiedExecTty,
        key: "unified_exec_tty",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ShellSnapshot,
        key: "shell_snapshot",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::LoginShellPackagePath,
        key: "login_shell_package_path",
        stage: Stage::Experimental {
            name: "Bundled tools in login shells",
            menu_description: "Keep bundled tools such as ripgrep available when login shell startup resets PATH.",
            announcement: "",
        },
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::PowerShellShellVersion,
        key: "powershell_shell_version",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ShellSnapshotV2,
        key: "shell_snapshot_v2",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DeferredExecutor,
        key: "deferred_executor",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CwdRelativeTurnDiffs,
        key: "cwd_relative_turn_diffs",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ContentItemKinds,
        key: "content_item_kinds",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UntrustedAsToolOutput,
        key: "untrusted_as_tool_output",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ExecutedToolCallMetadata,
        key: "executed_tool_call_metadata",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeMode,
        key: "code_mode",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeToolSearch,
        key: "code_mode_tool_search",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeHost,
        key: "code_mode_host",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CodeModeHostOutputTruncation,
        key: "code_mode_host_output_truncation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CodeModePrewarm,
        key: "code_mode_prewarm",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeInterrupt,
        key: "code_mode_interrupt",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::InstantInterrupt,
        key: "instant_interrupt",
        stage: Stage::UnderDevelopment,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::InterruptOnCancel,
        key: "interrupt_on_cancel",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeOnly,
        key: "code_mode_only",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodeModeOnlyStrictThirdPartyTools,
        key: "code_mode_only_strict_3p_tools",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::StandaloneWebSearch,
        key: "standalone_web_search",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::HostedSearchPolicy,
        key: "hosted_search_policy",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RuntimeMetrics,
        key: "runtime_metrics",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CriticalPathTiming,
        key: "critical_path_timing",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::MemoryTool,
        key: "memories",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ExternalAgentMemoryImport,
        key: "external_agent_memory_import",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::LocalThreadStoreCompression,
        key: "local_thread_store_compression",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::BackgroundPaginatedRolloutMigration,
        key: "background_paginated_rollout_migration",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Chronicle,
        key: "chronicle",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ApplyPatchStreamingEvents,
        key: "apply_patch_streaming_events",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ExecPermissionApprovals,
        key: "exec_permission_approvals",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WriteStdinApproval,
        key: "write_stdin_approval",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CodexHooks,
        key: "hooks",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::RequestPermissionsTool,
        key: "request_permissions_tool",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ApiKeyModelDiscovery,
        key: "api_key_model_discovery",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CliDaybreak,
        key: "cli_daybreak",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ApiKeyCyberAccessPrograms,
        key: "api_key_cyber_access_programs",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::DaybreakProgramPropagation,
        key: "daybreak_program_propagation",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::EnableRequestCompression,
        key: "enable_request_compression",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UnboundedConnectionRetries,
        key: "unbounded_connection_retries",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::NetworkProxy,
        key: "network_proxy",
        stage: Stage::Experimental {
            name: "Network proxy",
            menu_description: "Apply network proxy restrictions to sandboxed sessions that already have network access.",
            announcement: "NEW: Network proxy can now be enabled from /experimental. Restart Codex after enabling it.",
        },
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CredentialMasking,
        key: "credential_masking",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RespectSystemProxy,
        key: "respect_system_proxy",
        stage: Stage::Experimental {
            name: "System proxy",
            menu_description: "Use macOS or Windows system proxy settings for Codex requests.",
            announcement: "",
        },
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SystemProxyFallback,
        key: "system_proxy_fallback",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::MultiAgentV2,
        key: "multi_agent_v2",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ModelCatalogInContext,
        key: "model_catalog_in_context",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::MultiAgentV2DynamicTools,
        key: "multi_agent_v2_dynamic_tools",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SubagentThreadEnvironments,
        key: "subagent_thread_environments",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SubagentDefaultContextLimits,
        key: "subagent_default_context_limits",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DeferMailboxPreemption,
        key: "defer_mailbox_preemption",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WakeOnAgentMessages,
        key: "wake_on_agent_messages",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AgentMessageBoard,
        key: "agent_message_board",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AgentMessageBoardMcp,
        key: "agent_message_board_mcp",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Apps,
        key: "apps",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::Psp,
        key: "psp",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::EnableMcpApps,
        key: "enable_mcp_apps",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Mcp20260728,
        key: "mcp_2026_07_28",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::McpToolListChanged,
        key: "mcp_tool_list_changed",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::McpConnectionSharing,
        key: "mcp_connection_sharing",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::McpLazyStdioStartup,
        key: "mcp_lazy_stdio_startup",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AppsEarlyResourceDiscovery,
        key: "apps_early_resource_discovery",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CodexAppsMcp20260728,
        key: "codex_apps_mcp_2026_07_28",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::McpOAuthRefreshCoordination,
        key: "mcp_oauth_refresh_coordination",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UseXaa,
        key: "use_xaa",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::DeferredToolWorldState,
        key: "deferred_tool_world_state",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CommandSessionsWorldState,
        key: "command_sessions_world_state",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::IncrementalTools,
        key: "incremental_tools",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::SuppressModelChangeAnnouncements,
        key: "suppress_model_change_announcements",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::PredictionPreserveParentTools,
        key: "prediction_preserve_parent_tools",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::IncrementalExecTools,
        key: "incremental_exec_tools",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::NonPrefixedMcpToolNames,
        key: "non_prefixed_mcp_tool_names",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ToolSuggest,
        key: "tool_suggest",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::RecommendedPlugins,
        key: "recommended_plugins",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Plugins,
        key: "plugins",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::SimplifiedSelectionHints,
        key: "simplified_selection_hints",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ExecutorCapabilityDiscovery,
        key: "executor_capability_discovery",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ExecutorCapabilityDiscoveryV2,
        key: "executor_capability_discovery_v2",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SkipHostSkillDiscovery,
        key: "skip_host_skill_discovery",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::InAppBrowser,
        key: "in_app_browser",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::BrowserAnnotationApi,
        key: "browser_annotation_api",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::InAppChat,
        key: "in_app_chat",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::InAppLocalAutomation,
        key: "in_app_local_automation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::InAppUpdates,
        key: "in_app_updates",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::BrowserUse,
        key: "browser_use",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::BrowserUseFullCdpAccess,
        key: "browser_use_full_cdp_access",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::BrowserUseExternal,
        key: "browser_use_external",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ComputerUse,
        key: "computer_use",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ImageGeneration,
        key: "image_generation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ImageGenerationAnimation,
        key: "image_generation_animation",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::OmitAppServerNotificationMedia,
        key: "omit_app_server_notification_media",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ImageResizeNotice,
        key: "image_resize_notice",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::UnifiedImageBudget,
        key: "unified_image_budget",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::ConcurrentReasoningSummaries,
        key: "concurrent_reasoning_summaries",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::OutputTokenReplay,
        key: "output_token_replay",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SkillMcpDependencyInstall,
        key: "skill_mcp_dependency_install",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::OmitSkillDescriptions,
        key: "omit_skill_descriptions",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SkillSearch,
        key: "skill_search",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::SkillCatalogDeltas,
        key: "skill_catalog_deltas",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::MentionsV2,
        key: "mentions_v2",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::DefaultModeRequestUserInput,
        key: "default_mode_request_user_input",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::SendMessageToUserAsync,
        key: "send_message_to_user_async",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::TerminalVisualizationInstructions,
        key: "terminal_visualization_instructions",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianApproval,
        key: "guardian_approval",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::GuardianIncludeAdditionalContext,
        key: "guardian_include_additional_context",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::GuardianRecordedItems,
        key: "guardian_recorded_items",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianReuseParentCompaction,
        key: "guardian_reuse_parent_compaction",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::GuardianRootHandoffContext,
        key: "guardian_root_handoff_context",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianNativeMultiAgentHistory,
        key: "guardian_native_multi_agent_history",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianEnhancedNodeReplTranscripts,
        key: "guardian_enhanced_node_repl_transcripts",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianNodeReplTranscriptImages,
        key: "guardian_node_repl_transcript_images",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianTrustOrchestratorConnectors,
        key: "guardian_trust_orchestrator_connectors",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianConversationHistoryTools,
        key: "guardian_conversation_history_tools",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianDenialReconsideration,
        key: "guardian_denial_reconsideration",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianV2,
        key: "guardianv2",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianV2DecisionsComparison,
        key: "guardianv2_decisions_comparison",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::GuardianV3,
        key: "guardianv3",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Goals,
        key: "goals",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::TokenBudget,
        key: "token_budget",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ContextManagement,
        key: "context_management",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RolloutBudget,
        key: "rollout_budget",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ReasoningEffortOverride,
        key: "reasoning_effort_override",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::CurrentTimeReminder,
        key: "current_time_reminder",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::NonfatalClockReadErrors,
        key: "nonfatal_clock_read_errors",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::ToolCallMcpElicitation,
        key: "tool_call_mcp_elicitation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::AuthElicitation,
        key: "auth_elicitation",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::GatewaySetupWizard,
        key: "gateway_setup_wizard",
        stage: Stage::Stable,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::Artifact,
        key: "artifact",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::FastMode,
        key: "fast_mode",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UltrafastMode,
        key: "ultrafast_mode",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::StepModelSwitching,
        key: "step_model_switching",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::PreventIdleSleep,
        key: "prevent_idle_sleep",
        stage: if cfg!(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )) {
            Stage::Experimental {
                name: "Prevent sleep while running",
                menu_description: "Keep your computer awake while Codex is running a thread.",
                announcement: "NEW: Prevent sleep while running is now available in /experimental.",
            }
        } else {
            Stage::UnderDevelopment
        },
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AsyncCompaction,
        key: "async_compaction",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::AsyncCompactionRootOnly,
        key: "async_compaction_root_only",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::CompactionImageBudget,
        key: "compaction_image_budget",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::RetainClientDeveloperMessages,
        key: "retain_client_developer_messages",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RetainClientToolOutputs,
        key: "retain_client_tool_outputs",
        stage: Stage::Stable,
        default_enabled: true,
    },
    FeatureSpec {
        id: Feature::UseAgentIdentity,
        key: "use_agent_identity",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::RuntimeAgentInstances,
        key: "runtime_agent_instances",
        stage: Stage::UnderDevelopment,
        default_enabled: false,
    },
    FeatureSpec {
        id: Feature::WorkspaceDependencies,
        key: "workspace_dependencies",
        stage: Stage::Stable,
        default_enabled: true,
    },
];

pub fn unstable_features_warning_event(
    effective_features: Option<&Table>,
    suppress_unstable_features_warning: bool,
    features: &Features,
    config_path: &str,
) -> Option<Event> {
    if suppress_unstable_features_warning {
        return None;
    }

    let mut under_development_feature_keys = Vec::new();
    if let Some(table) = effective_features {
        for (key, value) in table {
            let is_enabled = value.as_bool() == Some(true)
                || value
                    .as_table()
                    .and_then(|table| table.get("enabled"))
                    .and_then(toml::Value::as_bool)
                    == Some(true);
            if !is_enabled {
                continue;
            }
            let Some(spec) = FEATURES.iter().find(|spec| spec.key == key.as_str()) else {
                continue;
            };
            if !features.enabled(spec.id) {
                continue;
            }
            if matches!(spec.stage, Stage::UnderDevelopment) {
                under_development_feature_keys.push(spec.key.to_string());
            }
        }
    }

    if under_development_feature_keys.is_empty() {
        return None;
    }

    under_development_feature_keys.sort();
    let under_development_feature_keys = under_development_feature_keys.join(", ");
    let message = format!(
        "Under-development features enabled: {under_development_feature_keys}. Under-development features are incomplete and may behave unpredictably. To suppress this warning, set `suppress_unstable_features_warning = true` in {config_path}."
    );
    Some(Event {
        id: String::new(),
        msg: EventMsg::Warning(WarningEvent { message }),
    })
}

#[cfg(test)]
mod tests;
