<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Shrug design

Shrug installs and runs Atlas on DGX Sparks. It does a few things, and each
one has to work every time: find the hardware, choose the model, install it,
start it, prove it works, connect the agent, and report how it is running.

## Product rules

1. **Install and use.** One installer, one binary, one command from a bare
   Spark to a working endpoint. A step that needs the user is a bug, unless
   it is sudo or a license decision.
2. **The best model for each cluster size, and few others.** Each size has a
   default for agent work and at most one alternative that wins a job the
   default loses. A new model replaces the old one when it wins on the same
   hardware with the same benchmarks.
3. **Speed counts only with quality.** A release is listed only after its
   source qualified it for tool calling and quality on that cluster size.
4. **Agents are first-class users.** `--json` everywhere, stable exit codes
   (0 ok, 1 error, 2 usage, 3 blockers), plans that change nothing until
   `--go`, and blockers that name the command that fixes them.
5. **Narrow scope.** DGX Spark and Atlas. No other engines, media models,
   cloud providers or community host; [LLooM](https://github.com/Enntity/lloom)
   covers those.

## Releases

GB10 has 128 GB of unified memory per Spark. Sparks join over direct
ConnectX-7 cables, so a cluster is 1, 2 or 4 machines; three Sparks run the
two-Spark release on two of them.

| Sparks | Default | Alternative | Qualified by |
|---|---|---|---|
| 1 | Qwen3.8-27B NVFP4, 128K, MTP | Qwen3.8-Flash-Next NVFP4, 32K, n-gram table on NVMe | Atlas-Inf recipes |
| 2 | GLM-5.3-Flash FP4-Local, SparkGLM 0.1.0 Spark mode, 1M × 4 | Qwen3.8-Flash-Next FP4-Local, SparkQwen 0.3.1 Spark mode, 256K × 8 | Enntity |
| 4 | GLM-5.3-Flash, SparkGLM 0.1.0 TP4, ring when the 3–0 cable exists | | Enntity |

The single-Spark releases follow Atlas-Inf's `qwen3.8-27b-nvfp4-agentic` and
`qwen3.8-flash-next-nvfp4` recipes on their `atlas-gb10` image. The 27B
release departs from its recipe's gate settings in one way: 131072 context
with FP8 KV instead of 32K with BF16, which the recipe itself recommends for
agentic coding. Quality above about 24K is uncharacterized on those weights.
Flash-Next on one Spark is the bigger model but prefills at roughly 750 tok/s
and holds 32K, so it is the alternative, not the default.

GLM and Qwen on two Sparks were measured with different harnesses, so neither
release's numbers say which is better for agents. GLM is the default for its
1M context and tool-calling record; a shared qualification on one pair would
settle it.

SparkGLM's launcher mounts the `incoai/GLM-5.3-Flash-DFlash2` drafter in every
mode, and that drafter is CC BY-NC-ND 4.0. Shrug shows every weight license in
the plan and requires acceptance of a non-commercial one (a prompt during
`up --go`, or `--accept-noncommercial`), remembered per weight and revision.

## Architecture

One static `aarch64-unknown-linux-musl` binary. It runs on the Spark that
serves the API (rank 0) and reaches the others over ssh.

```
shrug               probe, discover, plan; changes nothing
shrug up --go       install or upgrade, start, verify, start the gateway
shrug rollback --go previous release
shrug status        release, pins, ranks, memory, clocks, gateway, recent requests
shrug bench         three streamed workloads, receipt under ~/.shrug/bench
shrug connect X     claude-code, codex, opencode, env
shrug logs | down | models | clock-cap --go
shrug serve         the gateway (started detached by `up`)
```

### Probe and discovery

A read-only shell probe on each host reports the GPU, memory, RDMA devices
with their IPv4 addresses, the LAN route, Docker and the NVIDIA toolkit, the
running containers and which of them hold a GPU, GPU processes mapped to
their containers, the clock-cap service, free disk and a few tools.

Peers are found from rank 0's cable addresses (the complement host on /30 and
/31, the other of .1/.2 on /24), probed over ssh, and the search continues
through each new peer by ProxyCommand chains that carry the same options and
identity files on every hop. `--hosts` skips discovery. Shrug keeps its own
known_hosts file and never edits `~/.ssh/known_hosts`. A host that answers
ping but refuses ssh becomes a blocker naming the `ssh-copy-id` to run.

