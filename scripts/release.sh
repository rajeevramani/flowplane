#!/bin/bash
set -euo pipefail

# Flowplane Release Script
# Usage: ./scripts/release.sh [patch|minor|major]

RELEASE_TYPE="${1:-patch}"

# Ensure we're on main branch
CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo "Error: Must be on main branch to create release"
    exit 1
fi

# Ensure working directory is clean
if ! git diff-index --quiet HEAD --; then
    echo "Error: Working directory is not clean"
    exit 1
fi

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
echo "Current version: $CURRENT_VERSION"

# Calculate new version
case $RELEASE_TYPE in
    patch)
        NEW_VERSION=$(echo $CURRENT_VERSION | awk -F. '{$NF = $NF + 1;} 1' | sed 's/ /./g')
        ;;
    minor)
        NEW_VERSION=$(echo $CURRENT_VERSION | awk -F. '{$(NF-1) = $(NF-1) + 1; $NF = 0;} 1' | sed 's/ /./g')
        ;;
    major)
        NEW_VERSION=$(echo $CURRENT_VERSION | awk -F. '{$1 = $1 + 1; $2 = 0; $NF = 0;} 1' | sed 's/ /./g')
        ;;
    *)
        echo "Error: Invalid release type. Use patch, minor, or major"
        exit 1
        ;;
esac

echo "New version: $NEW_VERSION"

# Update Cargo.toml
sed -i.bak "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
rm Cargo.toml.bak

# Update Cargo.lock
cargo check

# Run tests to ensure everything works
echo "Running tests..."
cargo test --all-features

# Create commit and tag
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to $NEW_VERSION

🤖 Generated with Claude Code

Co-Authored-By: Claude <noreply@anthropic.com>"

git tag -a "v$NEW_VERSION" -m "Release version $NEW_VERSION"

echo "Release $NEW_VERSION created successfully!"
echo "Push with: git push origin main --tags"