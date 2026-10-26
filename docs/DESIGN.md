<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Shrug design

Shrug installs and runs Atlas on DGX Sparks. It does a few things, and each
one has to work every time: find the hardware, choose the model, install it,
start it, prove it works, connect the agent, and report how it is running.

## Product rules

1. **Install and use.** One installer, one binary, one command to get from a
   bare Spark to a working endpoint. Anything that needs a manual step is a
   bug in Shrug or in its documentation.
2. **The best model for each cluster size, and few others.** Each cluster size
   gets one default model for agent work. A second model is added only when
   it wins a job the default loses (much lower latency, for example), and the
   measurement shows it. A new model replaces the old one when it wins on the
   same hardware with the same benchmarks; we don't keep both.
3. **Speed counts only with quality.** A release is listed only after it
   passes the quality and tool-calling checks of its source repository, on the
   cluster size it is listed for. Throughput comes after that.
4. **Agents are first-class users.** `--json` on every command, stable exit
   codes, plans that change nothing until `--go`, and errors that say what
   to run next.
5. **Narrow scope.** DGX Spark and Atlas in v1. No other engines, no media
   models, no cloud providers, no community host. [LLooM](https://github.com/Enntity/lloom)
   already covers all of that.

## Cluster sizes

GB10 has 128 GB of unified memory per Spark, of which the engine sees about
120 GB. Sparks join over direct ConnectX-7 cables, so a cluster is 1, 2 or 4
machines. Three Sparks run the two-Spark release on two of them.

### One Spark

No release is chosen. Atlas-Inf publishes single-GB10 numbers for several
candidates, but those come from short prompts with `max_tokens <= 30` and are
not Enntity measurements:

| Candidate | Upstream decode (tok/s) | Notes |
|---|---:|---|
| Qwen3.8-27B NVFP4 | 23.59 | Atlas-Inf's flagship; dense; MTP |
| Qwen3.5-35B-A3B NVFP4 | 131 | MTP K=2; 3B active |
| Qwen3.8-Flash-Next NVFP4 | 36.7 | ~180B MoE; streams a 47.7 GB table from NVMe |
| Nemotron-3.5-Lightning 30B-A3B + DSpark | "sub-10ms/token" | drafter-dependent |

Before this size ships, we need a one-Spark qualification in the style of
SparkQwen's: tool calling with streaming `tool_calls`, a long-context needle,
arithmetic, decode and prefill, all on our hardware with receipts. This is
probably the size with the most users, and it is the size we know least about.

### Two Sparks

Both engines have receipts on our pair, but from different benchmarks, so we
can't yet say which one is better:

| | SparkQwen RC16 | SparkGLM unified, TP2 |
|---|---|---|
| Model | Qwen3.8-Flash-Next NVFP4 | GLM-5.3-Flash FP4-Local |
| Context per request | 256K (`4x262k`) | 1M (`4x1m`) |
| Decode, one stream | sparkDash structured / prose / code: 102.0 / 72.7 / 93.8 | RigMark-derived structured / prose / code, Spark mode, median of 2: 89.8 / 37.5 / 67.2 |
| Tool calling | `tool_probe.py` 35/35 | ToolEvalBench 91.0 (FP4-Local release image) |

The default is decided by one shared qualification run on the same pair:
the same tool-calling suite, needle, quality probe and decode/prefill
workloads, run against both releases. Until that run exists, Shrug asks the
user to choose and shows both rows.

### Four Sparks

SparkGLM unified TP4. Medians of two runs, Spark mode: prose / code /
structured 57.1 / 104.1 / 137.9 tok/s on a chain and 58.3 / 106.4 / 141.4 on
a ring. Ring needs the closing rank 3 to rank 0 link; Shrug detects it and
otherwise uses the chain.

### Weight licenses

SparkGLM's Spark mode uses the `incoai/GLM-5.3-Flash-DFlash2` drafter, which
is CC BY-NC-ND 4.0 (non-commercial). Strict mode doesn't need it. Each
release in the catalog lists the licenses of the weights it downloads.
Shrug prints them in the plan and won't download a non-commercial weight
unless the user explicitly accepts it.

## Architecture

One static `aarch64-unknown-linux-musl` binary, `shrug`, installed on the
Spark that serves the API (rank 0). It reaches the other ranks over `ssh`.

```
shrug               probe + plan (read-only, the default)
shrug up --go       install or upgrade, start, verify, connect
shrug rollback      return to the previous release
shrug status        model, pins, KV pool, per-rank memory, links, recent throughput
shrug bench         quick benchmark beside the published receipts
shrug connect X     client config for claude-code, codex, opencode, omp
shrug down          stop every rank
shrug serve         the gateway (run as a systemd service by `up`)
```

### Probe

Read-only facts about this machine and its peers: GB10 present
(`nvidia-smi`), memory, disk free on the model root, Docker with the NVIDIA
runtime configured, `gpu-clock-cap.service` state, ConnectX-7 netdevs with
their RoCE IPv4 addresses, and the peer on each cable. Peers are found from
the cable addresses and confirmed over `ssh`. The result is one JSON
document; every later step reads it and none of them probes again.

### Catalog

The catalog lists the releases Shrug can install. Engine releases arrive every
few days, much faster than Shrug itself changes, so the catalog is versioned
and published separately from the binary:

- Entries are TOML files under `catalog/`, changed through reviewed pull
  requests. Release CI signs each catalog version with Ed25519 and attaches it
  to a GitHub release.
- The binary pins the public keys (two, so one can be rotated out) and rejects
  any catalog not signed by one of them. HTTPS from GitHub is not the trust
  root: a key fetched from the same host is not independent of it.
- The binary also embeds the catalog that was current when it was built, so
  a fresh install works offline and a failed fetch falls back to it.
- An entry can declare the oldest Shrug that understands it. An older binary
  lists that entry as needing an upgrade instead of trying to run it.
- A newer catalog only changes the plan. `shrug` reports that a newer release
  is available; `shrug up --go` drains requests, stops every rank, starts and
  verifies the new release, and keeps the previous one for `shrug rollback`.

Each release records:

- id, cluster size, topology (chain or ring), role (`agent` or `fast`)
- model: Hugging Face repo, immutable revision, size on disk, license
- extra weights (drafter): repo, revision, license
- engine: image reference by manifest digest, Atlas commit, source repo and tree
- profile and mode passed to the engine
- the client-visible model ID and context limit
- links to the receipts it was qualified with, plus the quick-bench numbers
  `shrug bench` compares against

LLooM's `backends/atlas-sparkglm/pins.json` and `backends/atlas-sparkqwen/pins.json`
already carry most of this and are the starting point.

### Engines

Shrug runs Atlas in Docker. DGX OS ships Docker and the NVIDIA Container
Toolkit, so there is nothing to install, though some Sparks still need the
NVIDIA runtime configured; `shrug` checks for that. Docker keeps each engine's
CUDA and NCCL versions independent of the host's, and an image digest is the
identity every receipt refers to.

Docker already stores the heavy part once. In the two images LLooM pins today
(2.18 GB and 2.02 GB compressed), about 1.70 GB is the same CUDA runtime base.
Each image adds about 480 MB and 320 MB of its own, mostly the layer from its
runtime package install. The Atlas binary itself is about 25 MB compressed.

What costs more is one image per model. SparkGLM and SparkQwen each build
Atlas for one model (`ATLAS_TARGET_MODEL=glm-5.3-flash` or
`qwen3.8-flash-next`). Atlas-Inf builds `ATLAS_TARGET_MODEL='*'` into one
multi-model `atlas-gb10` image instead. The target is the same for Shrug: one
multi-model image per engine release, built from Enntity/atlas `spark-shared`,
which already integrates the GLM and Qwen work. Models then become catalog
data (weights, profile and mode), and upgrading the engine is one pull per
Spark. SparkGLM and SparkQwen stay the qualification and release lines for
their models and publish into that image.

Two things have to happen first. SparkQwen `main` already pins `spark-shared`;
SparkGLM `main` still pins its own `sparkglm` branch and has to move. And a
build with every model compiled in is not the binary either repository
qualified, so both models need receipts on it. Until then, catalog entries
point at the per-model images.

Weights live in one store per Spark, laid out by Hugging Face repo and
revision, mounted read-only into whichever engine runs, and copied to the
other ranks once over the cable. Removing a release keeps its weights unless
the user asks to delete them.

A native bundle (binary and libraries, no Docker) would bring back the
dependence on the host's CUDA and NCCL, add a `sudo apt` step, and lose the
digest identity. We'll revisit it for Strix Halo, where Atlas already runs
natively.

### Install and start

Ported from SparkGLM's and SparkQwen's `start.sh` and `install/`, which already
work. Pull the image by digest on each rank and check its labels. Download
the weights once on rank 0 and copy them over the cable. Check the clock cap
and free memory on every rank. Start the workers first, then rank 0, and
wait for health. The steps are idempotent, so a rerun skips finished work.

Before starting anything, Shrug refuses to proceed if any rank is already
running an Atlas, LLooM or other GPU workload. It never stops a process it
didn't start.

### Verify

`up` reports ready only after a real chat completion, a streamed response,
and a streamed tool call with well-formed `tool_calls` deltas all succeed
through the Shrug gateway. Health checks alone don't count.

### Gateway

A thin authenticated proxy on `127.0.0.1:8145` in front of Atlas on rank 0's
loopback. Atlas already serves `/v1/chat/completions`, `/v1/responses` and
`/v1/messages`, so Shrug passes requests through without translating them.
It adds:

- API keys, required on any non-loopback bind
- `/v1/models` with the release's exact model ID and a `default` alias
- per-request metrics (time to first token, output rate, tokens) for `status`
- cancellation sent through to Atlas, and no idle timeout on long prefills
  (LLooM hit a five-minute transport deadline on cold 1M-token prompts)

A cluster runs one release at a time, because each engine takes most of
unified memory; packaging doesn't change that. Switching drains requests,
stops every rank, and starts the new release. That removes the admission
planner, eviction and co-residency logic that make up a large part of LLooM.

### Status and bench

`status` is the place to see how the cluster is running: release and pins, KV
pool size, free memory and GPU clock per rank, link state, and throughput from
recent requests. `bench` runs the release's quick workloads through the
gateway and prints your numbers next to the published ones, from the same
harness and settings, so a slow cable or missing clock cap shows up as a gap.

## What comes from where

| Source | Taken | Left behind |
|---|---|---|
| LLooM | GB10 and cluster detection (`machine-profile.mjs`, `cluster.mjs`), Atlas pin manifests and checks, client configs (`client-integrations.mjs`), `doctor` checks, dry-run / `--go` conventions, security defaults | vLLM, SGLang, MLX, llama.cpp, Ollama and media backends; community host and recipe packs; admission and eviction; head promotion, standby, reservations; external providers; Node |
| SparkGLM, SparkQwen | rank start order, clock-cap check, weight copy over the cable, profiles, quick-bench workloads and receipts | per-model shell entry points (Shrug becomes the entry point; the repos stay the engine releases) |
| Enntity/atlas | the engine, through the images those repos publish | nothing is vendored here |

Shrug doesn't build Atlas. It runs the images SparkGLM and SparkQwen publish
and qualify.

## Milestones

1. **Skeleton.** Repository, design, Rust workspace, CI.
2. **Probe and plan.** `shrug` on a real pair and a real quad prints the
   correct cluster, links, release and plan, with `--json`. Changes nothing.
3. **Two Sparks end to end.** `up --go` from bare Sparks to verified
   endpoint for both two-Spark releases. `down`, rerun and resume.
4. **Status, bench, connect.**
5. **Four Sparks.** TP4 chain and ring.
6. **One Spark.** After its qualification picks a release.
7. **Distribution.** Static release binaries, `install.sh` with checksum
   verification, signed catalogs, upgrade and rollback.

In parallel, outside this repository: SparkGLM onto `spark-shared` and a
qualified multi-model image. Strix Halo follows, as a second hardware family
in the catalog.

## Open questions

- **Upstream alignment.** Whether our multi-Spark engine work publishes into
  the same multi-model image line as Atlas-Inf's `atlas-gb10`, how
  single-Spark releases qualified upstream enter the catalog, and how Shrug
  relates to `sparkrun`.
- **Two-Spark default.** Decided by the shared qualification above.
- **One-Spark release.** Needs its own qualification.
- **Running from a laptop.** A macOS build that drives the Sparks over `ssh`
  would let an agent on the developer's machine manage the cluster. Not in v1.