Two ports are cabled when their IPv4 networks match. From the graph: one host
is single, two a pair, four of degree two a ring, four in a line a chain.
Rank 0 is the local machine; a chain must be run from an end. A pair uses its
lowest-subnet rail. On four Sparks each edge uses every rail, in subnet order,
and the leader address is the first of rank 0's addresses that every rank can
reach. On our cluster this reproduces the hand-written LLooM configuration.

Blockers apply to the hosts the release uses: no GB10, Docker unusable, no
NVIDIA toolkit, no clock cap, a GPU held by anything not named `shrug-*`, not
enough disk, an unaccepted non-commercial weight, too few Sparks. Problems on
unused hosts are warnings.

### Catalog

`catalog/releases.json` lists the releases. Each records the model and
drafter by Hugging Face repo and revision with sizes and licenses, the engine
image by digest and its Atlas commit, the launcher, per-rank argument
templates, the client-visible model ID, context, concurrency, receipts and
who qualified it. `shrug catalog build` inlines each launcher's text and
SHA-256 into `catalog/catalog.json`, which the binary embeds.

The launchers are SparkGLM's and SparkQwen's own `install/start-node.sh` at
the qualified commits, byte for byte, plus Shrug's `atlas-gb10.sh` for the
single-Spark image. Shrug pipes the release's launcher to `bash -s` on each
rank with the rendered arguments, so a release runs with the flags its
receipts were measured with. Format: [CATALOG.md](CATALOG.md).

Engine releases ship faster than Shrug, so the catalog updates on its own.
Release CI signs each published catalog with Ed25519 and uploads it to the
`catalog` prerelease. The binary pins two public keys (CI and an offline
backup), accepts only a signed catalog whose serial is not older than the
embedded or cached one, and falls back to the embedded copy. A new catalog
changes only the plan.

### Install, start, verify

1. Pull the image by digest on every rank (falling back to `docker save |
   ssh docker load` from rank 0), check it is arm64, and run `nvidia-smi -L`
   inside it.
2. Download weights with `hf`, or a pinned `python:3.12-slim` container with
   a pinned `huggingface_hub`, on rank 0 and on every rank not cabled to it,
   in parallel. A copy of the pinned revision already on the cluster — a
   LLooM model directory or a Hugging Face cache snapshot — is reused by
   hardlink, or pulled from a peer over the cable, before downloading.
   Copy to cabled ranks over the cable with eight rsync streams.
   A `.shrug-revision` marker is written last on each host.
3. Remove Shrug's previous containers, start ranks from the highest to rank 0,
   and wait up to 20 minutes for `/health`, watching every container.
4. Start the gateway, then verify through it: `17*23` answered `391` with a
   normal stop, a streamed reply, a streamed `get_weather` tool call with JSON
   arguments, and an Anthropic `/v1/messages` reply. Only then is it ready.

`up --go` on a healthy cluster already running the chosen release does
nothing; `--restart` relaunches.

### Gateway

`shrug serve` listens on `127.0.0.1:8145` by default and proxies
`/v1/chat/completions`, `/v1/completions`, `/v1/responses`, `/v1/messages` and
`/v1/messages/count_tokens` to the engine on `127.0.0.1:8893`. It rewrites
the request's model to the release's, streams responses frame by frame with
no timeouts, drops the upstream request when the client disconnects, and
records first-byte time and token usage for `status`. On a non-loopback
listen it generates an API key and requires it from non-loopback peers.

Engines don't restart themselves after a reboot; `shrug up --go` brings the
cluster back, skipping finished steps.

### Docker

DGX OS ships Docker and the NVIDIA container toolkit, and receipts name image
digests, so engines run as containers. The two Enntity images share their
1.7 GB CUDA base layer. The longer-term target is one multi-model Atlas image
per engine release, built from Enntity/atlas `spark-shared`, which SparkQwen
already pins; SparkGLM has to move to it and both models need receipts on
the combined build first.

## What is verified

- Unit and integration tests cover probe parsing, topology for every shape,
  argument rendering for every release, launch order, skip logic, the
  verification parsers, catalog signing, the gateway against a fake engine,
  the client configs, the bench parser and the installer.
- Read-only runs on our four Sparks found the ring, matched LLooM's per-rank
  devices and blocked on the production workloads running there.
- End-to-end `up --go` runs on real Sparks are tracked in the release notes.

## Open questions

- **Upstream alignment.** Whether the multi-Spark engine work publishes into
  Atlas-Inf's `atlas-gb10` image line, how upstream-qualified releases enter
  the catalog, and how Shrug relates to `sparkrun`.
- **Two-Spark default.** A shared qualification of both releases on one pair.
- **Strix Halo.** The next hardware family; Atlas already runs there natively.
