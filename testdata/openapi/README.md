# OpenAPI schema snapshot for the wire-shape contract

The wire-shape contract (`tests/wire_shape_contract.rs`, run by `cargo test` in the Test workflow) diffs the SDK's serde types against the platform's OpenAPI specs. It reads the schema declarations under `components.schemas` and the names of each one's `properties`.

The four files here hold exactly that, and nothing else. `scripts/snapshot_openapi_schemas.py`, vendored byte-identical from the other AxonFlow SDKs, derives them from the platform's `docs/api/*.yaml` at platform commit `ba5201a52edd2bfbe9f1dadd736f19cae03d9cc4`, the merged platform source for the v11.2.0 train. It works on the YAML node graph, so every declaration survives in source order, including a schema declared twice in one file and a declaration with no properties. It drops descriptions, types, paths and the `info` block; the full specs carry the platform's own licence statement there, and this repository is MIT. `python scripts/snapshot_openapi_schemas.py --self-test` checks all of this on a planted spec.

| File | sha256 of the source spec |
|---|---|
| `agent-api.yaml` | `617e6236f91dd7716baaab8e2f22ef24f1713ebfd4938cc52a60a3368e3acb1a` |
| `masfeat-api.yaml` | `eb224773c2a14fc6a84a630b652b17e010a4776da28529b13e173b002d2d4756` |
| `orchestrator-api.yaml` | `a1f96d929ff35acb1204ce5ba3c6e605b8f91d93e7fa62a2fa8a760ea8486860` |
| `policy-api.yaml` | `a1a7aff59d759033f99721a40d942e8750c6026d69f9efb0edf66016f0069ae9` |

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
