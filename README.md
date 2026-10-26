<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<h1 align="center">¯\_(ツ)_/¯</h1>
<p align="center"><strong>Shrug</strong>: Atlas does the heavy lifting.</p>

Shrug runs the best [Atlas](https://github.com/Enntity/atlas) model your DGX
Sparks can hold. Run it on one Spark. It finds the other Sparks on the cables,
picks the model for your cluster size, pulls a pinned engine image and pinned
weights, starts every rank, checks that real tool calls work, and gives your
agents one OpenAI- and Anthropic-compatible endpoint.

```sh
curl -fsSL https://github.com/Enntity/shrug/releases/latest/download/install.sh | sh
shrug            # what it found and what it would do; changes nothing
shrug up --go    # do it
```

## What you get

| Sparks | Default | Context × requests | Option |
|---|---|---|---|
| 1 | Qwen3.8-27B, `qwen3.8-27b-1x` | 128K × 1 | Qwen3.8-Flash-Next, `qwen3.8-flash-next-1x` (32K × 1) |
| 2 | GLM-5.3-Flash, `glm-5.3-flash-2x` ([SparkGLM 0.1.0](https://github.com/Enntity/sparkglm/releases/tag/v0.1.0)) | 1M × 4 | Qwen3.8-Flash-Next, `qwen3.8-flash-next-2x` ([SparkQwen 0.3.1](https://github.com/Enntity/sparkqwen/releases/tag/v0.3.1)), 256K × 8 |
| 4 | GLM-5.3-Flash, `glm-5.3-flash-4x` (SparkGLM 0.1.0, ring or chain) | 1M × 4 | |

Three Sparks run the two-Spark release on two of them. Pick an option with
`--model <id>`; `shrug models` lists them with their licenses and receipts.

The two- and four-Spark releases are SparkGLM and SparkQwen as they were
qualified, with the same image digests, launchers and profiles. The one-Spark
releases are Atlas-Inf's own [recipes](https://github.com/Atlas-Inf/sparkrun-recipes)
on their `atlas-gb10` image; the 27B one runs at 128K context with an FP8 KV
cache, as that recipe recommends for agent work, rather than its 32K gate
setting.

## Before you start

- **DGX Spark with DGX OS.** Docker and the NVIDIA container toolkit come
  with it.
- **Disk.** Free space on each Spark: about 190 GB for GLM, 140 GB for
  Qwen3.8-Flash-Next, 25 GB for Qwen3.8-27B. Weights go under
  `~/.shrug/models`; Shrug reuses a copy of the pinned revision already on
  the cluster (LLooM's model directory or the Hugging Face cache) by
  hardlink or over the cable instead of downloading.
- **More than one Spark:** direct ConnectX-7 cables with an IPv4 address on
  each end, and passwordless ssh from the Spark you run Shrug on to the
  others. When ssh is missing, `shrug` prints the exact `ssh-copy-id` to run.
  Four Sparks can be a ring or a chain; on a chain, run Shrug on an end.
- **GPU clock cap.** A long cold prefill can power an uncapped Spark off.
  Run `shrug clock-cap --go` once; it asks for sudo on each Spark.
- **Nothing else on the GPUs.** Shrug won't start while another container or
  process holds a GPU, and it never stops anything it didn't start.

## Use it

The gateway listens on `http://127.0.0.1:8145` and serves
`/v1/chat/completions`, `/v1/responses`, `/v1/completions`, `/v1/messages`
and `/v1/models`, streaming and tool calls included. Whatever model name a
client sends, it gets the one the cluster runs; `default` works too.

```sh
shrug connect claude-code --go   # writes ~/.local/bin/shrug-claude
shrug connect codex --go         # adds a `shrug` profile to ~/.codex/config.toml
shrug connect opencode --go      # adds a `shrug` provider to opencode.json
shrug connect env                # prints OPENAI_* and ANTHROPIC_* exports
```

To reach it from your laptop, listen on the LAN. Shrug then creates an API
key and requires it from every machine but the Spark itself:

```sh
shrug up --go --listen 0.0.0.0:8145
shrug connect claude-code --remote   # prints what to set on the laptop
```

Other commands:

- `shrug status`: release, pins, each rank's container and free memory, clock
  cap, gateway health, recent requests and the engine's prefix-cache metrics.
- `shrug bench`: three short streamed workloads; prints first-token latency and
  decode tok/s for every repetition and saves a receipt under `~/.shrug/bench`.
- `shrug logs [--rank N] [-f]`, `shrug down`, `shrug rollback --go`.

Every command takes `--json`. Exit code 3 means the plan has blockers, and
the plan names each one with the command that fixes it.

After a reboot, run `shrug up --go` again. Finished steps are skipped, so it
only restarts the engine and the gateway.

## Updates

Shrug fetches a signed model list from this repository's `catalog` release.
It checks the signature against keys built into the binary, and falls back to
the copy built in when it's offline. A new entry changes only what `shrug`
proposes. Nothing moves until you run `shrug up --go`, and `shrug rollback --go`
returns to the previous release. Rerun the installer to update Shrug itself.

## Licenses

Shrug is AGPL-3.0-only ([LICENSE](LICENSE)). The weights keep their own
licenses, and `shrug` lists them before downloading. The GLM releases use the
`incoai/GLM-5.3-Flash-DFlash2` drafter, which is CC BY-NC-ND 4.0
(non-commercial); Shrug asks before downloading it, or takes
`--accept-noncommercial`.

Shrug runs Enntity's fork of Atlas for the multi-Spark releases and Atlas-Inf's
image for the single-Spark ones. "DGX" and "Spark" are trademarks of NVIDIA
Corporation; Shrug is not affiliated with or endorsed by NVIDIA.

Design and catalog format: [docs/DESIGN.md](docs/DESIGN.md), [docs/CATALOG.md](docs/CATALOG.md).
