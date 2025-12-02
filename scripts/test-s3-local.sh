#!/bin/bash
# Run S3 integration tests locally with MinIO
# Usage: ./scripts/test-s3-local.sh
#
# This script:
# 1. Starts MinIO in a Docker container
# 2. Creates a test bucket and uploads test data
# 3. Runs the S3 integration tests
# 4. Cleans up

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

MINIO_CONTAINER="tileyolo-minio-test"
MINIO_PORT=9000
MINIO_USER="minioadmin"
MINIO_PASS="minioadmin"
BUCKET="test-bucket"

cleanup() {
    echo "Cleaning up..."
    docker stop "$MINIO_CONTAINER" 2>/dev/null || true
    docker rm "$MINIO_CONTAINER" 2>/dev/null || true
}

trap cleanup EXIT

echo "Starting MinIO container..."
docker run -d --name "$MINIO_CONTAINER" \
    -p "$MINIO_PORT:9000" \
    -e "MINIO_ROOT_USER=$MINIO_USER" \
    -e "MINIO_ROOT_PASSWORD=$MINIO_PASS" \
    quay.io/minio/minio:latest server /data

echo "Waiting for MinIO to be ready..."
for i in {1..30}; do
    if curl -sf "http://localhost:$MINIO_PORT/minio/health/live" > /dev/null 2>&1; then
        echo "MinIO is ready"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "MinIO failed to start"
        exit 1
    fi
    sleep 1
done

echo "Installing MinIO client (mc) if needed..."
if ! command -v mc &> /dev/null; then
    if [[ "$OSTYPE" == "darwin"* ]]; then
        brew install minio/stable/mc 2>/dev/null || {
            curl -sLo /tmp/mc https://dl.min.io/client/mc/release/darwin-amd64/mc
            chmod +x /tmp/mc
            MC="/tmp/mc"
        }
    else
        curl -sLo /tmp/mc https://dl.min.io/client/mc/release/linux-amd64/mc
        chmod +x /tmp/mc
        MC="/tmp/mc"
    fi
else
    MC="mc"
fi

echo "Configuring MinIO client..."
$MC alias set local "http://localhost:$MINIO_PORT" "$MINIO_USER" "$MINIO_PASS" --api S3v4

echo "Creating test bucket..."
$MC mb "local/$BUCKET" || true

echo "Uploading test data..."
# Look for test files in common locations
TEST_FILE=""
for f in "$PROJECT_DIR/data/test/gray_3857.tif" \
         "$PROJECT_DIR/data/viridis/"*.tif \
         "$PROJECT_DIR/test_data/viridis/"*.tif; do
    if [ -f "$f" ]; then
        TEST_FILE="$f"
        break
    fi
done

if [ -n "$TEST_FILE" ]; then
    BASENAME=$(basename "$TEST_FILE")
    $MC cp "$TEST_FILE" "local/$BUCKET/viridis/$BASENAME"
    echo "Uploaded: $TEST_FILE -> $BUCKET/viridis/$BASENAME"
else
    echo "Warning: No test GeoTIFF found, tests may fail"
    # Create minimal test file
    if command -v gdal_create &> /dev/null; then
        gdal_create -of GTiff -outsize 256 256 -bands 1 -burn 128 \
            -a_srs EPSG:3857 \
            -a_ullr -20037508.34 20037508.34 20037508.34 -20037508.34 \
            /tmp/test.tif
        $MC cp /tmp/test.tif "local/$BUCKET/viridis/test.tif"
        echo "Created and uploaded minimal test.tif"
    fi
fi

echo ""
echo "Bucket contents:"
$MC ls --recursive "local/$BUCKET/"

echo ""
echo "Running S3 integration tests..."
cd "$PROJECT_DIR"

export AWS_ACCESS_KEY_ID="$MINIO_USER"
export AWS_SECRET_ACCESS_KEY="$MINIO_PASS"
export AWS_ENDPOINT_URL="http://localhost:$MINIO_PORT"
export AWS_ALLOW_HTTP="true"
export AWS_REGION="us-east-1"
export S3_TEST_BUCKET="$BUCKET"
export S3_TEST_KEY="viridis/gray_3857.tif"

cargo test --features s3-tests s3 -- --nocapture

echo ""
echo "All S3 tests completed!"
