use super::*;
use pretty_assertions::assert_eq;

#[test]
fn native_launch_preserves_the_command_and_rejects_sandbox_requests() {
    let cwd_uri = if cfg!(windows) {
        PathUri::parse("file:///workspace/remote").expect("POSIX path URI")
    } else {
        PathUri::parse("file:///C:/workspace/remote").expect("Windows path URI")
    };
    let transform = |permissions: &PermissionProfile, selection| {
        SandboxManager::new().transform(SandboxTransformRequest {
            command: SandboxCommand {
                program: "true".into(),
                args: Vec::new(),
                cwd: cwd_uri.clone(),
                env: HashMap::new(),
                managed_network: None,
                additional_permissions: None,
            },
            permissions,
            workspace_roots: None,
            runtime_readable_paths: &[],
            selection,
            enforce_managed_network: false,
            environment_id: Some("remote"),
            network: None,
            sandbox_policy_cwd: &cwd_uri,
            sandbox_exe: None,
            use_legacy_landlock: false,
            windows_sandbox_level: WindowsSandboxLevel::Disabled,
        })
    };
    let native = SandboxSelection::Explicit(SandboxType::None);
    let request = transform(&PermissionProfile::Disabled, native)
        .expect("native launch")
        .into_command_request();
    assert_eq!(request.command, ["true"]);
    assert_eq!(request.cwd, cwd_uri);
    assert_eq!(request.sandbox, SandboxType::None);
    assert!(matches!(
        transform(&PermissionProfile::read_only(), native),
        Err(SandboxTransformError::SandboxUnavailable)
    ));
    assert!(matches!(
        transform(
            &PermissionProfile::Disabled,
            SandboxSelection::RequirePlatform
        ),
        Err(SandboxTransformError::SandboxUnavailable)
    ));
}
