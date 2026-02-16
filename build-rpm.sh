#!/bin/bash
set -e

VERSION="0.7.5"
RELEASE="1"
DIST="fc40"

echo "=== Building Handy ${VERSION}-${RELEASE}.${DIST} ==="

# Check for required commands
command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Please install Rust."; exit 1; }
command -v meson >/dev/null 2>&1 || { echo "Error: meson not found. Please install meson."; exit 1; }
command -v rpmbuild >/dev/null 2>&1 || { echo "Error: rpmbuild not found. Please install rpm-build."; exit 1; }

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Create source tarball
echo "Creating source tarball..."
TARBALL="handy-${VERSION}.tar.gz"
git archive --format=tar.gz --prefix=Handy-fedora-gnome/ HEAD > "$TARBALL"
echo "Created $TARBALL"

# Build SRPM
echo ""
echo "=== Building SRPM ==="
rpmbuild -bs \
    --define "_sourcedir $(pwd)" \
    --define "_specdir $(pwd)" \
    --define "_srcrpmdir $(pwd)" \
    --define "_rpmdir $(pwd)" \
    packaging/fedora/handy.spec

SRPM=$(ls handy-${VERSION}-${RELEASE}*.src.rpm 2>/dev/null | head -1)
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
    packaging/fedora/handy.spec

echo ""
echo "=== Build Complete ==="
echo "SRPM: $SRPM"
echo "RPM packages:"
find . -name "handy-${VERSION}-${RELEASE}.${DIST}*.rpm" -type f 2>/dev/null | while read rpm; do
    echo "  - $rpm"
done

echo ""
echo "To install:"
echo "  sudo dnf install ./handy-${VERSION}-${RELEASE}.${DIST}.x86_64.rpm"
