use super::config_processor::reload_user_config;
use super::*;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use codex_app_server_protocol::PluginAvailability;
use codex_config::types::McpServerConfig;
use codex_core::McpRefreshReason;
use codex_core_plugins::PluginMarketplaceContext;
use codex_core_plugins::is_openai_curated_marketplace_name;
use codex_core_plugins::loader::load_configured_plugin_mcp_servers;
use codex_core_plugins::manifest::is_agent_plugin_manifest;
use codex_mcp::McpOAuthLoginSupport;
use codex_mcp::McpRuntimeContext;
use codex_mcp::oauth_login_support;
use codex_mcp::resolve_oauth_callback;
use codex_mcp::should_retry_without_scopes;
use codex_plugin::PluginId;
use codex_rmcp_client::McpOAuthClientRegistration;
use codex_rmcp_client::OAuthDiscoveryTimeout;
use codex_rmcp_client::StreamableHttpRedirectMode;
use codex_rmcp_client::perform_oauth_login_silent;
use codex_utils_path_uri::LegacyAppPathString;
use codex_utils_path_uri::PathUri;

mod local;
mod search;

fn plugin_redirect_mode(plugin_root: &Path) -> StreamableHttpRedirectMode {
    if is_agent_plugin_manifest(plugin_root) {
        StreamableHttpRedirectMode::AgentPluginV1
    } else {
        StreamableHttpRedirectMode::Legacy
    }
}

#[derive(Clone)]
pub(crate) struct PluginRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,

    config_manager: ConfigManager,
    on_effective_plugins_changed:
        Arc<dyn Fn(codex_core_plugins::EffectivePluginsChange) + Send + Sync>,
}

fn plugin_skills_to_info<'a>(
    skills: impl IntoIterator<Item = &'a codex_skills::SkillMetadata>,
    disabled_skill_paths: &HashSet<PathUri>,
) -> Vec<SkillSummary> {
    skills
        .into_iter()
        .map(|skill| SkillSummary {
            name: skill.name.clone(),
            description: skill.description.clone(),
            short_description: skill.short_description.clone(),
            interface: skill.interface.clone().map(|interface| {
                codex_app_server_protocol::SkillInterface {
                    display_name: interface.display_name,
                    short_description: interface.short_description,
                    icon_small: interface.icon_small,
                    icon_large: interface.icon_large,
                    icon_small_url: None,
                    icon_large_url: None,
                    brand_color: interface.brand_color,
                    default_prompt: interface.default_prompt,
                }
            }),
            path: Some(skill.path_to_skills_md.clone().into()),
            enabled: !disabled_skill_paths.contains(&skill.path_to_skills_md),
        })
        .collect()
}

fn local_plugin_interface_to_info(interface: PluginManifestInterface) -> PluginInterface {
    PluginInterface {
        display_name: interface.display_name,
        short_description: interface.short_description,
        long_description: interface.long_description,
        developer_name: interface.developer_name,
        category: interface.category,
        capabilities: interface.capabilities,
        website_url: interface.website_url,
        privacy_policy_url: interface.privacy_policy_url,
        terms_of_service_url: interface.terms_of_service_url,
        default_prompt: interface.default_prompt,
        brand_color: interface.brand_color,
        composer_icon: interface.composer_icon,
        composer_icon_url: None,
        logo: interface.logo,
        logo_dark: interface.logo_dark,
        logo_url: None,
        logo_url_dark: None,
        screenshots: interface.screenshots,
        screenshot_urls: Vec::new(),
    }
}

fn marketplace_plugin_source_to_info(source: MarketplacePluginSource) -> PluginSource {
    match source {
        MarketplacePluginSource::Local { path } => PluginSource::Local { path },
        MarketplacePluginSource::Git {
            url,
            path,
            ref_name,
            sha,
        } => PluginSource::Git {
            url,
            path,
            ref_name,
            sha,
        },
        MarketplacePluginSource::Npm {
            package,
            version,
            registry,
        } => PluginSource::Npm {
            package,
            version,
            registry,
        },
    }
}

