#![cfg(test)]

use assert_matches::assert_matches;
use pretty_assertions::assert_eq;

use super::*;
use crate::ExecServerCli;

fn exec_server_from_args(args: &[&str]) -> ExecServerCommand {
    let cli = ExecServerCli::try_parse_from(
        ["codex-exec-server"]
            .into_iter()
            .chain(args.iter().copied()),
    )
    .expect("parse executor arguments");
    cli.command
}

#[test]
fn exec_server_defaults_preserve_local_and_noise_modes() {
    for args in [
        vec![],
        vec!["--listen", "stdio"],
        vec![
            "--remote",
            "https://registry.example.com",
            "--environment-id",
            "env-1",
        ],
        vec![
            "--remote",
            "https://registry.example.com",
            "--environment-id",
            "env-1",
            "--remote-transport",
            "noise",
            "--use-agent-identity-auth",
        ],
    ] {
        let command = exec_server_from_args(&args);
        assert_eq!(command.remote_transport, ExecServerRemoteTransport::Noise);
        assert!(!command.executor_credential_stdin);
        command
            .validate_remote_transport()
            .expect("valid existing mode");
    }
}

#[test]
fn exec_server_parses_executor_credential_options() {
    let args = [
        "--remote",
        "https://registry.example.com",
        "--environment-id",
        "env-1",
    ];
    for options in [
        vec!["--executor-credential-stdin"],
        vec!["--executor-credential-stdin", "--exit-on-stdin-close"],
    ] {
        let options: Vec<_> = args.iter().copied().chain(options).collect();
        let command = exec_server_from_args(&options);
        assert!(command.executor_credential_stdin);
        command
            .validate_remote_transport()
            .expect("valid scoped mode");
    }
}

#[test]
fn exec_server_forward_parses_shared_remote_options() {
    let cli = ExecServerCli::try_parse_from([
        "codex-exec-server",
        "forward",
        "--connect",
        "ws://127.0.0.1:8765",
        "--remote",
        "https://example.openai.com",
        "--environment-id",
        "env-1",
        "--name",
        "forwarded",
        "--strict-config",
        "--use-agent-identity-auth",
    ])
    .expect("parse forward");
    assert_matches!(cli.command, ExecServerCommand {
        command: Some(ExecServerSubcommand::Forward { connect }),
        remote: Some(remote),
        environment_id: Some(environment_id),
        name: Some(name),
        strict_config: true,
        use_agent_identity_auth: true,
        ..
    } if connect == "ws://127.0.0.1:8765" && remote == "https://example.openai.com" && environment_id == "env-1" && name == "forwarded");
}

#[test]
fn standalone_help_documents_executor_options() {
    use clap::CommandFactory;
    let help = ExecServerCli::command()
        .term_width(80)
        .mut_arg("exit_on_stdin_close", |arg| arg.hide_env_values(true))
        .mut_arg("proxy_private_ips_via_upstream", |arg| {
            arg.hide_env_values(true)
        })
        .render_long_help()
        .to_string()
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(help);
}
