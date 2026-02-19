#!/bin/bash
set -e

# Single source of truth - read version from Cargo.toml and release from RELEASE file
VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
RELEASE=$(cat RELEASE)
DIST="$(rpm --eval '%dist' | sed 's/^\.//' | tr -d '\n')"
if [ -z "$DIST" ]; then
    DIST="fc40"
fi
CUDA_PAYLOAD_DIR="packaging/cuda-runtime/lib64"
CUDA_MANIFEST="packaging/cuda-runtime/MANIFEST.sha256"
REQUIRED_CUDA_SONAMES=(libcudart.so.12 libcublas.so.12 libcublasLt.so.12 libcufft.so.11 libcudnn.so.9)

echo "=== Building ibus-handy ${VERSION}-${RELEASE}.${DIST} ==="

# Check for required commands
command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Please install Rust."; exit 1; }
command -v rpmbuild >/dev/null 2>&1 || { echo "Error: rpmbuild not found. Please install rpm-build."; exit 1; }
command -v rsync >/dev/null 2>&1 || { echo "Error: rsync not found. Please install rsync."; exit 1; }
command -v patchelf >/dev/null 2>&1 || { echo "Error: patchelf not found. Please install patchelf."; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo "Error: sha256sum not found."; exit 1; }
command -v rpm2cpio >/dev/null 2>&1 || { echo "Error: rpm2cpio not found."; exit 1; }
command -v cpio >/dev/null 2>&1 || { echo "Error: cpio not found."; exit 1; }
command -v ldd >/dev/null 2>&1 || { echo "Error: ldd not found."; exit 1; }

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

validate_cuda_payload() {
    if [ ! -d "$CUDA_PAYLOAD_DIR" ]; then
        echo "Error: missing CUDA payload directory: $CUDA_PAYLOAD_DIR"
        echo "Populate it with redistributable CUDA/cuDNN runtime libs before building."
        exit 1
    fi

    if [ ! -s "$CUDA_MANIFEST" ]; then
        echo "Error: missing or empty CUDA manifest: $CUDA_MANIFEST"
        echo "Generate payload via: ./scripts/stage-cuda-runtime-from-wheels.sh"
        exit 1
    fi

    if ! (cd packaging/cuda-runtime && sha256sum --strict --check MANIFEST.sha256); then
        echo "Error: CUDA payload manifest verification failed."
        exit 1
    fi

    for soname in "${REQUIRED_CUDA_SONAMES[@]}"; do
        if ! find "$CUDA_PAYLOAD_DIR" -maxdepth 1 -name "${soname}*" | grep -q .; then
            echo "Error: required CUDA runtime library missing from payload: ${soname}"
            exit 1
        fi
    done
}

validate_rpm_cuda_runtime_payload() {
    local rpm_file="$1"
    local rpm_contents
    rpm_contents="$(rpm -qlp "$rpm_file")"

    for provider_lib in \
        /handy/onnxruntime/libonnxruntime_providers_shared.so \
        /handy/onnxruntime/libonnxruntime_providers_cuda.so; do
        if ! grep -q "${provider_lib}$" <<<"$rpm_contents"; then
            echo "Error: RPM missing ${provider_lib##*/}"
            exit 1
        fi
    done

    for soname in "${REQUIRED_CUDA_SONAMES[@]}"; do
        if ! grep -E -q "/handy/cuda/${soname}(\\.|$)" <<<"$rpm_contents"; then
            echo "Error: RPM missing CUDA payload entry for ${soname}"
            exit 1
        fi
    done
}

validate_rpm_cuda_linking() {
    local rpm_file="$1"
    local extract_dir
    extract_dir="$(mktemp -d)"
    trap 'rm -rf "$extract_dir"' RETURN

    rpm2cpio "$rpm_file" | (cd "$extract_dir" && cpio -idmu --quiet)

    local provider_cuda=""
    local runtime_dir=""
    local cuda_dir=""
    if [ -f "$extract_dir/usr/lib64/handy/onnxruntime/libonnxruntime_providers_cuda.so" ]; then
        provider_cuda="$extract_dir/usr/lib64/handy/onnxruntime/libonnxruntime_providers_cuda.so"
        runtime_dir="$extract_dir/usr/lib64/handy/onnxruntime"
        cuda_dir="$extract_dir/usr/lib64/handy/cuda"
    elif [ -f "$extract_dir/usr/lib/handy/onnxruntime/libonnxruntime_providers_cuda.so" ]; then
        provider_cuda="$extract_dir/usr/lib/handy/onnxruntime/libonnxruntime_providers_cuda.so"
        runtime_dir="$extract_dir/usr/lib/handy/onnxruntime"
        cuda_dir="$extract_dir/usr/lib/handy/cuda"
    else
        echo "Error: extracted RPM missing CUDA provider library"
        exit 1
    fi

    local provider_rpath
    provider_rpath="$(patchelf --print-rpath "$provider_cuda")"
    if [ "$provider_rpath" != '$ORIGIN:$ORIGIN/../cuda' ]; then
        echo "Error: unexpected CUDA provider RUNPATH: $provider_rpath"
        exit 1
    fi

    local provider_shared="${provider_cuda%providers_cuda.so}providers_shared.so"
    local shared_rpath
    shared_rpath="$(patchelf --print-rpath "$provider_shared")"
    if [ "$shared_rpath" != '$ORIGIN' ]; then
        echo "Error: unexpected shared provider RUNPATH: $shared_rpath"
        exit 1
    fi

    local ldd_log="$extract_dir/ldd.log"
    LD_LIBRARY_PATH="$runtime_dir:$cuda_dir" ldd -r "$provider_cuda" >"$ldd_log" 2>&1 || true

    local unresolved
    unresolved="$(grep '=> not found' "$ldd_log" || true)"
    if [ -n "$unresolved" ]; then
        local unresolved_non_driver
        unresolved_non_driver="$(
            grep -Ev '^[[:space:]]*libcuda\.so\.1 => not found$' <<<"$unresolved" || true
        )"
        if [ -n "$unresolved_non_driver" ]; then
            echo "Error: unresolved runtime libraries detected in CUDA provider:"
            echo "$unresolved_non_driver"
            exit 1
        fi
    fi

    rm -rf "$extract_dir"
    trap - RETURN
}

