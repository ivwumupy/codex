//! Bind filesystem and network inputs for sandbox preparation on the execution host.

use crate::SandboxExecRequest;
use crate::SandboxType;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::io;
use std::process::Command;

/// A command prepared for its selected sandbox. Native Windows token sessions
/// must use the Windows session launcher.
#[must_use]
pub struct WrappedSandboxCommand {
    pub(super) request: SandboxExecRequest,
    // UTF-8 argv cannot represent every native executable path.
    pub(super) native_program: Option<OsString>,
}

impl WrappedSandboxCommand {
    /// Return the request with UTF-8 argv for callers that spawn the command themselves.
    pub fn into_command_request(self) -> SandboxExecRequest {
        self.request
    }

    /// Return a configured process command when argv carries all required sandbox enforcement.
    pub fn into_direct_command(self) -> io::Result<Command> {
        let request = self.request;
        if request.sandbox == SandboxType::WindowsRestrictedToken {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "a native Windows sandbox requires its session launcher",
            ));
        }
        let (program, args) = request.command.split_first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "sandbox command is empty")
        })?;
        let mut command = Command::new(
            self.native_program
                .as_deref()
                .unwrap_or_else(|| OsStr::new(program)),
        );
        #[cfg(unix)]
        if let Some(arg0) = request.arg0 {
            use std::os::unix::process::CommandExt;
            command.arg0(arg0);
        }
        command
            .args(args)
            .current_dir(request.cwd.to_abs_path()?)
            .env_clear()
            .envs(request.env);
        Ok(command)
    }
}
