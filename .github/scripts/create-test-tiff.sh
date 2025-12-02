#!/bin/bash
# Creates a minimal valid GeoTIFF for S3 integration testing
# Usage: ./create-test-tiff.sh /path/to/output.tif

set -e

OUTPUT="${1:-/tmp/test.tif}"

# Check if gdal_create is available (preferred)
if command -v gdal_create &> /dev/null; then
    gdal_create -of GTiff -outsize 256 256 -bands 1 -burn 128 \
        -a_srs EPSG:3857 \
        -a_ullr -20037508.34 20037508.34 20037508.34 -20037508.34 \
        "$OUTPUT"
    echo "Created test GeoTIFF with gdal_create: $OUTPUT"
    exit 0
fi

# Check if gdal_translate is available
if command -v gdal_translate &> /dev/null; then
    # Create a simple PGM image first, then convert to GeoTIFF
    # PGM format: P5 width height maxval followed by binary data
    TMP_PGM="/tmp/test_$$.pgm"
    echo -e "P5\n256 256\n255" > "$TMP_PGM"
    # Fill with gradient data (256x256 bytes)
    for i in $(seq 0 255); do
        printf "%.0s\x80" $(seq 1 256) >> "$TMP_PGM"
    done

    gdal_translate -of GTiff -a_srs EPSG:3857 \
        -a_ullr -20037508.34 20037508.34 20037508.34 -20037508.34 \
        "$TMP_PGM" "$OUTPUT"
    rm -f "$TMP_PGM"
    echo "Created test GeoTIFF with gdal_translate: $OUTPUT"
    exit 0
fi

# Fallback: Create minimal TIFF header manually
# This creates a very basic TIFF that can be read but may not have geo info
echo "Warning: GDAL not available, creating minimal TIFF header"

# Little-endian TIFF header
# Magic: "II" (0x4949) + Version: 42 (0x002A) + IFD offset: 8
printf 'II\x2a\x00\x08\x00\x00\x00' > "$OUTPUT"

# Minimal IFD with essential tags
# Number of directory entries: 8
printf '\x08\x00' >> "$OUTPUT"

# Tag 256 (ImageWidth): 256
printf '\x00\x01\x03\x00\x01\x00\x00\x00\x00\x01\x00\x00' >> "$OUTPUT"
# Tag 257 (ImageLength): 256
printf '\x01\x01\x03\x00\x01\x00\x00\x00\x00\x01\x00\x00' >> "$OUTPUT"
# Tag 258 (BitsPerSample): 8
printf '\x02\x01\x03\x00\x01\x00\x00\x00\x08\x00\x00\x00' >> "$OUTPUT"
# Tag 259 (Compression): 1 (none)
printf '\x03\x01\x03\x00\x01\x00\x00\x00\x01\x00\x00\x00' >> "$OUTPUT"
# Tag 262 (PhotometricInterpretation): 1 (min is black)
printf '\x06\x01\x03\x00\x01\x00\x00\x00\x01\x00\x00\x00' >> "$OUTPUT"
# Tag 273 (StripOffsets): offset to image data
printf '\x11\x01\x03\x00\x01\x00\x00\x00\x6e\x00\x00\x00' >> "$OUTPUT"
# Tag 278 (RowsPerStrip): 256
printf '\x16\x01\x03\x00\x01\x00\x00\x00\x00\x01\x00\x00' >> "$OUTPUT"
# Tag 279 (StripByteCounts): 65536 (256*256)
printf '\x17\x01\x03\x00\x01\x00\x00\x00\x00\x00\x01\x00' >> "$OUTPUT"

# Next IFD offset: 0 (none)
printf '\x00\x00\x00\x00' >> "$OUTPUT"

# Image data (256x256 = 65536 bytes of gray value 128)
dd if=/dev/zero bs=1 count=65536 2>/dev/null | tr '\0' '\200' >> "$OUTPUT"

echo "Created minimal test TIFF: $OUTPUT"
