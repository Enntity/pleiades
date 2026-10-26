<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Pleiades design

Pleiades installs and runs Atlas on DGX Sparks. It does a few things, and each
one has to work every time: find the hardware, choose the model, install it,
start it, prove it works, connect the agent, and report how it is running.

## Product rules

1. **Install and use.** One installer, one binary, one command to get from a
   bare Spark to a working endpoint. Anything that needs a manual step is a
   bug in Pleiades or in its documentation.
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
workloads, run against both releases. Until that run exists, Pleiades asks
the user to choose and shows both rows.

### Four Sparks

SparkGLM unified TP4. Medians of two runs, Spark mode: prose / code /
structured 57.1 / 104.1 / 137.9 tok/s on a chain and 58.3 / 106.4 / 141.4 on
a ring. Ring needs the closing rank 3 to rank 0 link; Pleiades detects it and
otherwise uses the chain.

### Weight licenses

SparkGLM's Spark mode uses the `incoai/GLM-5.3-Flash-DFlash2` drafter, which
is CC BY-NC-ND 4.0 (non-commercial). Strict mode doesn't need it. Each
release in the catalog lists the licenses of the weights it downloads.
Pleiades prints them in the plan and won't download a non-commercial weight
unless the user explicitly accepts it.

## Architecture

One static `aarch64-unknown-linux-musl` binary, `pleiades`, installed on the
Spark that serves the API (rank 0). It reaches the other ranks over `ssh`.

```
pleiades            probe + plan (read-only, the default)
pleiades up --go    install, start, verify, connect
pleiades status     model, pins, KV pool, per-rank memory, links, recent throughput
pleiades bench      quick benchmark beside the published receipts
pleiades connect X  client config for claude-code, codex, opencode, omp
pleiades down       stop every rank
pleiades serve      the gateway (run as a systemd service by `up`)
```

### Probe

Read-only facts about this machine and its peers: GB10 present
(`nvidia-smi`), memory, disk free on the model root, Docker with the NVIDIA
runtime, `gpu-clock-cap.service` state, ConnectX-7 netdevs with their RoCE
IPv4 addresses, and the peer on each cable. Peers are found from the cable
addresses and confirmed over `ssh`. The result is one JSON document; every
later step reads it and none of them probes again.

### Catalog

Releases are TOML files under `catalog/`, compiled into the binary. A new
catalog means a new Pleiades release: no remote feed, no signing
infrastructure, and nothing that can change under a running cluster. Each
release records:

- id, cluster size, topology (chain or ring), role (`agent` or `fast`)
- model: Hugging Face repo, immutable revision, size on disk, license
- extra weights (drafter): repo, revision, license
- engine: image reference by manifest digest, Atlas commit, source repo and tree
- profile and mode passed to the engine
- the client-visible model ID and context limit
- links to the receipts it was qualified with, plus the quick-bench numbers
  `pleiades bench` compares against

LLooM's `backends/atlas-sparkglm/pins.json` and `backends/atlas-sparkqwen/pins.json`
already carry most of this and are the starting point.

### Install and start

Ported from SparkGLM's and SparkQwen's `start.sh` and `install/`, which already
work. Pull the image by digest on each rank and check its labels. Download
the weights once on rank 0 and copy them over the cable. Check the clock cap
and free memory on every rank. Start the workers first, then rank 0, and
wait for health. The steps are idempotent, so a rerun skips finished work.

Before starting anything, Pleiades refuses to proceed if any rank is already
running an Atlas, LLooM or other GPU workload. It never stops a process it
didn't start.

### Verify

`up` reports ready only after a real chat completion, a streamed response,
and a streamed tool call with well-formed `tool_calls` deltas all succeed
through the Pleiades gateway. Health checks alone don't count.

### Gateway

A thin authenticated proxy on `127.0.0.1:8145` in front of Atlas on rank 0's
loopback. Atlas already serves `/v1/chat/completions`, `/v1/responses` and
`/v1/messages`, so Pleiades passes requests through without translating
them. It adds:

- API keys, required on any non-loopback bind
- `/v1/models` with the release's exact model ID and a `default` alias
- per-request metrics (time to first token, output rate, tokens) for `status`
- cancellation sent through to Atlas, and no idle timeout on long prefills
  (LLooM hit a five-minute transport deadline on cold 1M-token prompts)

A cluster runs one release at a time. Switching (`pleiades use <id>`) drains
requests, stops every rank, and starts the new release. That removes the
admission planner, eviction and co-residency logic that make up a large part
of LLooM.

### Status and bench

`status` is the place to see how the cluster is running: release and pins, KV
pool size, free memory and GPU clock per rank, link state, and throughput from
recent requests. `bench` runs the release's quick workloads through the
gateway and prints your numbers next to the published ones, from the same
harness and settings, so a slow cable or missing clock cap shows up as a gap.

## What comes from where

| Source | Taken | Left behind |
|---|---|---|
| LLooM | GB10 and cluster detection (`machine-profile.mjs`, `cluster.mjs`), Atlas pin manifests and checks, client configs (`client-integrations.mjs`), `doctor` checks, dry-run / `--go` conventions, security defaults | vLLM, SGLang, MLX, llama.cpp, Ollama and media backends; community host and signed packs; admission and eviction; head promotion, standby, reservations; external providers; Node |
| SparkGLM, SparkQwen | rank start order, clock-cap check, weight copy over the cable, profiles, quick-bench workloads and receipts | per-model shell entry points (Pleiades becomes the entry point; the repos stay the engine releases) |
| Enntity/atlas | the engine, through the images those repos publish | nothing is vendored here |

The engine stays in SparkGLM and SparkQwen. Pleiades doesn't build Atlas.
It runs the images those repositories publish and qualify.

## Milestones

1. **Skeleton.** Repository, design, Rust workspace, CI.
2. **Probe and plan.** `pleiades` on a real pair and a real quad prints the
   correct cluster, links, release and plan, with `--json`. Changes nothing.
3. **Two Sparks end to end.** `up --go` from bare Sparks to verified
   endpoint for both two-Spark releases. `down`, rerun and resume.
4. **Status, bench, connect.**
5. **Four Sparks.** TP4 chain and ring.
6. **One Spark.** After its qualification picks a release.
7. **Distribution.** Static release binaries, `install.sh` with checksum
   verification, upgrade in place.

Strix Halo follows, as a second hardware family in the catalog.

## Open questions

- **Atlas branding.** Using the Atlas name or visual identity in Pleiades
  materials needs Atlas-Inf's agreement. Until then, the docs say "runs
  Enntity's fork of Atlas" and nothing more.
- **Two-Spark default.** Decided by the shared qualification above.
- **One-Spark release.** Needs its own qualification.
- **Running from a laptop.** A macOS build that drives the Sparks over `ssh`
  would let an agent on the developer's machine manage the cluster. Not in v1.
