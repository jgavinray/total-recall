# TRT-LLM on DGX Spark GB10 — Runbook

**Ticket:** SPARK-1  
**Goal:** Run Nemotron-3-Super-120B NVFP4 via TensorRT-LLM on DGX Spark GB10 for 3–5× throughput over llama.cpp  
**Last Updated:** 2026-03-20 (live audit via SSH)  
**Status:** Research Complete — Environment Audited Live  

---

## Environment Audit (LIVE — 2026-03-20 via SSH zoidberg@192.168.0.33)

### Hardware & OS

```
Host: spark-e294
SSH: zoidberg@192.168.0.33
```

**OS:**
```
Linux spark-e294 6.17.0-1008-nvidia #8-Ubuntu SMP PREEMPT_DYNAMIC Wed Jan 21 17:56:56 UTC 2026
Architecture: aarch64 (arm64)
OS: Ubuntu 24.04 (confirmed via container label)
```

### NVIDIA Driver & CUDA

```
Driver Version: 580.126.09
CUDA Version (host):  13.0  (reported by nvidia-smi)
CUDA Version (on disk): 13.0.96  (/usr/local/cuda-13.0)
nvcc: Not in $PATH (CUDA dev tools installed at /usr/local/cuda-13.0/)
```

### GPU

```
GPU:                 NVIDIA GB10 (DGX Spark)
Product Architecture: Blackwell
Compute Capability:  12.1 (sm_121)
Product Brand:       NVIDIA RTX
```

**Note:** GB10 uses unified memory architecture — `nvidia-smi --query-gpu=memory.total` returns `[N/A]`. Memory usage is tracked via per-process accounting.

**Current GPU Memory Usage (processes at time of audit):**
```
PID 2662947 (llama-server - Qwen3.5-122B):   76,857 MiB
PID 2662954 (llama-server - Qwen2.5-VL-7B):   6,904 MiB
Total in use: ~83,761 MiB (~81.8 GB)
```

**⚠️ GPU MEMORY CONFLICT:** llama.cpp processes currently consuming ~81.8 GB GPU memory. These must be stopped before TRT-LLM can load Nemotron-3-Super-120B.

### Docker

```
Docker version 29.1.3, build f52814d
```

### Disk Space

```
Device:    /dev/nvme0n1p2
Total:     3.7 TB
Used:      863 GB
Available: 2.7 TB
Mount:     /
```

**Docker disk usage:**
```
Images:       82.01 GB total (22 images)
Containers:   3.2 GB
Volumes:      373.6 MB
Build Cache:  4.9 GB
```

---

## TRT-LLM Container Research

### Container Already On Disk ✅

The correct TRT-LLM container for DGX Spark is **already pulled and available:**

```
Image: nvcr.io/nvidia/tensorrt-llm/release:spark-single-gpu-dev
Size:  37.1 GB
Local ID: sha256:474ca9e2e7b2d276d3d5b49dab602846067036fb59d07f9d9f822fbc253f794e
Digest:   sha256:4342a40dd7bdb4be9eeadd541aa3e739cd6d72c5441f36844c5345d07ec629da
Architecture: arm64 (linux/arm64)
Created: 2025-10-01
```

### Container Key Versions (from Docker labels)

| Component | Version |
|-----------|---------|
| **TensorRT-LLM** | **1.1.0rc3** |
| TensorRT | 10.11.0.33 |
| CUDA (in container) | 12.9 (nvcc 12.9.86) |
| PyTorch | 2.8.0a0+5228986 (nv25.6) |
| cuBLAS | 12.9.1.4 |
| cuDNN | 9.10.2.21 |
| NCCL | 2.27.3 |
| Python | 3.12 |
| Ubuntu | 24.04 |

### sm_121 Support Status

- **TRT-LLM 1.0:** Added sm_121 (Blackwell) support
- **TRT-LLM 1.1.0rc3 (this container):** Includes single-GPU DGX Spark beta support
- **Compute Capability 12.1:** Confirmed on live system via `nvidia-smi --query-gpu=compute_cap`

### Alternative/Newer Container Tags

