#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Start the Atlas GB10 engine in Docker on one Spark. Shrug renders the
# arguments from the catalog release and pipes this script to bash -s.
set -euo pipefail

image= model_root= checkpoint= cuda_cache= name=
while [ $# -gt 0 ]; do
  case "$1" in
    --image) image=$2; shift 2 ;;
    --model-root) model_root=$2; shift 2 ;;
    --checkpoint) checkpoint=$2; shift 2 ;;
    --cuda-cache) cuda_cache=$2; shift 2 ;;
    --name) name=$2; shift 2 ;;
    --) shift; break ;;
    *) echo "atlas-gb10: unknown argument $1" >&2; exit 2 ;;
  esac
done
for v in image model_root checkpoint cuda_cache name; do
  eval "test -n \"\$$v\"" || { echo "atlas-gb10: --$v is required" >&2; exit 2; }
done

ckpt=$(realpath "$model_root/$checkpoint")
[ -f "$ckpt/config.json" ] || { echo "atlas-gb10: no checkpoint at $ckpt" >&2; exit 1; }
mkdir -p "$cuda_cache"
docker rm -f "$name" >/dev/null 2>&1 || true

docker run -d --name "$name" --restart no --network host --ipc host \
  --shm-size 32g --memory 114g --gpus all \
  --cap-add IPC_LOCK --cap-add SYS_NICE --ulimit memlock=-1:-1 \
  --security-opt no-new-privileges=true --stop-timeout 60 \
  --mount "type=bind,src=$ckpt,dst=$ckpt,readonly" \
  --mount "type=bind,src=$cuda_cache,dst=/atlas-cuda-cache" \
  -e HF_HUB_OFFLINE=1 -e TRANSFORMERS_OFFLINE=1 \
  -e CUDA_CACHE_PATH=/atlas-cuda-cache -e CUDA_CACHE_MAXSIZE=4294967296 \
  "$image" serve "$ckpt" "$@"