fn convert_configured_marketplace_plugin_to_plugin_summary(
    plugin: codex_core_plugins::ConfiguredMarketplacePlugin,
) -> PluginSummary {
    PluginSummary {
        id: plugin.id,

        local_version: plugin.local_version,
        installed: plugin.installed,

        enabled: plugin.enabled,
        name: plugin.name,
        source: marketplace_plugin_source_to_info(plugin.source),
        install_policy: plugin.policy.installation.into(),

        auth_policy: plugin.policy.authentication.into(),
        availability: PluginAvailability::Available,

        interface: plugin.interface.map(local_plugin_interface_to_info),
        keywords: plugin.keywords,
    }
}

impl PluginRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,

        config_manager: ConfigManager,
        on_effective_plugins_changed: Arc<
            dyn Fn(codex_core_plugins::EffectivePluginsChange) + Send + Sync,
        >,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,

            config_manager,
            on_effective_plugins_changed,
        }
    }

    pub(crate) async fn plugin_list(
        &self,
        params: PluginListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_list_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_installed(
        &self,
        params: PluginInstalledParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_installed_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_read(
        &self,
        params: PluginReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_read_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_install(
        &self,
        params: PluginInstallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_install_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_uninstall(
        &self,
        params: PluginUninstallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_uninstall_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) fn effective_plugins_changed_callback(
        &self,
    ) -> Arc<dyn Fn(codex_core_plugins::EffectivePluginsChange) + Send + Sync> {
        Arc::clone(&self.on_effective_plugins_changed)
    }

    async fn on_effective_plugins_changed(&self, reason: McpRefreshReason) {
        self.clear_plugin_related_caches();
        self.thread_manager
            .invalidate_mcp_runtimes_for(reason)
            .await;
        self.thread_manager.refresh_hook_runtimes().await;
    }

    fn clear_plugin_related_caches(&self) {
        self.thread_manager.plugins_manager().clear_cache();
        self.thread_manager.skills_service().clear_cache();
    }

    async fn load_latest_config(
        &self,
        fallback_cwd: Option<PathBuf>,
    ) -> Result<Config, JSONRPCErrorError> {
        self.config_manager
            .load_latest_config(fallback_cwd)
            .await
            .map_err(|err| internal_error(format!("failed to reload config: {err}")))
    }

    async fn plugin_list_response(
        &self,
        params: PluginListParams,
    ) -> Result<PluginListResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginListParams {
            cwds,
            force_refetch,
        } = params;
        let roots = cwds.unwrap_or_default();
        let config = self.load_catalog_config(&roots).await?;
        let context = self.load_marketplace_context(roots, &config).await;
        let empty_response = || PluginListResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
        };
        if !context.plugins_enabled() && context.load_errors.is_empty() {
            return Ok(empty_response());
        }
        if force_refetch
            && plugins_manager
                .refresh_non_curated_plugin_cache_for_context(&context)
                .await
        {
            self.on_effective_plugins_changed(McpRefreshReason::NonCuratedPluginCacheRefreshed)
                .await;
        }
        let (data, marketplace_load_errors, local_marketplaces) = {
            let context_for_marketplace_listing = context.clone();
            let plugins_manager_for_marketplace_listing = plugins_manager.clone();
            match tokio::task::spawn_blocking(move || {
                let outcome = plugins_manager_for_marketplace_listing
                    .list_marketplaces_for_context(
                        &context_for_marketplace_listing,
                        /*include_openai_curated*/ true,
                    )?;
                let local_marketplaces = outcome.marketplaces.clone();
                Ok::<
                    (
                        Vec<PluginMarketplaceEntry>,
                        Vec<codex_app_server_protocol::MarketplaceLoadErrorInfo>,
                        Vec<codex_core_plugins::ConfiguredMarketplace>,
                    ),
                    MarketplaceError,
                >((
                    outcome
                        .marketplaces
                        .into_iter()
                        .map(|marketplace| PluginMarketplaceEntry {
                            name: marketplace.name,
                            path: Some(marketplace.path),
                            interface: marketplace.interface.map(|interface| {
                                MarketplaceInterface {
                                    display_name: interface.display_name,
                                }
                            }),
                            plugins: marketplace
                                .plugins
                                .into_iter()
                                .map(|plugin| {
                                    convert_configured_marketplace_plugin_to_plugin_summary(plugin)
                                })
                                .collect(),
                        })
                        .collect(),
                    outcome
                        .errors
                        .into_iter()
                        .map(|err| codex_app_server_protocol::MarketplaceLoadErrorInfo {
                            marketplace_path: err.path,
                            message: err.message,
                        })
                        .collect(),
                    local_marketplaces,
                ))
            })
            .await
            {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(err)) => {
                    return Err(Self::marketplace_error(err, "list marketplace plugins"));
                }
                Err(err) => {
                    return Err(internal_error(format!(
                        "failed to list marketplace plugins: {err}"
                    )));
                }
            }
        };

        plugins_manager.maybe_start_non_curated_plugin_cache_refresh(&context, &local_marketplaces);

        Ok(PluginListResponse {
            marketplaces: data,
            marketplace_load_errors,
        })
    }

    async fn plugin_installed_response(
        &self,
        params: PluginInstalledParams,
    ) -> Result<PluginInstalledResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginInstalledParams {
            cwds,
            install_suggestion_plugin_names,
        } = params;
        let roots = cwds.unwrap_or_default();
        let install_suggestion_plugin_names = install_suggestion_plugin_names
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();

        let empty_response = || PluginInstalledResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
        };
        let config = self.load_catalog_config(&roots).await?;
        let context = self.load_marketplace_context(roots, &config).await;
        if !context.plugins_enabled() && context.load_errors.is_empty() {
            return Ok(empty_response());
        }
        let (data, marketplace_load_errors) = self
            .load_local_installed_and_suggested_plugins(
                plugins_manager.clone(),
                &context,
                install_suggestion_plugin_names,
            )
            .await?;

        Ok(PluginInstalledResponse {
            marketplaces: data,
            marketplace_load_errors,
        })
    }

    async fn load_local_installed_and_suggested_plugins(
        &self,
        plugins_manager: Arc<codex_core_plugins::PluginsManager>,
        context: &PluginMarketplaceContext,
        install_suggestion_plugin_names: HashSet<String>,
    ) -> Result<
        (
            Vec<PluginMarketplaceEntry>,
            Vec<codex_app_server_protocol::MarketplaceLoadErrorInfo>,
        ),
        JSONRPCErrorError,
    > {
        let context = context.clone();
        match tokio::task::spawn_blocking(move || {
            let outcome = plugins_manager
                .list_marketplaces_for_context(&context, /*include_openai_curated*/ true)?;
            Ok::<
                (
                    Vec<PluginMarketplaceEntry>,
                    Vec<codex_app_server_protocol::MarketplaceLoadErrorInfo>,
                ),
                MarketplaceError,
            >((
                outcome
                    .marketplaces
                    .into_iter()
                    .filter_map(|marketplace| {
                        let plugins = marketplace
                            .plugins
                            .into_iter()
                            .filter(|plugin| {
                                plugin.installed
                                    || install_suggestion_plugin_names.contains(&plugin.name)
                            })
                            .map(|plugin| {
                                convert_configured_marketplace_plugin_to_plugin_summary(plugin)
                            })
                            .collect::<Vec<_>>();

                        (!plugins.is_empty()).then_some(PluginMarketplaceEntry {
                            name: marketplace.name,
                            path: Some(marketplace.path),
                            interface: marketplace.interface.map(|interface| {
                                MarketplaceInterface {
                                    display_name: interface.display_name,
                                }
                            }),
                            plugins,
                        })
                    })
                    .collect(),
                outcome
                    .errors
                    .into_iter()
                    .map(|err| codex_app_server_protocol::MarketplaceLoadErrorInfo {
                        marketplace_path: err.path,
                        message: err.message,
                    })
                    .collect(),
            ))
        })
        .await
        {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Self::marketplace_error(
                err,
                "list installed and suggested marketplace plugins",
            )),
            Err(err) => Err(internal_error(format!(
                "failed to list installed and suggested plugins: {err}"
            ))),
        }
    }

    async fn plugin_read_response(
        &self,
        params: PluginReadParams,
    ) -> Result<PluginReadResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginReadParams {
            marketplace_path,
            plugin_name,
        } = params;
        let marketplace_path = marketplace_path
            .ok_or_else(|| invalid_request("plugin/read requires a local marketplacePath"))?;
        let config = self
            .load_latest_config(marketplace_path.as_path().parent().map(Path::to_path_buf))
            .await?;
        let plugins_input = config.plugins_config_input();

        let plugin = {
            let request = PluginReadRequest {
                plugin_name,
                marketplace_path,
            };
            let outcome = plugins_manager
                .read_plugin_for_config(&plugins_input, &request)
                .await
                .map_err(|err| Self::marketplace_error(err, "read plugin details"))?;
            let app_summaries =
                plugin_app_summaries(&outcome.plugin.apps, &outcome.plugin.app_category_by_id);
            let visible_skills = outcome.plugin.skills.iter().filter(|skill| {
                skill.matches_product_restriction_for_product(
                    self.thread_manager.session_source().restriction_product(),
                )
            });
            let skills =
                plugin_skills_to_info(visible_skills, &outcome.plugin.disabled_skill_paths);
            let onboarding_skill = if outcome.plugin.enabled
                && let Some(path) = outcome.plugin.onboarding_skill.as_ref()
            {
                let path = PathUri::from_abs_path(path);
                skills
                    .iter()
                    .find(|skill| {
                        skill.enabled
                            && skill
                                .path
                                .as_ref()
                                .and_then(LegacyAppPathString::to_inferred_path_uri)
                                .as_ref()
                                == Some(&path)
                    })
                    .cloned()
            } else {
                None
            };
            PluginDetail {
                marketplace_name: outcome.marketplace_name,
                marketplace_path: outcome.marketplace_path,
                summary: PluginSummary {
                    id: outcome.plugin.id,

                    local_version: outcome.plugin.local_version,
                    name: outcome.plugin.name,
                    source: marketplace_plugin_source_to_info(outcome.plugin.source),
                    installed: outcome.plugin.installed,

                    enabled: outcome.plugin.enabled,
                    install_policy: outcome.plugin.policy.installation.into(),

                    auth_policy: outcome.plugin.policy.authentication.into(),
                    availability: PluginAvailability::Available,

                    interface: outcome.plugin.interface.map(local_plugin_interface_to_info),
                    keywords: outcome.plugin.keywords,
                },

                description: outcome.plugin.description,
                skills,
                onboarding_skill,
                hooks: outcome
                    .plugin
                    .hooks
                    .into_iter()
                    .map(|hook| codex_app_server_protocol::PluginHookSummary {
                        key: hook.key,
                        event_name: hook.event_name.into(),
                    })
                    .collect(),
                apps: app_summaries,
                app_templates: Vec::new(),
                mcp_servers: outcome.plugin.mcp_server_names,
                scheduled_tasks: None,
            }
        };

        Ok(PluginReadResponse { plugin })
    }

    async fn plugin_install_response(
        &self,
        params: PluginInstallParams,
    ) -> Result<PluginInstallResponse, JSONRPCErrorError> {
        let PluginInstallParams {
            marketplace_path,
            install_attempt_id: _,
            plugin_name,
        } = params;
        let marketplace_path = marketplace_path
            .ok_or_else(|| invalid_request("plugin/install requires a local marketplacePath"))?;
        let config_cwd = marketplace_path.as_path().parent().map(Path::to_path_buf);
        let mut config = self.load_latest_config(config_cwd.clone()).await?;
        let auth = match self.auth_manager.auth_with_http_client_factory().await {
            Some((auth, factory)) => {
                config.application_network_policy = factory.network_policy().clone();
                Some(auth)
            }
            None => None,
        };

        let plugins_manager = self.thread_manager.plugins_manager();
        let marketplace_display = marketplace_path.display().to_string();
        let plugin_name_for_log = plugin_name.clone();
        let request = PluginInstallRequest {
            plugin_name,
            marketplace_path,
        };

        let result = match plugins_manager
            .install_plugin(&config.config_layer_stack, request)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                warn!(
                    marketplace = %marketplace_display,
                    plugin_name = %plugin_name_for_log,
                    "failed to install plugin: {err}"
                );
                return Err(Self::plugin_install_error(err));
            }
        };
        let config = match self.load_latest_config(config_cwd).await {
            Ok(mut reloaded_config) => {
                // Keep the policy captured with this installation's auth snapshot.
                reloaded_config.application_network_policy = config.application_network_policy;
                reloaded_config
            }
            Err(err) => {
                warn!(
                    "failed to reload config after plugin install, using current config: {err:?}"
                );
                config
            }
        };

        self.clear_plugin_related_caches();
        reload_user_config(&self.config_manager, &self.thread_manager).await;
        self.thread_manager
            .invalidate_mcp_runtimes_for(McpRefreshReason::PluginInstalled)
            .await;
        self.thread_manager.refresh_hook_runtimes().await;

        let plugin_mcp_servers = load_configured_plugin_mcp_servers(
            result.installed_path.as_path(),
            auth.as_ref().map(CodexAuth::auth_mode),
            &result.plugin_id,
            &config.config_layer_stack,
            config.codex_home.as_path(),
        )
        .await;
        if !plugin_mcp_servers.is_empty() {
            let redirect_mode = plugin_redirect_mode(result.installed_path.as_path());
            self.start_plugin_mcp_oauth_logins(
                &config,
                &result.plugin_id,
                plugin_mcp_servers,
                redirect_mode,
            )
            .await;
        }

        let plugin_app_declarations =
            load_plugin_apps(result.installed_path.as_path(), &result.plugin_id).await;
        let apps_needing_auth = self
            .plugin_apps_needing_auth_for_install(
                &config,
                auth.as_ref(),
                &result.plugin_id.as_key(),
                &plugin_app_declarations,
            )
            .await;

        Ok(PluginInstallResponse {
            auth_policy: result.auth_policy.into(),
            apps_needing_auth,
        })
    }

    async fn plugin_apps_needing_auth_for_install(
        &self,
        config: &Config,
        auth: Option<&CodexAuth>,
        plugin_id: &str,
        plugin_app_declarations: &[codex_plugin::AppDeclaration],
    ) -> Vec<AppSummary> {
        if plugin_app_declarations.is_empty()
            || !config
                .features
                .apps_enabled_for_auth(auth.is_some_and(CodexAuth::is_chatgpt_auth))
        {
            return Vec::new();
        }

        let plugin_apps =
            codex_plugin::app_connector_ids_from_declarations(plugin_app_declarations);
        let app_category_by_id = plugin_app_declarations
            .iter()
            .filter_map(|app| {
                app.category
                    .as_ref()
                    .map(|category| (app.connector_id.0.clone(), category.clone()))
            })
            .collect();
        let environment_manager = self.thread_manager.environment_manager();
        let app_summaries = plugin_app_summaries(&plugin_apps, &app_category_by_id);
        let accessible_connectors_result =
            connectors::list_accessible_connectors_from_mcp_tools_with_mcp_manager(
                config,
                true,
                environment_manager,
                self.thread_manager.mcp_manager(),
            )
            .await;
        let (accessible_connectors, codex_apps_ready) = match accessible_connectors_result {
            Ok(status) => (status.connectors, status.codex_apps_ready),
            Err(err) => {
                warn!(
                    plugin = plugin_id,
                    "failed to load accessible apps after plugin install: {err:#}"
                );
                (
                    connectors::list_cached_accessible_connectors_from_mcp_tools(config, auth)
                        .unwrap_or_default(),
                    false,
                )
            }
        };
        if !codex_apps_ready {
            warn!(
                plugin = plugin_id,
                "codex_apps MCP not ready after plugin install; skipping appsNeedingAuth check"
            );
            return Vec::new();
        }

        let accessible_ids = accessible_connectors
            .iter()
            .map(|connector| connector.id.as_str())
            .collect::<HashSet<_>>();
        app_summaries
            .into_iter()
            .filter(|app| !accessible_ids.contains(app.id.as_str()))
            .collect()
    }

    async fn start_plugin_mcp_oauth_logins(
        &self,
        config: &Config,
        plugin_id: &PluginId,
        mut plugin_mcp_servers: HashMap<String, McpServerConfig>,
        redirect_mode: StreamableHttpRedirectMode,
    ) {
        let plugin_id = plugin_id.as_key();
        config.apply_plugin_mcp_server_requirements(&plugin_id, &mut plugin_mcp_servers);
        let runtime_context = McpRuntimeContext::new(
            self.thread_manager.environment_manager(),
            config.cwd.to_path_buf(),
        );
        for (name, server) in plugin_mcp_servers {
            // EMA uses the account's enterprise grant, never per-plugin OAuth fallback.
            if !server.enabled || matches!(server.auth, codex_config::types::McpServerAuth::EmaAuth)
            {
                continue;
            }
            if !server.is_local_environment() {
                warn!(
                    plugin = %plugin_id,
                    server = %name,
                    environment_id = %server.environment_id,
                    "skipping plugin MCP OAuth for an unowned environment"
                );
                continue;
            }
            let http_client = match runtime_context.resolve_http_client(&name, &server) {
                Ok(http_client) => http_client,
                Err(err) => {
                    warn!("failed to resolve MCP runtime for plugin install {name}: {err}");
                    continue;
                }
            };
            let login_support = oauth_login_support(
                &server.transport,
                Arc::clone(&http_client),
                OAuthDiscoveryTimeout::LOCAL,
                redirect_mode,
            )
            .await;
            let oauth_config = match login_support {
                McpOAuthLoginSupport::Supported(config) => config,
                McpOAuthLoginSupport::Unsupported => continue,
                McpOAuthLoginSupport::Unknown(err) => {
                    warn!(
                        "MCP server may or may not require login for plugin install {name}: {err}"
                    );
                    continue;
                }
            };

            let resolved_scopes = resolve_oauth_scopes(
                /*explicit_scopes*/ None,
                server.scopes.clone(),
                oauth_config.discovered_scopes.clone(),
            );

            let store_mode = config.mcp_oauth_credentials_store_mode;
            let keyring_backend_kind = config.auth_keyring_backend_kind();
            let callback_port = server.oauth_callback_port(config.mcp_oauth_callback_port);
            let callback_url = match resolve_oauth_callback(
                &server,
                &oauth_config.url,
                config.mcp_oauth_callback_url.as_deref(),
            ) {
                Ok(callback_url) => callback_url,
                Err(error) => {
                    warn!(
                        "failed to resolve MCP OAuth callback for plugin install {name}: {error}"
                    );
                    continue;
                }
            };
            let outgoing = Arc::clone(&self.outgoing);
            let notification_name = name.clone();
            let oauth_credential_name = server.oauth_credential_name(&name).into_owned();
            let thread_manager = Arc::clone(&self.thread_manager);
            let http_client = Arc::clone(&http_client);
            let global_callback_url = config.mcp_oauth_callback_url.clone();

            tokio::spawn(async move {
                let oauth_client_config = server.oauth.as_ref();
                let first_attempt = perform_oauth_login_silent(
                    &oauth_credential_name,
                    &oauth_config.url,
                    store_mode,
                    keyring_backend_kind,
                    oauth_config.http_headers.clone(),
                    oauth_config.env_http_headers.clone(),
                    &resolved_scopes.scopes,
                    oauth_client_config,
                    McpOAuthClientRegistration::Auto,
                    server.oauth_resource.as_deref(),
                    callback_port,
                    callback_url.as_deref(),
                    global_callback_url.as_deref(),
                    Arc::clone(&http_client),
                    redirect_mode,
                )
                .await;

                let final_result = match first_attempt {
                    Err(err) if should_retry_without_scopes(&resolved_scopes, &err) => {
                        perform_oauth_login_silent(
                            &oauth_credential_name,
                            &oauth_config.url,
                            store_mode,
                            keyring_backend_kind,
                            oauth_config.http_headers,
                            oauth_config.env_http_headers,
                            &[],
                            oauth_client_config,
                            McpOAuthClientRegistration::Auto,
                            server.oauth_resource.as_deref(),
                            callback_port,
                            callback_url.as_deref(),
                            global_callback_url.as_deref(),
                            http_client,
                            redirect_mode,
                        )
                        .await
                    }
                    result => result,
                };

                let (success, error) = match final_result {
                    Ok(()) => (true, None),
                    Err(err) => (false, Some(err.to_string())),
                };
                if success {
                    thread_manager
                        .invalidate_mcp_runtimes_for(McpRefreshReason::PluginOAuthLoginCompleted)
                        .await;
                }

                let notification = ServerNotification::McpServerOauthLoginCompleted(
                    McpServerOauthLoginCompletedNotification {
                        name: notification_name,
                        thread_id: None,
                        login_id: None,
                        success,
                        error,
                    },
                );
                outgoing.send_server_notification(notification).await;
            });
        }
    }

    async fn plugin_uninstall_response(
        &self,
        params: PluginUninstallParams,
    ) -> Result<PluginUninstallResponse, JSONRPCErrorError> {
        let PluginUninstallParams { plugin_id } = params;
        codex_plugin::PluginId::parse(&plugin_id)
            .map_err(|err| invalid_request(err.to_string()))?;
        let plugins_manager = self.thread_manager.plugins_manager();

        plugins_manager
            .uninstall_plugin(plugin_id)
            .await
            .map_err(Self::plugin_uninstall_error)?;
        match self.load_latest_config(/*fallback_cwd*/ None).await {
            Ok(_) => {
                self.on_effective_plugins_changed(McpRefreshReason::PluginUninstalled)
                    .await
            }
            Err(err) => {
                warn!(
                    "failed to reload config after plugin uninstall, clearing plugin-related caches only: {err:?}"
                );
                self.clear_plugin_related_caches();
            }
        }
        Ok(PluginUninstallResponse {})
    }

    fn plugin_install_error(err: CorePluginInstallError) -> JSONRPCErrorError {
        if err.is_invalid_request() {
            return invalid_request(err.to_string());
        }

        match err {
            CorePluginInstallError::Marketplace(err) => {
                Self::marketplace_error(err, "install plugin")
            }
            CorePluginInstallError::Config(err) => {
                internal_error(format!("failed to persist installed plugin config: {err}"))
            }

            CorePluginInstallError::Join(err) => {
                internal_error(format!("failed to install plugin: {err}"))
            }
            CorePluginInstallError::Store(err) => {
                internal_error(format!("failed to install plugin: {err}"))
            }
        }
    }

    fn plugin_uninstall_error(err: CorePluginUninstallError) -> JSONRPCErrorError {
        if err.is_invalid_request() {
            return invalid_request(err.to_string());
        }

        match err {
            CorePluginUninstallError::Config(err) => {
                internal_error(format!("failed to clear plugin config: {err}"))
            }

            CorePluginUninstallError::Join(err) => {
                internal_error(format!("failed to uninstall plugin: {err}"))
            }
            CorePluginUninstallError::Store(err) => {
                internal_error(format!("failed to uninstall plugin: {err}"))
            }
            CorePluginUninstallError::InvalidPluginId(_) => {
                unreachable!("invalid plugin ids are handled above");
            }
        }
    }

    fn marketplace_error(err: MarketplaceError, action: &str) -> JSONRPCErrorError {
        match err {
            MarketplaceError::MarketplaceNotFound { .. }
            | MarketplaceError::InvalidMarketplaceFile { .. }
            | MarketplaceError::PluginNotFound { .. }
            | MarketplaceError::PluginNotAvailable { .. }
            | MarketplaceError::PluginsDisabled
            | MarketplaceError::InvalidPlugin(_) => invalid_request(err.to_string()),
            MarketplaceError::Io { .. } => internal_error(format!("failed to {action}: {err}")),
        }
    }
}

fn plugin_app_summaries(
    plugin_apps: &[codex_plugin::AppConnectorId],
    app_category_by_id: &HashMap<String, String>,
) -> Vec<AppSummary> {
    let mut seen = HashSet::new();
    plugin_apps
        .iter()
        .filter(|app| seen.insert(&app.0))
        .map(|app| AppSummary {
            id: app.0.clone(),
            name: app.0.clone(),
            category: app_category_by_id.get(&app.0).cloned(),
            description: None,
            install_url: None,
        })
        .collect()
}
