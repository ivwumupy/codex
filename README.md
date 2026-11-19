<p align="center"><strong>Codex CLI</strong> is a coding agent from OpenAI that runs locally on your computer.
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want Codex in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/codex/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>codex app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Codex App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Codex Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

## Quickstart

### Build this fork

Run the following commands from the repository root on Linux or macOS.

```shell
cargo build --locked --bin codex --bin codex-code-mode-host --bin codex-exec-server
./target/debug/codex
```

The CLI runs commands with the current user's permissions and never prompts for execution approval. Explicit restricted profiles are rejected.

### Authentication

Run `codex` to configure an API key or a custom provider. To save an OpenAI API key from the shell:

```shell
printenv OPENAI_API_KEY | codex login --with-api-key
```

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Rust protocol overview**](./docs/protocol_v1.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
