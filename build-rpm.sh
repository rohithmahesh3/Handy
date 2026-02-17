#!/bin/bash
set -e

VERSION="0.7.5"
RELEASE="9"
DIST="$(rpm --eval '%dist' | sed 's/^\.//' | tr -d '\n')"
if [ -z "$DIST" ]; then
    DIST="fc40"
fi

echo "=== Building ibus-handy ${VERSION}-${RELEASE}.${DIST} ==="

# Check for required commands
command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Please install Rust."; exit 1; }
command -v rpmbuild >/dev/null 2>&1 || { echo "Error: rpmbuild not found. Please install rpm-build."; exit 1; }
command -v rsync >/dev/null 2>&1 || { echo "Error: rsync not found. Please install rsync."; exit 1; }

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

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
