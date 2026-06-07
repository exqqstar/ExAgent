# Third-Party Notices

ExAgent is licensed under `MIT OR Apache-2.0`. Some documentation in this
repository summarizes or references third-party open-source projects.

The root [NOTICE](NOTICE) file contains attribution notices for ExAgent itself.

## External Reference Material

Private design notes and local-only reference checkouts are not distributed in
the public repository. Do not add vendored third-party source, reference packs,
or copied documentation unless the upstream license allows it and the required
license and attribution notices are included.

## Dependency Licenses

Rust and npm dependencies keep their own licenses. Use `cargo deny check
licenses` for the Rust dependency policy configured in `deny.toml`. For npm
dependencies, inspect `apps/desktop/package-lock.json` and package metadata.
