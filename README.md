<!-- SPDX-License-Identifier: AGPL-3.0-only -->
# Pleiades

Point Pleiades at your DGX Sparks and it serves the best model they can run.
It finds your machines and the cables between them, picks a model for your
cluster size, pulls a pinned [Atlas](https://github.com/Enntity/atlas) engine
and pinned weights, starts every rank, checks that real tool calls work, and
gives your agents one OpenAI- and Anthropic-compatible endpoint.

The name comes from the sky. In myth the Pleiades are the daughters of Atlas,
and the star named Atlas (27 Tauri) is a member of the Pleiades cluster. This
project is Atlas, clustered.

> **Status: pre-alpha.** Nothing installs yet. This repository holds the
> design ([docs/DESIGN.md](docs/DESIGN.md)) and the first skeleton. The
> engines it will run already exist and have published receipts:
> [SparkQwen](https://github.com/Enntity/sparkqwen) and
> [SparkGLM](https://github.com/Enntity/sparkglm).

## What it will do

```sh
curl -fsSL https://github.com/Enntity/pleiades/releases/latest/download/install.sh | sh
pleiades            # look at this machine and its peers, show the plan
pleiades up --go    # install, start, verify, connect your agents
```

`pleiades` with no arguments changes nothing. It prints what it found and what
it would do: which Sparks, which links, which model, how much disk, how long
the download is. `--go` does it.

| Your cluster | Memory | Model | Engine release |
|---|---|---|---|
| 1 DGX Spark | 128 GB | Not chosen yet; see [the design](docs/DESIGN.md#one-spark) | none qualified |
| 2 DGX Sparks | 256 GB | Qwen3.8-Flash-Next or GLM-5.3-Flash | SparkQwen RC16, SparkGLM unified |
| 4 DGX Sparks | 512 GB | GLM-5.3-Flash, tensor-parallel over four | SparkGLM unified TP4 |

One model per cluster size for each job, chosen by measurement on that
hardware. We add a model when it beats the current one, not to make the list
longer.

Once it is running:

- `http://127.0.0.1:8145/v1` serves OpenAI chat, Responses and Anthropic
  messages, with streaming and tool calls.
- `pleiades status` shows the model, engine pin, KV pool, free memory on each
  rank, link health, and recent throughput.
- `pleiades bench` runs the release's quick benchmark on your machines and
  puts your numbers next to the published receipts, so you can tell whether
  your cluster performs the way ours did.
- `pleiades connect claude-code` (or `codex`, `opencode`, `omp`) writes the
  client config.

Every command takes `--json`, so agents can drive it.

## Scope

Only DGX Spark, and only Atlas. AMD Strix Halo is the next platform, because
Atlas already runs there. Macs come when Atlas's Metal backend can serve a
model worth recommending. If you want vLLM, Ollama, MLX, image models or
cloud providers behind one gateway, use [LLooM](https://github.com/Enntity/lloom),
which Pleiades is cut down from.

## License

AGPL-3.0-only ([LICENSE](LICENSE)). Model weights are not distributed here and
keep their own licenses; Pleiades shows each license before downloading.

Pleiades runs Enntity's fork of Atlas. It is not an Atlas-Inf project.
"DGX" and "Spark" are trademarks of NVIDIA Corporation; Pleiades is not
affiliated with or endorsed by NVIDIA.
