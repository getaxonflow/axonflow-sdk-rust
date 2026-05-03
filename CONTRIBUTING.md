# Contributing to AxonFlow SDK for Rust

Thanks for considering a contribution. The Rust SDK is currently a placeholder — most contributions right now are about *bootstrapping* the SDK rather than evolving an existing one. This document is shorter than the equivalent file in the established SDKs because there is far less to follow yet.

## Sign your commits — Developer Certificate of Origin (DCO) is required

All contributions to this repository must be **signed off** under the [Developer Certificate of Origin v1.1](https://developercertificate.org/). The DCO is a per-commit affirmation that you wrote the code (or otherwise have the right to submit it) and are licensing it under the same license as the rest of this repository.

Add the sign-off automatically with `-s` (or `--signoff`) on every commit:

```bash
git commit -s -m "your commit message"
```

This appends a trailer like:

```
Signed-off-by: Your Name <your.email@example.com>
```

The name and email must match `git config user.name` / `git config user.email`.

If you forgot `-s` on an existing commit, fix it with one of:

```bash
# most recent commit
git commit --amend --signoff --no-edit

# every commit on the current branch
git rebase --signoff origin/main
```

A DCO check runs automatically on every PR opened in the `getaxonflow` org. **PRs with any unsigned commit will be blocked from merging until the missing sign-offs are added.** No exceptions, including for maintainers.

## API contract reference

Mirror the API surface of the existing SDKs:

- [TypeScript](https://github.com/getaxonflow/axonflow-sdk-typescript) — most fully-featured; treat as canonical
- [Python](https://github.com/getaxonflow/axonflow-sdk-python)
- [Go](https://github.com/getaxonflow/axonflow-sdk-go)
- [Java](https://github.com/getaxonflow/axonflow-sdk-java)

The `tests/` directory in any of those repos shows the expected client surface (registration, governance evaluation, MAP plans, approvals, retry context, audit). Ports should match function names, parameter shapes, and wire format.

## Discuss before you write

Open an issue or [GitHub Discussion](https://github.com/getaxonflow/axonflow/discussions) before writing a full port. Helps with:

- Avoiding duplicate effort
- Aligning on Rust idioms (sync vs `async`, error types, feature flags)
- Discussing minimum supported Rust version (`MSRV`)
- Scope of the first cut (governance + audit is a good v0.1.0 floor; MAP / approvals can follow)

## Getting started (once there's code to build against)

```bash
git clone https://github.com/getaxonflow/axonflow-sdk-rust.git
cd axonflow-sdk-rust
cargo build
cargo test
```

## Importing an existing port

If you've already built a Rust SDK in a private repository and want it landed here, the lowest-friction paths are:

1. **PR from a fork.** Fork this repo, copy your code into the fork, push, open a PR. Keeps your commit history attributed to you.
2. **Repo transfer.** Transfer your private repo to the `getaxonflow` org (one-click on GitHub). We rename this placeholder out of the way and rename yours into its place. Preserves your full history.
3. **Bundle import.** Send us a tarball or pointer; we cherry-pick. Last resort — easiest to lose attribution.

In all three cases we'll review against the API contract from the other SDKs and ask about license compatibility (the Rust SDK ships under MIT — your code needs to be MIT, Apache-2.0, BSD, or similar) and long-term maintenance plans.

## Pull request expectations

- Tests for any new client surface
- `cargo fmt` clean
- `cargo clippy -- -D warnings` clean
- `cargo doc` builds without warnings
- Public types documented inline
- No new dependencies pulled in without discussion

## Commit messages

We loosely follow [Conventional Commits](https://www.conventionalcommits.org/) — same pattern as the other SDKs. Examples:

- `feat: add MAP plan client`
- `fix(audit): handle 429 retry-after`
- `docs: expand quickstart`

Squash-merging means individual commit shapes inside the PR matter less than a clean PR title.

## Reporting bugs / asking questions

- Bugs: [open an issue](https://github.com/getaxonflow/axonflow-sdk-rust/issues/new)
- General questions: [GitHub Discussions](https://github.com/getaxonflow/axonflow/discussions)
- Private feedback: [hello@getaxonflow.com](mailto:hello@getaxonflow.com)

## Code of Conduct

Participation is governed by [CODE_OF_CONDUCT.md](./CODE_OF_CONDUCT.md).
