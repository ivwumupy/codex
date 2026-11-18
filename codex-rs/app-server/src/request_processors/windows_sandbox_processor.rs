use super::*;

#[derive(Clone)]
pub(crate) struct WindowsSandboxRequestProcessor;

impl WindowsSandboxRequestProcessor {
    pub(crate) async fn windows_sandbox_readiness(
        &self,
        _request_id: &ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Ok(Some(
            WindowsSandboxReadinessResponse {
                status: WindowsSandboxReadiness::NotConfigured,
            }
            .into(),
        ))
    }

    pub(crate) async fn windows_sandbox_setup_start(
        &self,
        _request_id: &ConnectionRequestId,
        _params: WindowsSandboxSetupStartParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        Err(invalid_request(
            "Windows sandbox setup is unsupported in this YOLO-only build",
        ))
    }
}