echo "Validating staged CUDA runtime payload..."
validate_cuda_payload

# Generate spec file from template
echo "Generating spec file from template..."
sed -e "s/@VERSION@/$VERSION/g" \
    -e "s/@RELEASE@/$RELEASE/g" \
    packaging/fedora/ibus-handy.spec.in > packaging/fedora/ibus-handy.spec
echo "Generated packaging/fedora/ibus-handy.spec"

# Generate handy.xml from template
echo "Generating handy.xml from template..."
sed -e "s/@VERSION@/$VERSION/g" \
    packaging/fedora/handy.xml.in > packaging/fedora/handy.xml
echo "Generated packaging/fedora/handy.xml"

# Create source tarball
echo "Creating source tarball..."
TARBALL="ibus-handy-${VERSION}.tar.gz"
STAGE_DIR="$(mktemp -d)"
mkdir -p "${STAGE_DIR}/ibus-handy-${VERSION}"
rsync -a \
    --exclude='.git' \
    --exclude='target' \
    --exclude='x86_64' \
    --exclude='*.rpm' \
    --exclude='*.src.rpm' \
    --exclude='ibus-handy-*.tar.gz' \
    ./ "${STAGE_DIR}/ibus-handy-${VERSION}/"
tar -C "${STAGE_DIR}" -czf "$TARBALL" "ibus-handy-${VERSION}"
rm -rf "${STAGE_DIR}"
echo "Created $TARBALL"

# Build SRPM
echo ""
echo "=== Building SRPM ==="
rpmbuild -bs \
    --define "_sourcedir $(pwd)" \
    --define "_specdir $(pwd)" \
    --define "_srcrpmdir $(pwd)" \
    --define "_rpmdir $(pwd)" \
    packaging/fedora/ibus-handy.spec

SRPM=$(ls ibus-handy-${VERSION}-${RELEASE}*.src.rpm 2>/dev/null | head -1)
if [ -n "$SRPM" ]; then
    echo "Created SRPM: $SRPM"
else
    echo "Error: SRPM not found"
    exit 1
fi

# Build RPM
echo ""
echo "=== Building RPM ==="
rpmbuild -bb \
    --define "_sourcedir $(pwd)" \
    --define "_specdir $(pwd)" \
    --define "_srcrpmdir $(pwd)" \
    --define "_rpmdir $(pwd)" \
    --define "dist .${DIST}" \
    packaging/fedora/ibus-handy.spec

RPM_BIN=$(find . -name "ibus-handy-${VERSION}-${RELEASE}.${DIST}.x86_64.rpm" -type f | head -1)
if [ -z "$RPM_BIN" ]; then
    RPM_BIN=$(find . -name "ibus-handy-${VERSION}-${RELEASE}.${DIST}*.x86_64.rpm" -type f | head -1)
fi

if [ -z "$RPM_BIN" ]; then
    echo "Error: built x86_64 RPM not found for provider validation"
    exit 1
fi

echo "Validating ONNX provider and CUDA runtime payload in $RPM_BIN..."
validate_rpm_cuda_runtime_payload "$RPM_BIN"
echo "Validating extracted CUDA provider linking..."
validate_rpm_cuda_linking "$RPM_BIN"

echo ""
echo "=== Build Complete ==="
echo "SRPM: $SRPM"
echo "RPM packages:"
find . -name "ibus-handy-${VERSION}-${RELEASE}.${DIST}*.rpm" -type f 2>/dev/null | while read rpm; do
    echo "  - $rpm"
done

echo ""
echo "To install:"
echo "  sudo dnf install ./ibus-handy-${VERSION}-${RELEASE}.${DIST}.x86_64.rpm"
