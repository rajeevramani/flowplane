#!/bin/bash
# Checkpoint Validation Script for Flowplane Control Plane

set -e

echo "🔍 Running Flowplane Checkpoint Validation..."
echo "==============================================="

# Change to project directory
cd "$(dirname "$0")"

# Code quality checks
echo "📋 Checking code format..."
if cargo fmt --check; then
    echo "✅ Code format check passed"
else
    echo "❌ Code format check failed"
    exit 1
fi

echo ""
echo "🔍 Running clippy..."
if cargo clippy -- -D warnings; then
    echo "✅ Clippy check passed"
else
    echo "❌ Clippy check failed"
    exit 1
fi

echo ""
echo "🔨 Building project..."
if cargo build; then
    echo "✅ Build successful"
else
    echo "❌ Build failed"
    exit 1
fi

echo ""
echo "🧪 Running tests..."
if RUN_E2E=1 cargo test -- --test-threads=1; then
    echo "✅ All tests passed"
else
    echo "❌ Tests failed"
    exit 1
fi

echo ""
echo "📦 Checking dependencies..."
if cargo check; then
    echo "✅ Dependency check passed"
else
    echo "❌ Dependency check failed"
    exit 1
fi

echo ""
echo "🎯 Checkpoint validation complete!"
echo "Ready for commit and checkpoint advancement."