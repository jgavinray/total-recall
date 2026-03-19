# TRT-LLM on DGX Spark GB10 — Runbook

## Environment Audit (SPARK-1)

### Hardware & OS
**BLOCKER:** SSH access to Spark (192.168.0.33) requires password authentication. SSH key `~/.ssh/id_ed25519` was rejected with "Permission denied (publickey,password)".

*Pending manual password entry for:*
- OS/arch (`uname -a`)
- CUDA version (`nvcc --version` or `nvidia-smi`)
- Driver version
- Docker version
- Available disk space (`df -h`)
- Current GPU memory usage (`nvidia-smi`)

**Expected Environment (based on DGX Spark specifications):**
- GPU: NVIDIA GB10 (SM 12.1, Blackwell architecture)
- Architecture: aarch64 (ARM64)
- CUDA: Expected 12.8+ (based on TRT-LLM requirements)
- OS: Expected Linux aarch64 (Ubuntu 24.04 or similar)

### CUDA / Driver
**BLOCKER:** SSH access required

*From GitHub issue #11368 (tested environment):*
- GPU: NVIDIA GB10 (SM 12.1, DGX Spark)
- TRT-LLM: 1.3.0rc2 (commit f42a6cb)
- CUDA: 12.8
- PyTorch: 2.7 (CUDA 12.6)

### Docker
**BLOCKER:** SSH access required

*From release notes, expected base images:*
- TRT-LLM container: `nvcr.io/nvidia/pytorch:25.10-py3` (Release 1.1+)
- TRT-LLM Backend: `nvcr.io/nvidia/tritonserver:25.10-py3` (Release 1.1+)

### Disk
**BLOCKER:** SSH access required

*Estimated requirements (see Disk Requirements section below)*

### GPU
**BLOCKER:** SSH access required

