# Configuration

The [configuration schema](crates/core/config.schema.json) lists the settings available in this fork's `config.toml`. Run `codex features list` to see feature defaults and `codex --help` for command-line options.

Features with their own settings use an `enabled` field in the feature table. For example:

```toml
[features.code_mode]
enabled = true
```

For a boolean-only feature, set the key directly under `[features]`, for example `shell_snapshot = false`. CLI feature switches use the same paths when saving changes.
