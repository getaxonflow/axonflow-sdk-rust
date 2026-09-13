# OpenAPI schema snapshot for the wire-shape contract

The wire-shape contract (`tests/wire_shape_contract.rs`, run by `cargo test` in the Test workflow) diffs the SDK's serde types against the platform's OpenAPI specs. It reads the schema declarations under `components.schemas` and the names of each one's `properties`.

The four files here hold exactly that, and nothing else. `scripts/snapshot_openapi_schemas.py`, vendored byte-identical from the other AxonFlow SDKs, derives them from the platform's `docs/api/*.yaml` at platform commit `36e0e96b7e5c16626d394272b727b533f2b94a04`, the v11.0.0 candidate. It works on the YAML node graph, so every declaration survives in source order, including a schema declared twice in one file and a declaration with no properties. It drops descriptions, types, paths and the `info` block; the full specs carry the platform's own licence statement there, and this repository is MIT. `python scripts/snapshot_openapi_schemas.py --self-test` checks all of this on a planted spec.

| File | sha256 of the source spec |
|---|---|
| `agent-api.yaml` | `49bd1b145cd3b2d29e8cc8de240cc184d4f5d24f83b6c546894d1f0a682a032a` |
| `masfeat-api.yaml` | `2d49d6af2d5b1510b373c01fce1df2b712b32766b14d477652797746c52147e7` |
| `orchestrator-api.yaml` | `b173c9bec456e4af3aa09c63306e503caa73ca0cf61da3dfaa7b21453dcd1188` |
| `policy-api.yaml` | `090730a359bf242b6ed5c1a0d63a2831c749583bd957cc3ecc64c99ead5514b7` |

Each file's header repeats its source path, commit and digest. The files are byte-identical to the Python and TypeScript SDKs' snapshots at the same commit.

## Why a snapshot, not the community mirror

The community mirror publishes `docs/api/` byte for byte, but it receives v11 only at the v11.0.0 tag. The SDK's v11 wire work is checked against the same schemas, derived here ahead of the tag.

## Changing these files

A change to any file in this directory is a change to the contract's pin. The `Wire-Shape Contract` workflow treats it the same as a change to `openapi_specs_sha` in `testdata/wire_shape_baseline.json`: the PR needs the `spec-pin-bump` label, and it should not also change SDK types. Do not edit the files by hand.

To refresh the snapshot from a platform commit, then the baseline from the snapshot:

```
python scripts/snapshot_openapi_schemas.py <path to docs/api> testdata/openapi --source-commit <full 40-character commit>
WIRE_SHAPE_REFRESH=1 cargo test --test wire_shape_contract -- --ignored refresh_baseline
```

Then update the table above.

`python scripts/snapshot_openapi_schemas.py --check-snapshot testdata/openapi` checks that every file carries the generated header, that all of them name one full platform commit, and that each is exactly the script's derived form, which refuses prose, types or a spec copied in verbatim. It cannot see a declaration added by hand in the derived form itself; the `spec-pin-bump` label that any change here requires is what makes that visible.

The baseline refresh takes the platform commit from these files' headers, and the contract fails when that commit differs from the baseline's `openapi_specs_sha`.
