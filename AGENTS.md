<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Working in this repository

Shrug installs and runs Atlas on DGX Sparks. Read [docs/DESIGN.md](docs/DESIGN.md)
before changing behavior; it is the product contract.

- Keep it small. One binary, one catalog, one gateway. Removing code beats
  adding it. If a feature is in LLooM and not in the design, it stays in LLooM.
- Engine work belongs in `Enntity/atlas`; engine releases belong in
  `Enntity/sparkglm` and `Enntity/sparkqwen`. Shrug runs their published
  images by digest and never builds or patches Atlas.
- A catalog entry needs immutable pins (model revision, image digest, Atlas
  commit), the licenses of every downloaded weight, and links to the receipts
  that qualified it on that cluster size. No entry ships on vendor numbers.
  Catalogs are signed only by release CI; signing keys never enter the
  repository, a fixture or a log.
- Planning is read-only. Nothing downloads, writes config, or starts a
  process without `--go`. Keep `--json` output stable; it is the agent API.
- Never stop or replace a process Shrug did not start, and never take a
  Spark that is serving someone else's workload.
- This repository is public. Keep host names, user names, private addresses,
  keys and home paths out of code, docs, tests and fixtures.
- Every source file starts with `SPDX-License-Identifier: AGPL-3.0-only`.
- Before sending a change, run the checks in `.github/workflows/ci.yml`:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/check-spdx.sh
```
