use super::*;
use crate::auth_mode::auth_mode_to_api;
use crate::model_catalog::ModelCatalog;
use crate::outgoing_message::AccountNotification;
use codex_core::McpRefreshReason;
use codex_mcp::ema_auth_scope;
use codex_rmcp_client::EnterpriseOAuthCredentialGuard;

mod enterprise_login;

pub(super) use enterprise_login::EnterpriseLoginCompletion;
pub(super) use enterprise_login::EnterpriseLoginTarget;

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
    model_catalog: Arc<ModelCatalog>,
    pub(super) enterprise_login: Arc<enterprise_login::EnterpriseLoginState>,
}

impl AccountRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
        model_catalog: Arc<ModelCatalog>,
    ) -> Arc<Self> {
        let enterprise_login = Arc::new(enterprise_login::EnterpriseLoginState::new(
            Arc::clone(&auth_manager),
            Arc::clone(&thread_manager),
            config_manager.clone(),
        ));
        Arc::new(Self {
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
            model_catalog,
            enterprise_login,
        })
    }

    pub(crate) async fn login_account(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.login_v2(request_id, params).await.map(|()| None)
    }

    pub(crate) async fn logout_account(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.logout_v2(request_id).await.map(|()| None)
    }

    pub(crate) async fn cancel_login_account(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.cancel_login_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account(
        &self,
        _params: GetAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let config = self.load_latest_config().await;
        let provider =
            create_model_provider(config.model_provider, Some(self.auth_manager.clone()));
        let state = provider
            .account_state()
            .map_err(|err| invalid_request(err.to_string()))?;
        Ok(Some(
            GetAccountResponse {
                account: state.account.map(Account::from),
                requires_openai_auth: state.requires_openai_auth,
                user_id: None,
                workspace_routing: None,
            }
            .into(),
        ))
    }

    pub(crate) async fn get_auth_status(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_auth_status_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn cancel_active_login(&self) {
        self.enterprise_login.cancel(/*login_id*/ None).await;
    }

    pub(crate) fn clear_external_auth(&self) {
        self.auth_manager.clear_external_auth();
    }

    fn current_account_updated_notification(&self) -> AccountUpdatedNotification {
        let auth = self.auth_manager.auth_cached();
        AccountUpdatedNotification {
            auth_mode: auth
                .as_ref()
                .map(CodexAuth::api_auth_mode)
                .map(auth_mode_to_api),
            plan_type: auth.as_ref().and_then(CodexAuth::account_plan_type),
        }
    }

    async fn load_latest_config(&self) -> Config {
        match self
            .config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
        {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!("failed to reload config, using startup config: {err}");
                self.config.as_ref().clone()
            }
        }
    }

    async fn maybe_refresh_plugin_caches_for_current_config(
        config_manager: &ConfigManager,
        thread_manager: &Arc<ThreadManager>,
        auth: Option<CodexAuth>,
    ) {
        thread_manager
            .plugins_manager()
            .clear_recommended_plugins_cache();

        match config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
        {
            Ok(config) => {
                let runtime_auth = thread_manager.runtime_auth_for_config(&config);
                Self::spawn_effective_plugins_changed_task(
                    Arc::clone(thread_manager),
                    config_manager.clone(),
                    McpRefreshReason::AccountPluginCacheChanged,
                );
                let plugins_config = config.plugins_config_input();
                let refresh_thread_manager = Arc::clone(thread_manager);
                let refresh_config_manager = config_manager.clone();
                let on_effective_plugins_changed: Arc<
                    dyn Fn(codex_core_plugins::EffectivePluginsChange) + Send + Sync,
                > = Arc::new(move |change| {
                    Self::spawn_effective_plugins_changed_task(
                        Arc::clone(&refresh_thread_manager),
                        refresh_config_manager.clone(),
                        change.metadata_change_id.map_or(
                            McpRefreshReason::AccountPluginCacheChanged,
                            McpRefreshReason::InstalledPluginMetadata,
                        ),
                    );
                });
                if let Err(err) = runtime_auth {
                    warn!("failed to initialize runtime auth for remote plugin refresh: {err}");
                    return;
                }
                thread_manager
                    .plugins_manager()
                    .maybe_start_remote_plugin_caches_refresh(
                        &plugins_config,
                        auth,
                        Some(on_effective_plugins_changed),
                    );
            }
            Err(err) => {
                warn!(
                    "failed to reload config after account changed, skipping remote installed plugins cache refresh: {err}"
                );
            }
        }
    }

    fn spawn_effective_plugins_changed_task(
        thread_manager: Arc<ThreadManager>,
        config_manager: ConfigManager,
        reason: McpRefreshReason,
    ) {
        tokio::spawn(async move {
            thread_manager.plugins_manager().clear_cache();
            thread_manager.skills_service().clear_cache();
            crate::mcp_refresh::reload_mcp_config_best_effort(&thread_manager, &config_manager)
                .await;
            thread_manager.invalidate_mcp_runtimes_for(reason).await;
        });
    }

    async fn login_v2(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<(), JSONRPCErrorError> {
        if self.auth_manager.is_workload_identity_selected() {
            return Err(self.configured_auth_owned_by_host_error());
        }
        self.enterprise_login.cancel(/*login_id*/ None).await;
        match params {
            LoginAccountParams::ApiKey { api_key } => {
                self.login_api_key_v2(request_id, LoginApiKeyParams { api_key })
                    .await;
            }
            LoginAccountParams::Chatgpt { .. }
            | LoginAccountParams::ChatgptDeviceCode
            | LoginAccountParams::ChatgptAuthTokens { .. } => {
                return Err(invalid_request("ChatGPT account login is unsupported."));
            }
            LoginAccountParams::AmazonBedrock { .. }
            | LoginAccountParams::AmazonBedrockAccessKeys { .. } => {
                return Err(invalid_request("Amazon Bedrock login is unsupported."));
            }
        }
        Ok(())
    }

    fn external_auth_active_error(&self) -> JSONRPCErrorError {
        invalid_request("External auth is active. Use account/logout to clear it.")
    }

    fn configured_auth_owned_by_host_error(&self) -> JSONRPCErrorError {
        invalid_request(
            "Configured external authentication is owned by the app-server host and cannot be changed through account RPCs.",
        )
    }

    async fn login_api_key_common(
        &self,
        params: &LoginApiKeyParams,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        if self.auth_manager.is_external_chatgpt_auth_active() {
            return Err(self.external_auth_active_error());
        }

        if !self
            .auth_manager
            .is_login_method_allowed(ForcedLoginMethod::Api)
        {
            return Err(invalid_request(
                "API key login is disabled by this workspace.",
            ));
        }

        match login_with_api_key(
            &self.config.codex_home,
            &params.api_key,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        ) {
            Ok(()) => {
                self.auth_manager.reload().await;
                self.config_manager.clear_chatgpt_config_loader();
                Ok(())
            }
            Err(err) => Err(internal_error(format!("failed to save api key: {err}"))),
        }
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, params: LoginApiKeyParams) {
        let result = self
            .login_api_key_common(&params)
            .await
            .map(|()| LoginAccountResponse::ApiKey {});
        let logged_in = result.is_ok();
        if logged_in {
            self.model_catalog.invalidate().await;
        }
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications(/*login_id*/ None)
                .await;
        }
    }

    async fn cancel_login_response(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<CancelLoginAccountResponse, JSONRPCErrorError> {
        let status = if self.enterprise_login.cancel(Some(&params.login_id)).await {
            CancelLoginAccountStatus::Canceled
        } else {
            CancelLoginAccountStatus::NotFound
        };
        Ok(CancelLoginAccountResponse { status })
    }

    async fn send_login_success_notifications(&self, login_id: Option<Uuid>) {
        self.thread_manager
            .invalidate_mcp_runtimes_for(McpRefreshReason::AccountLoginCompleted)
            .await;
        self.send_account_login_notifications(AccountLoginCompletedNotification {
            login_id: login_id.map(|id| id.to_string()),
            success: true,
            error: None,
            onboarding_entrypoint: None,
        })
        .await;
    }

    async fn send_account_login_notifications(&self, payload: AccountLoginCompletedNotification) {
        let auth_changes = self.auth_manager.auth_change_state_receiver();
        let owner_generation = auth_changes.borrow().owner_generation;
        if payload.success && auth_changes.borrow().owner_generation == owner_generation {
            Self::maybe_refresh_plugin_caches_for_current_config(
                &self.config_manager,
                &self.thread_manager,
                self.auth_manager.auth_cached(),
            )
            .await;
        }

        let success = payload.success;
        self.outgoing
            .send_account_notification(
                /*connection_id*/ None,
                &auth_changes,
                owner_generation,
                AccountNotification::LoginCompleted(payload),
            )
            .await;

        if success {
            let notification = self.current_account_updated_notification();
            self.outgoing
                .send_account_notification(
                    /*connection_id*/ None,
                    &auth_changes,
                    owner_generation,
                    AccountNotification::Updated(notification),
                )
                .await;
        }
    }

    async fn logout_common(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        if self.auth_manager.is_workload_identity_selected() {
            return Err(self.configured_auth_owned_by_host_error());
        }
        self.cancel_active_login().await;
        let config =
            logout_configured_account(&self.config_manager, &self.auth_manager, &self.config)
                .await?;
        self.thread_manager
            .invalidate_mcp_runtimes_for(McpRefreshReason::AccountLoggedOut)
            .await;
        clear_account_configuration(&self.config_manager, &config).await?;

        self.model_catalog.invalidate().await;

        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        // Reflect the current auth method after logout (likely None).
        Ok(self
            .auth_manager
            .auth_cached()
            .as_ref()
            .map(CodexAuth::api_auth_mode)
            .map(auth_mode_to_api))
    }

    async fn logout_v2(&self, request_id: ConnectionRequestId) -> Result<(), JSONRPCErrorError> {
        let result = self.logout_common().await;
        let account_updated =
            result
                .as_ref()
                .ok()
                .cloned()
                .map(|auth_mode| AccountUpdatedNotification {
                    auth_mode,
                    plan_type: None,
                });
        self.outgoing
            .send_result(request_id, result.map(|_| LogoutAccountResponse {}))
            .await;

        if let Some(payload) = account_updated {
            self.outgoing
                .send_server_notification(ServerNotification::AccountUpdated(payload))
                .await;
        }
        Ok(())
    }

    async fn get_auth_status_response(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        let include_token = params.include_token.unwrap_or(false);
        let do_refresh = params.refresh_token.unwrap_or(false);
        if do_refresh && let Err(err) = self.auth_manager.refresh_token().await {
            tracing::warn!("failed to refresh provider credentials: {err}");
        }

        // Determine whether auth is required based on the active model provider.
        // If a custom provider is configured with `requires_openai_auth == false`,
        // then no auth step is required; otherwise, default to requiring auth.
        let config = self.load_latest_config().await;
        let requires_openai_auth = config.model_provider.requires_openai_auth;

        let response = if !requires_openai_auth {
            GetAuthStatusResponse {
                auth_method: None,
                auth_token: None,
                requires_openai_auth: Some(false),
            }
        } else {
            let auth = if do_refresh {
                self.auth_manager.auth_cached()
            } else {
                self.auth_manager.auth().await
            };
            match auth {
                Some(auth) => {
                    let permanent_refresh_failure =
                        self.auth_manager.refresh_failure_for_auth(&auth).is_some();
                    let auth_mode = auth_mode_to_api(auth.api_auth_mode());
                    let (reported_auth_method, token_opt) =
                        if self.auth_manager.is_workload_identity_selected()
                            || matches!(
                                auth,
                                CodexAuth::Headers(_)
                                    | CodexAuth::AgentIdentity(_)
                                    | CodexAuth::PersonalAccessToken(_)
                            )
                            || include_token && permanent_refresh_failure
                        {
                            // Host-owned and metadata-bearing credentials are never exported.
                            (Some(auth_mode), None)
                        } else {
                            match auth.get_token() {
                                Ok(token) if !token.is_empty() => {
                                    let tok = if include_token { Some(token) } else { None };
                                    (Some(auth_mode), tok)
                                }
                                Ok(_) => (None, None),
                                Err(err) => {
                                    tracing::warn!("failed to get token for auth status: {err}");
                                    (None, None)
                                }
                            }
                        };
                    GetAuthStatusResponse {
                        auth_method: reported_auth_method,
                        auth_token: token_opt,
                        requires_openai_auth: Some(true),
                    }
                }
                None => GetAuthStatusResponse {
                    auth_method: None,
                    auth_token: None,
                    requires_openai_auth: Some(true),
                },
            }
        };

        Ok(response)
    }
}

pub(crate) async fn logout_configured_account(
    config_manager: &ConfigManager,
    auth_manager: &Arc<AuthManager>,
    fallback_config: &Config,
) -> Result<Config, JSONRPCErrorError> {
    // Another process may have changed the persisted workspace. Reload both
    // account authority and its policy before selecting a grant to remove.
    auth_manager
        .reload_with_cached_auth()
        .await
        .map_err(|err| internal_error(format!("logout failed: {err}")))?;
    config_manager.revoke_mcp_ema();
    let config = config_manager
        .load_latest_config(/*fallback_cwd*/ None)
        .await;
    let scope = ema_auth_scope(auth_manager.auth_cached().as_ref());
    let enterprise_policy_failed = scope.is_some() && config.is_err();
    // Startup policy may belong to another workspace. Never use that fallback
    // to select a credential; a policy failure must not block primary logout.
    let enterprise_grant = config.as_ref().ok().and_then(|config| {
        scope
            .as_ref()
            .zip(config.mcp_enterprise_managed_auth.as_ref())
            .map(|(scope, profile)| {
                (
                    profile.idp.credential_name(scope),
                    profile.idp.issuer.clone(),
                    config.auth_keyring_backend_kind(),
                )
            })
    });
    let config = config.unwrap_or_else(|_| fallback_config.clone());

    // Retain the credential lock through primary logout. Otherwise a second
    // process can commit after deletion but before the account is removed.
    let enterprise_guard =
        if let Some((credential_name, issuer, keyring_backend)) = enterprise_grant {
            EnterpriseOAuthCredentialGuard::acquire(&credential_name, &issuer, keyring_backend)
                .await
                .map(Some)
        } else {
            Ok(None)
        };
    let cleanup_failed = match &enterprise_guard {
        Ok(Some(guard)) => guard.delete_tokens().is_err(),
        Ok(None) => false,
        Err(_) => true,
    };
    if enterprise_policy_failed || cleanup_failed {
        tracing::warn!("Failed to remove enterprise authorization; continuing account logout");
    }

    match auth_manager.logout_with_revoke().await {
        Ok(_) => {}
        Err(err) => {
            return Err(internal_error(format!("logout failed: {err}")));
        }
    }
    drop(enterprise_guard);
    Ok(config)
}

/// Discard account-scoped cloud and provider settings after credentials have been removed.
pub(crate) async fn clear_account_configuration(
    config_manager: &ConfigManager,
    _config: &Config,
) -> Result<(), JSONRPCErrorError> {
    config_manager.clear_chatgpt_config_loader();
    Ok(())
}