For newer releases check:
- NGC Registry: `nvcr.io/nvidia/tensorrt-llm/release`
- Tags: `1.1.0`, `1.2.0`, `latest`
- Filter: `linux/arm64` architecture

**Release 1.2.0** adds expanded DGX Spark validation list — worth upgrading if available for aarch64.

---

## trtllm-serve Availability

### Status: ✅ CONFIRMED AVAILABLE

Binary location in container: `/usr/local/bin/trtllm-serve`

**⚠️ IMPORTANT:** Must set LD_LIBRARY_PATH or trtllm-serve crashes at import:
```
ImportError: libnvinfer.so.10: cannot open shared object file: No such file or directory
```

**Fix:** Add `/usr/local/tensorrt/targets/aarch64-linux-gnu/lib` to LD_LIBRARY_PATH

### Working Command to Launch trtllm-serve

```bash
docker run --rm --gpus all \
  -e LD_LIBRARY_PATH=/usr/local/tensorrt/targets/aarch64-linux-gnu/lib:$LD_LIBRARY_PATH \
  -p 8000:8000 \
  nvcr.io/nvidia/tensorrt-llm/release:spark-single-gpu-dev \
  trtllm-serve serve \
  <model_path> \
  --host 0.0.0.0 \
  --port 8000 \
  --backend pytorch \
  --tp_size 1
```

### trtllm-serve Commands (confirmed from --help)

```
Usage: trtllm-serve [OPTIONS] COMMAND [ARGS]...

Commands:
  serve                     Running an OpenAI API compatible server
  disaggregated             Running server in disaggregated mode
  disaggregated_mpi_worker  Launching disaggregated MPI worker
  mm_embedding_serve        Running an OpenAI API compatible server (multimodal)
```

### OpenAI-Compatible Endpoints

- `GET  /v1/models`
- `POST /v1/completions`
- `POST /v1/chat/completions`

---

## Nemotron-3-Super-120B (Nemotron-Nano-120B)

### HuggingFace Availability

| Model | HuggingFace Repo | Size | Type |
|-------|-----------------|------|------|
| BF16 (base) | `nvidia/Nemotron-3-Super-120B-A12B` | ~240 GB | Full precision |
| FP8 | `nvidia/Nemotron-3-Super-120B-A12B-FP8` | ~75 GB | FP8 quantized |
| **NVFP4** | **`nvidia/Nemotron-3-Super-120B-A12B-NVFP4`** | **~80 GB** | **Target** |
| GGUF | `bartowski/nvidia_Nemotron-3-Super-120B-A12B-GGUF` | ~70–120 GB | GGUF variants |

**Model Specs:**
- Total Parameters: 120B (12B active — MoE)
- Architecture: LatentMoE (Mamba-2 + MoE + Attention hybrid, MTP)
- Context Length: Up to 1M tokens
- Release Date: March 11, 2026
- License: NVIDIA Nemotron Open Model License
- Minimum GPU: 1× B200 OR 1× DGX Spark (GB10)

### ⚠️ Model Not in Validated List (Release 1.2.0)

The DGX Spark validated model list for TRT-LLM 1.2.0 does **not** include `Nemotron-3-Super-120B`. Validated 120B-class models:
- `openai/gpt-oss-120b` (MXFP4) — GPT-OSS-120B is validated

**Risk:** Nemotron-3-Super-120B NVFP4 may require testing/debugging for DGX Spark.

---

## Disk Feasibility

### Current Available Space

```
Available: 2.7 TB on /dev/nvme0n1p2
```

### Required Space Estimate

| Component | Size | Notes |
|-----------|------|-------|
| TRT-LLM Container | 37.1 GB | ✅ Already on disk |
| Nemotron NVFP4 weights | ~80 GB | Download from HuggingFace |
| TRT-LLM Engine build workspace | 200–400 GB | Temp space during build |
| Engine artifacts (output) | ~80–120 GB | Compiled engine |
| **Total needed** | **~400–640 GB** | Excluding container |

### Assessment: ✅ FEASIBLE

