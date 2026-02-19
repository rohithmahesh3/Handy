#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CUDA_PAYLOAD_DIR="${REPO_ROOT}/packaging/cuda-runtime/lib64"
CUDA_MANIFEST="${REPO_ROOT}/packaging/cuda-runtime/MANIFEST.sha256"
CUDA_REPORT="${REPO_ROOT}/packaging/cuda-runtime/STAGING_REPORT.json"

RUNTIME_VERSION="${NVIDIA_CUDA_RUNTIME_CU12_VERSION:-}"
CUBLAS_VERSION="${NVIDIA_CUBLAS_CU12_VERSION:-}"
CUFFT_VERSION="${NVIDIA_CUFFT_CU12_VERSION:-}"
CUDNN_VERSION="${NVIDIA_CUDNN_CU12_VERSION:-}"

runtime_pkg="nvidia-cuda-runtime-cu12"
cublas_pkg="nvidia-cublas-cu12"
cufft_pkg="nvidia-cufft-cu12"
cudnn_pkg="nvidia-cudnn-cu12"

if [[ -n "${RUNTIME_VERSION}" ]]; then
    runtime_pkg="${runtime_pkg}==${RUNTIME_VERSION}"
fi
if [[ -n "${CUBLAS_VERSION}" ]]; then
    cublas_pkg="${cublas_pkg}==${CUBLAS_VERSION}"
fi
if [[ -n "${CUFFT_VERSION}" ]]; then
    cufft_pkg="${cufft_pkg}==${CUFFT_VERSION}"
fi
if [[ -n "${CUDNN_VERSION}" ]]; then
    cudnn_pkg="${cudnn_pkg}==${CUDNN_VERSION}"
fi

required_sonames=(
    "libcudart.so.12"
    "libcublas.so.12"
    "libcublasLt.so.12"
    "libcufft.so.11"
    "libcudnn.so.9"
)

command -v python3 >/dev/null 2>&1 || {
    echo "Error: python3 is required."
    exit 1
}
python3 -m pip --version >/dev/null 2>&1 || {
    echo "Error: python3 pip is required (install python3-pip)."
    exit 1
}

echo "Staging CUDA runtime payload into ${CUDA_PAYLOAD_DIR}"
echo "Wheel specs:"
echo "  - ${runtime_pkg}"
echo "  - ${cublas_pkg}"
echo "  - ${cufft_pkg}"
echo "  - ${cudnn_pkg}"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

wheel_dir="${tmp_dir}/wheels"
extract_dir="${tmp_dir}/extract"
mkdir -p "${wheel_dir}" "${extract_dir}" "${CUDA_PAYLOAD_DIR}"

python3 -m pip download \
    --only-binary=:all: \
    --no-deps \
    --dest "${wheel_dir}" \
    "${runtime_pkg}" \
    "${cublas_pkg}" \
    "${cufft_pkg}" \
    "${cudnn_pkg}"

python3 - "${wheel_dir}" "${extract_dir}" <<'PY'
import pathlib
import sys
import zipfile

wheel_dir = pathlib.Path(sys.argv[1])
extract_dir = pathlib.Path(sys.argv[2])

wheels = sorted(wheel_dir.glob("*.whl"))
if not wheels:
    raise SystemExit("No wheels were downloaded.")

for wheel in wheels:
    wheel_target = extract_dir / wheel.stem
    wheel_target.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(wheel) as archive:
        archive.extractall(wheel_target)
PY

find "${CUDA_PAYLOAD_DIR}" -mindepth 1 -maxdepth 1 ! -name '.gitkeep' -exec rm -rf {} +

mapfile -t runtime_libs < <(
    find "${extract_dir}" \
        -type f \
        -path '*/nvidia/*/lib/*.so*' \
        -printf '%p\n' | sort -u
)

if [[ ${#runtime_libs[@]} -eq 0 ]]; then
    echo "Error: no runtime shared libraries found inside downloaded wheels."
    exit 1
fi

for lib in "${runtime_libs[@]}"; do
    cp -a "${lib}" "${CUDA_PAYLOAD_DIR}/"
done

for soname in "${required_sonames[@]}"; do
    if ! find "${CUDA_PAYLOAD_DIR}" -maxdepth 1 -name "${soname}*" | grep -q .; then
        echo "Error: staged payload missing required SONAME ${soname}"
        exit 1
    fi
done

(
    cd "${REPO_ROOT}/packaging/cuda-runtime"
    sha256sum lib64/* > MANIFEST.sha256
)

python3 - "${wheel_dir}" "${CUDA_PAYLOAD_DIR}" "${CUDA_REPORT}" <<'PY'
import json
import pathlib
import sys
from datetime import datetime, timezone

wheel_dir = pathlib.Path(sys.argv[1])
payload_dir = pathlib.Path(sys.argv[2])
report_path = pathlib.Path(sys.argv[3])

wheels = [p.name for p in sorted(wheel_dir.glob("*.whl"))]
libs = [p.name for p in sorted(payload_dir.glob("*.so*"))]

report = {
    "generated_at_utc": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
    "wheels": wheels,
    "libraries": libs,
}

report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
PY

echo "Staged ${#runtime_libs[@]} library files."
echo "Manifest: ${CUDA_MANIFEST}"
echo "Report:   ${CUDA_REPORT}"
