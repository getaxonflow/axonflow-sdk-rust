# AxonFlow SDK for Rust

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Status: placeholder](https://img.shields.io/badge/Status-placeholder-orange.svg)](#status)

> Official Rust SDK for AxonFlow — runtime control, MCP policy enforcement, approvals, and audit trails for production AI.

## Status

This repository is a **placeholder** — the Rust SDK has not been written yet. The companion SDKs in [TypeScript](https://github.com/getaxonflow/axonflow-sdk-typescript), [Python](https://github.com/getaxonflow/axonflow-sdk-python), [Go](https://github.com/getaxonflow/axonflow-sdk-go), and [Java](https://github.com/getaxonflow/axonflow-sdk-java) are the reference implementations for the API contract a Rust port should mirror.

## How AxonFlow Fits

AxonFlow is an AI control plane that enforces policy, gates approvals, and records audit trails for AI agents and MCP-based tools at runtime. SDKs are thin clients that talk to a deployed AxonFlow control plane (self-hosted or cloud) — the platform and SDKs are designed to be used together. SDKs alone are not sufficient for end-to-end governance.

For the full picture, see the [main project](https://github.com/getaxonflow/axonflow), the [docs site](https://docs.getaxonflow.com), or one of the existing SDK READMEs above.

## Contributing

We'd love a Rust SDK port. The most direct path is a community contribution.

If you'd like to take a first pass:

1. Read [CONTRIBUTING.md](./CONTRIBUTING.md) for the API contract reference, expected scope, and how to discuss the design before sinking in serious time.
2. Open an issue or [GitHub Discussion](https://github.com/getaxonflow/axonflow/discussions) so we can scope and avoid duplicate work.
3. Fork this repo, build, and open a PR.

If you've already prototyped a Rust SDK in a private repo and want to land it here, see CONTRIBUTING.md "Importing an existing port" for the simplest path.

## Security

See [SECURITY.md](./SECURITY.md) for responsible disclosure.

## License

[MIT](./LICENSE).
