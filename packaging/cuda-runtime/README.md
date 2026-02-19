# CUDA Runtime Payload (Website RPM Builds)

This directory is the build input for bundling optional NVIDIA GPU runtime libraries
into the Handy RPM.

## Required layout

- `packaging/cuda-runtime/lib64/` contains runtime `.so` files.
- `packaging/cuda-runtime/MANIFEST.sha256` contains checksums for everything under `lib64/`.
- `packaging/cuda-runtime/STAGING_REPORT.json` is generated metadata for staged wheel inputs.

## Recommended staging command

```bash
./scripts/stage-cuda-runtime-from-wheels.sh
```

Optional environment variables to pin wheel versions:

- `NVIDIA_CUDA_RUNTIME_CU12_VERSION`
- `NVIDIA_CUBLAS_CU12_VERSION`
- `NVIDIA_CUFFT_CU12_VERSION`
- `NVIDIA_CUDNN_CU12_VERSION`

## Minimum SONAME coverage

The payload must provide these SONAMEs:

- `libcudart.so.12`
- `libcublas.so.12`
- `libcublasLt.so.12`
- `libcufft.so.11`
- `libcudnn.so.9`

Include any additional transitive libraries needed by `libonnxruntime_providers_cuda.so`.

## Build-time manifest

The staging script regenerates the manifest automatically.

`./build-rpm.sh` will fail if the manifest is missing, empty, or mismatched.
