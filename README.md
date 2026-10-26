<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<h1 align="center">¯\_(ツ)_/¯</h1>
<p align="center"><strong>Shrug</strong>: Atlas does the heavy lifting.</p>

Point Shrug at your DGX Sparks and it serves the best model they can run.
It finds your machines and the cables between them, picks a model for your
cluster size, pulls a pinned [Atlas](https://github.com/Enntity/atlas) engine
and pinned weights, starts every rank, checks that real tool calls work, and
gives your agents one OpenAI- and Anthropic-compatible endpoint.

> **Status: pre-alpha.** Nothing installs yet. This repository holds the
> design ([docs/DESIGN.md](docs/DESIGN.md)) and the first skeleton. The
> engines it will run already exist and have published receipts:
> [SparkQwen](https://github.com/Enntity/sparkqwen) and
> [SparkGLM](https://github.com/Enntity/sparkglm).

## What it will do

```sh
curl -fsSL https://github.com/Enntity/shrug/releases/latest/download/install.sh | sh
shrug               # look at this machine and its peers, show the plan
shrug up --go       # install, start, verify, connect your agents
```

`shrug` with no arguments changes nothing. It prints what it found and what
it would do: which Sparks, which links, which model, how much disk, how long
the download is. `--go` does it.

| Your cluster | Memory | Model | Engine release |
|---|---|---|---|
| 1 DGX Spark | 128 GB | Not chosen yet; see [the design](docs/DESIGN.md#one-spark) | none qualified |
| 2 DGX Sparks | 256 GB | Qwen3.8-Flash-Next or GLM-5.3-Flash | SparkQwen, SparkGLM |
| 4 DGX Sparks | 512 GB | GLM-5.3-Flash, tensor-parallel over four | SparkGLM TP4 |

One model per cluster size for each job, chosen by measurement on that
hardware. We add a model when it beats the current one, not to make the list
longer. The model list is signed and updates separately from Shrug itself,
and a new entry never touches a running cluster until you run `shrug up --go`.

Once it is running:

- `http://127.0.0.1:8145/v1` serves OpenAI chat, Responses and Anthropic
  messages, with streaming and tool calls.
- `shrug status` shows the model, engine pin, KV pool, free memory on each
  rank, link health, and recent throughput.
- `shrug bench` runs the release's quick benchmark on your machines and puts
  your numbers next to the published receipts, so you can tell whether your
  cluster performs the way ours did.
- `shrug connect claude-code` (or `codex`, `opencode`, `omp`) writes the
  client config.

Every command takes `--json`, so agents can drive it.

## Scope

Only DGX Spark, and only Atlas. AMD Strix Halo is the next platform, because
Atlas already runs there. Macs come when Atlas's Metal backend can serve a
model worth recommending. If you want vLLM, Ollama, MLX, image models or
cloud providers behind one gateway, use [LLooM](https://github.com/Enntity/lloom),
which Shrug is cut down from.

## License

AGPL-3.0-only ([LICENSE](LICENSE)). Model weights are not distributed here and
keep their own licenses; Shrug shows each license before downloading.

Shrug runs Enntity's fork of Atlas. It is not an Atlas-Inf project.
"DGX" and "Spark" are trademarks of NVIDIA Corporation; Shrug is not
affiliated with or endorsed by NVIDIA.
