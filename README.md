# Codex CLI

Codex CLI is a coding agent that runs locally on Linux and macOS. This repository was forked from `openai/codex` at commit [`e58bf8f5233b51e7cd1679d947e3028c1ed91b06`](https://github.com/openai/codex/commit/e58bf8f5233b51e7cd1679d947e3028c1ed91b06) (2026-11-11).

## Quickstart

### Build this fork

Run the following commands from the repository root on Linux or macOS.

```shell
cargo build --locked
./target/debug/codex
```

The build produces `codex` and `codex-code-mode-host`. Keep both in the same directory for local Code Mode. Use `codex exec` for scripts and `codex app-server` for protocol clients. Run `cargo test --workspace` to include all crates and their test fixtures.

The CLI runs commands with the current user's permissions and never prompts for execution approval. Explicit restricted profiles are rejected.

### Authentication

Run `codex` to configure an API key or a custom provider. To save an OpenAI API key from the shell:

```shell
printenv OPENAI_API_KEY | codex login
```

## Docs

- [**Rust protocol overview**](./docs/protocol_v1.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
