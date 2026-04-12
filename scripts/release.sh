#!/bin/bash
# Build static binaries and create a GitHub release
# Usage: ./scripts/release.sh v0.2.0

set -euo pipefail

VERSION=${1:?Usage: $0 <version>}
REPO="frahlg/home-ems"

echo "Building home-ems ${VERSION}..."
mkdir -p release

# Build static binaries for both architectures
for PLATFORM in linux/arm64 linux/amd64; do
    ARCH=$(echo $PLATFORM | cut -d/ -f2)
    echo "  Building ${ARCH}..."
    docker build --platform ${PLATFORM} -t home-ems:${ARCH} .
    docker create --name ems-extract home-ems:${ARCH}
    docker cp ems-extract:/app/home-ems release/home-ems-linux-${ARCH}
    docker rm ems-extract
    chmod +x release/home-ems-linux-${ARCH}
    tar czf release/home-ems-linux-${ARCH}.tar.gz \
        -C release home-ems-linux-${ARCH} \
        -C .. drivers/ web/ config.example.yaml
done

echo "Creating GitHub release ${VERSION}..."
gh release create ${VERSION} \
    release/home-ems-linux-arm64.tar.gz \
    release/home-ems-linux-amd64.tar.gz \
    --repo ${REPO} \
    --title "${VERSION}" \
    --generate-notes

echo "Done! Release: https://github.com/${REPO}/releases/tag/${VERSION}"