2.7 TB available >> 640 GB needed. Disk is not a constraint.

---

## Critical Bugs & Blockers

### Bug #1: FP4 CUTLASS GEMM on GB10 (GitHub #11368)

- **Issue:** `nvfp4_gemm_cutlass` fails on GB10 (SM121) with `"Error Internal no error"`
- **Root Cause:** SM120 tile configs require >99 KiB shared memory; GB10 only has 99 KiB vs B200's ~228 KiB
- **Impact:** TRT-LLM's CUTLASS FP4 backend cannot run on DGX Spark
- **Workaround:** cuBLASLt FP4 backend works — 99.6 TFLOPS vs BF16 baseline 8.4 TFLOPS
- **Status:** Bug filed; fix suggested in issue; unclear if fixed in 1.1.0rc3

**Performance on GB10 (from issue #11368):**
| Backend | Peak TFLOPS | Status on GB10 |
|---------|------------|----------------|
| cuBLASLt FP4 | 99.6 | ✅ Works |
| CUTLASS Example 79 (SM121 tiles) | 41.6 | ✅ Works |
| TRT-LLM CUTLASS FP4 | — | ❌ SMEM overflow |
| BF16 baseline | 8.4 | ✅ Reference |

### Bug #2: trtllm-serve LD_LIBRARY_PATH Missing

- **Issue:** `libnvinfer.so.10: cannot open shared object file: No such file or directory`
- **Fix:** `-e LD_LIBRARY_PATH=/usr/local/tensorrt/targets/aarch64-linux-gnu/lib:$LD_LIBRARY_PATH`
- **Impact:** Container entrypoint works fine with the env var; not a blocker once documented

### Blocker #3: GPU Memory Conflict

- **Current state:** llama.cpp consuming ~81.8 GB GPU memory
- **Required:** Free GPU memory before running TRT-LLM engine
- **Action needed:** Stop llama.cpp processes before TRT-LLM testing

---

## Feasibility Assessment

### ✅ What Works / Is Available

1. **TRT-LLM sm_121 support** — Release 1.0+ includes Blackwell (GB10)
2. **DGX Spark container** — `spark-single-gpu-dev` (TRT-LLM 1.1.0rc3) already on disk (37.1 GB)
3. **trtllm-serve** — Binary at `/usr/local/bin/trtllm-serve`, OpenAI-compatible, works with LD_LIBRARY_PATH fix
4. **Nemotron NVFP4** — Available on HuggingFace (~80 GB)
5. **Disk space** — 2.7 TB free, well above requirements
6. **CUDA 12.9 in container** — Compatible with TRT-LLM 1.1.0rc3

### ❌ Blockers

1. **FP4 CUTLASS bug** — May affect NVFP4 engine build; cuBLASLt workaround exists but needs validation
2. **Nemotron not in validated model list** — Untested combination, may need debugging
3. **llama.cpp GPU memory conflict** — Must be resolved before TRT-LLM testing

### ⚠️ Risks

1. **Shared memory limitation** — GB10's 99 KiB SMEM vs B200's 228 KiB impacts FP4 performance
2. **MoE architecture** — Nemotron-3-Super-120B uses LatentMoE (hybrid Mamba-2+MoE+Attention) — TRT-LLM MoE support on GB10 untested for this specific model
3. **Engine build time** — 120B model compilation may take hours
4. **Beta support** — DGX Spark support is marked "beta" and single-node only

---

## Next Steps

### Step 1: Stop llama.cpp (Required Before TRT-LLM)

```bash
# On Spark
sudo kill 2662947 2662954
# Or via docker compose if containerized
```

### Step 2: Download Nemotron NVFP4 Weights

```bash
# From within TRT-LLM container or on host
huggingface-cli download nvidia/Nemotron-3-Super-120B-A12B-NVFP4 \
  --local-dir /models/nemotron-3-super-120b-nvfp4 \
  --local-dir-use-symlinks False
```

Estimated download: ~80 GB. Destination needs ~80 GB free (available: 2.7 TB).

### Step 3: Run TRT-LLM Container

```bash
docker run --rm -it --gpus all \
  -e LD_LIBRARY_PATH=/usr/local/tensorrt/targets/aarch64-linux-gnu/lib:$LD_LIBRARY_PATH \
  -v /models:/models \
  -p 8000:8000 \
  nvcr.io/nvidia/tensorrt-llm/release:spark-single-gpu-dev \
  bash
```

### Step 4: Test with Validated Model First (De-Risk)

Before attempting Nemotron-3-Super-120B, test with a validated model:

```bash
# GPT-OSS-20B (MXFP4) is validated for DGX Spark
trtllm-serve serve openai/gpt-oss-20b \
  --host 0.0.0.0 \
  --port 8000 \
  --backend pytorch \
  --tp_size 1
```

### Step 5: Attempt Nemotron-3-Super-120B

```bash
trtllm-serve serve /models/nemotron-3-super-120b-nvfp4 \
  --host 0.0.0.0 \
  --port 8000 \
  --backend pytorch \
  --tp_size 1 \
  --trust_remote_code
```

### Step 6: Monitor for FP4 CUTLASS Bug

If engine build fails with SMEM errors, force cuBLASLt backend:
```bash
# Environment variable to investigate
export TRTLLM_FORCE_CUBLAS=1
# Or check TRT-LLM docs for backend selection flags in 1.1.0rc3
```

---

## Open Questions

1. **Is FP4 CUTLASS bug fixed in TRT-LLM 1.1.0rc3?** (Bug filed against 1.3.0rc2)
2. **Does TRT-LLM 1.1.0rc3 support Nemotron-3-Super-120B's LatentMoE architecture?**
3. **What env var/config forces cuBLASLt instead of CUTLASS for FP4 on GB10?**
4. **Should we consider TRT-LLM 1.2.0+ for expanded DGX Spark validation?** (Need to check if aarch64 image exists)

---

## Environment Audit Commands (Reference)

```bash
# OS and architecture
uname -a

# GPU and driver info
nvidia-smi
nvidia-smi --query-gpu=name,compute_cap --format=csv,noheader

# CUDA version (host)
cat /usr/local/cuda/version.json

# Docker version
docker --version

# Disk space
df -h

# GPU memory by process
nvidia-smi --query-compute-apps=pid,used_memory --format=csv,noheader

# trtllm-serve with correct LD_LIBRARY_PATH
docker run --rm --gpus all \
  -e LD_LIBRARY_PATH=/usr/local/tensorrt/targets/aarch64-linux-gnu/lib:$LD_LIBRARY_PATH \
  --entrypoint trtllm-serve \
  nvcr.io/nvidia/tensorrt-llm/release:spark-single-gpu-dev \
  serve --help
```

---

## Summary Table

| Item | Value | Status |
|------|-------|--------|
| Host | spark-e294 (192.168.0.33) | ✅ Accessible via `zoidberg@` |
| OS | Ubuntu 24.04, aarch64 | ✅ |
| Kernel | 6.17.0-1008-nvidia | ✅ |
| Driver | 580.126.09 | ✅ |
| CUDA (host) | 13.0 | ✅ |
| GPU | NVIDIA GB10, sm_121, Blackwell | ✅ |
| Docker | 29.1.3 | ✅ |
| TRT-LLM Container | `spark-single-gpu-dev` (1.1.0rc3) | ✅ On disk (37.1 GB) |
| CUDA (container) | 12.9 | ✅ |
| TensorRT | 10.11.0.33 | ✅ |
| trtllm-serve | Present at `/usr/local/bin/trtllm-serve` | ✅ Works (needs LD_LIBRARY_PATH) |
| Available Disk | 2.7 TB | ✅ Sufficient |
| Nemotron NVFP4 | ~80 GB on HuggingFace | ✅ Available |
| GPU Memory (current) | ~81.8 GB used by llama.cpp | ⚠️ Must free before TRT-LLM |
| FP4 CUTLASS on GB10 | SMEM overflow bug | ❌ Known issue; cuBLASLt workaround |
| Nemotron validated | Not in 1.2.0 validated list | ⚠️ Risk |