*Expected:*
- GPU: NVIDIA GB10 (DGX Spark)
- Compute Capability: 12.1 (sm_121)
- Shared Memory: 99 KiB per block (vs B200's ~228 KiB)

---

## TRT-LLM Container Research

### Available Tags (aarch64)

**Container Registry:** https://catalog.ngc.nvidia.com/orgs/nvidia/teams/tensorrt-llm/containers/release/tags

**Latest Release Tags (from release notes):**
- Release 1.2.0 (Beta DGX Spark support)
- Release 1.1.0
- Release 1.0.0
- Release 0.21.0
- Release 0.20.0
- Release 0.19.0

**Base Images by Release:**
| Release | Base PyTorch | Base TritonServer | CUDA |
|---------|--------------|-------------------|------|
| 1.1+ | nvcr.io/nvidia/pytorch:25.10-py3 | nvcr.io/nvidia/tritonserver:25.10-py3 | 12.9 |
| 1.0 | nvcr.io/nvidia/pytorch:25.06-py3 | nvcr.io/nvidia/tritonserver:25.06-py3 | 12.9 |
| 0.21 | nvcr.io/nvidia/pytorch:25.05-py3 | nvcr.io/nvidia/tritonserver:25.05-py3 | 12.8.1 |
| 0.20 | nvcr.io/nvidia/pytorch:25.05-py3 | nvcr.io/nvidia/tritonserver:25.05-py3 | 12.8.1 |
| 0.19 | nvcr.io/nvidia/pytorch:25.03-py3 | nvcr.io/nvidia/tritonserver:25.03-py3 | 12.8.1 |
| 0.17 | nvcr.io/nvidia/pytorch:25.01-py3 | nvcr.io/nvidia/tritonserver:25.01-py3 | 12.8.0 |

**Recommended for DGX Spark:** Release 1.2.0 or later (first release with DGX Spark beta support)

### Correct Container for sm_121

**sm_121 Support Status:**
- **Release 1.0:** Added support for sm121
- **Release 1.2.0:** Added beta support for DGX Spark (single-node only)

**Validated Models for DGX Spark (Release 1.2.0):**
- GPT-OSS-20B, GPT-OSS-120B (MXFP4)
- Llama-3.1-8B-Instruct (FP16/FP8/NVFP4)
- Llama-3.3-70B-Instruct (FP8/NVFP4)
- Qwen3-8B, Qwen3-14B (FP16/FP8/NVFP4)
- Qwen3-32B (FP16/NVFP4)
- Qwen3-30B-A3B (FP16/NVFP4)
- NVIDIA-Nemotron-Nano-9B-v2 (FP4)
- Llama-3.3-Nemotron-Super-49B-v1.5 (FP8)
- Phi-4-multimodal-instruct (FP16/FP8/NVFP4)
- Phi-4-reasoning-plus (FP16/FP8/NVFP4)

**⚠️ CRITICAL BLOCKER - Nemotron-3-Super-120B NOT in validated list**

### trtllm-serve Availability

**Status:** ✅ Available

**Command Reference:**
```bash
trtllm-serve <model_path> \
  --host 0.0.0.0 \
  --port 8000 \
  --backend pytorch \
  --max_batch_size <size> \
  --max_num_tokens <tokens> \
  --tp_size <tensor_parallel> \
  --pp_size <pipeline_parallel> \
  --trust_remote_code
```

**OpenAI-Compatible Endpoints:**
- `/v1/models`
- `/v1/completions`
- `/v1/chat/completions`

**Example Usage:**
```bash
# From within the container
trtllm-serve openai/gpt-oss-120b \
  --host 0.0.0.0 \
  --port 8000 \
  --backend pytorch \
  --max_batch_size 720 \
  --max_num_tokens 16384 \
  --kv_cache_free_gpu_memory_fraction 0.9 \
  --tp_size 8 \
  --ep_size 8 \
  --trust_remote_code
```

---

## Nemotron-3-Super-120B

### HuggingFace Availability

**Model Card:** https://huggingface.co/nvidia/NVIDIA-Nemotron-3-Super-120B-A12B-NVFP4

**Model Versions Available:**
| Version | Type | Size |
|---------|------|------|
| NVIDIA-Nemotron-3-Super-120B-A12B-BF16 | Base (BF16) | ~240 GB |
| NVIDIA-Nemotron-3-Super-120B-A12B-FP8 | Quantized (FP8) | ~75 GB |
| **NVIDIA-Nemotron-3-Super-120B-A12B-NVFP4** | **Quantized (NVFP4)** | **80.4 GB** |
| bartowski/nvidia_Nemotron-3-Super-120B-A12B-GGUF | GGUF (various quantizations) | ~70-120 GB |

**Release Date:** March 11, 2026

**License:** NVIDIA Nemotron Open Model License

### Model Size / Disk Requirements

**Model Specifications:**
- Total Parameters: 120B (12B active)
- Architecture: LatentMoE - Mamba-2 + MoE + Attention hybrid with Multi-Token Prediction (MTP)
- Context Length: Up to 1M tokens
- Minimum GPU Requirement: 1× B200 OR 1× DGX Spark

**Disk Space Requirements (NVFP4 version):**
| Component | Size | Notes |
|-----------|------|-------|
| Model Weights | 80.4 GB | NVFP4 quantized checkpoint |
| Tokenizer | ~1 GB | Included in model repo |
| Engine Build Workspace | 200-400 GB | TRT-LLM engine compilation |
| Container Image | 15-25 GB | PyTorch 25.10-py3 based |
| **Total Estimated** | **300-520 GB** | With safety margin |

**Recommendation:** Minimum 500 GB free disk space for full workflow (weights + engine build + container)

---

## Feasibility Assessment

### ✅ What Works

1. **TRT-LLM sm_121 Support:** Release 1.0+ adds sm_121 (Blackwell) support
2. **DGX Spark Beta Support:** Release 1.2.0+ includes beta support for single-node DGX Spark
3. **NVFP4 Quantization:** Supported on Blackwell hardware (GB10)
4. **trtllm-serve:** Available with OpenAI-compatible endpoints
5. **Model Availability:** Nemotron-3-Super-120B NVFP4 available on HuggingFace (80.4 GB)

### ❌ Critical Blockers

1. **FP4 CUTLASS GEMM Bug (GitHub #11368):**
   - **Issue:** nvfp4_gemm_cutlass fails on GB10 (SM121) with "Error Internal no error"
   - **Root Cause:** SM120 tile configs require >99 KiB shared memory, but GB10 only has 99 KiB vs B200's ~228 KiB
   - **Impact:** TRT-LLM's CUTLASS-based FP4 backend cannot be used on DGX Spark
   - **Workaround:** cuBLASLt FP4 backend works (99.6 TFLOPS vs CUTLASS's failure)
   - **Status:** Bug reported, fix suggested in issue

2. **Model Not Validated:** Nemotron-3-Super-120B NOT in DGX Spark validated models list (Release 1.2.0)

3. **SSH Access Required:** Cannot complete environment audit without password authentication

### ⚠️ Risks

1. **Shared Memory Limitation:** GB10's 99 KiB SMEM vs B200's 228 KiB may impact performance
2. **Beta Support:** DGX Spark support is marked as "beta" - single-node only
3. **Engine Build Time:** Large model (120B) may require significant time to build TRT-LLM engines
4. **Memory Pressure:** 120B model even at NVFP4 may push GB10 memory limits

### 📊 Performance Comparison (from GitHub #11368)

| Backend | Peak TFLOPS | Status on GB10 |
|---------|-------------|----------------|
| cuBLASLt FP4 | 99.6 | ✅ Works |
| CUTLASS Example 79 (SM121 tiles) | 41.6 | ✅ Works |
| TRT-LLM CUTLASS FP4 | — | ❌ SMEM overflow |
| BF16 baseline | 8.4 | ✅ Reference |

---

## Open Questions / Blockers

### Immediate Blockers

1. **SSH Access:** Password required for Spark (192.168.0.33)
   - User: gavinray
   - Action: Provide SSH password or configure key-based auth

2. **FP4 CUTLASS Workaround:**
   - Can we force TRT-LLM to use cuBLASLt instead of CUTLASS for FP4 on GB10?
   - What environment variables or config options control backend selection?

3. **Engine Build Feasibility:**
   - Has anyone built a 120B MoE engine on single-node GB10?
   - What are the memory requirements during engine build (not inference)?

### Research Needed

1. **Container Tag Verification:**
   - Confirm exact container tag for TRT-LLM 1.2.0+ with aarch64 support
   - Verify sm_121 is in the container's supported architectures list

2. **Nemotron-3-Super-120B Specifics:**
   - Are there TRT-LLM-specific conversion scripts for this model?
   - Does the model require any special handling for MoE layers?

3. **Disk Space Verification:**
   - Confirm actual available disk space on Spark
   - Assess if expansion is needed before engine build

### Next Steps

1. **Enable SSH Access:** Provide password or configure SSH keys for Spark
2. **Complete Environment Audit:** Run diagnostic commands on Spark
3. **Test cuBLASLt Backend:** Verify if cuBLASLt FP4 backend can be used with TRT-LLM
4. **Attempt Small-Scale Test:** Build engine for a smaller validated model first (e.g., GPT-OSS-20B)
5. **Monitor TRT-LLM Releases:** Watch for FP4 CUTLASS fix for GB10

---

## Appendix: Commands for Environment Audit

When SSH access is available, run:

```bash
# OS and architecture
uname -a

# CUDA version
nvcc --version

# GPU and driver info
nvidia-smi

# Docker version
docker --version

# Disk space
df -h

# GPU memory usage (if any processes running)
nvidia-smi --query-gpu=memory.used,memory.total --format=csv
```

---

**Last Updated:** 2026-03-19 11:38 PDT
**Status:** Research Phase - Environment Audit Pending SSH Access
