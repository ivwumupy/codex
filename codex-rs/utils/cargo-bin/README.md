# Cargo test helpers

`cargo_bin` locates a test executable using Cargo's binary environment variables or the adjacent build output. `find_resource!` resolves fixture paths relative to the calling crate's manifest. `repo_root` returns the repository directory containing the Cargo workspace.
