# TileYolo

**Serve GeoTIFFs as an XYZ tile API with zero configuration.**

Drop your TIFFs into subfolders by style and run `tileyolo` — it handles everything else.

## Install

```bash
cargo install tileyolo
```

Or use as a library:

```bash
cargo add tileyolo
```

## Usage

1. `cd` into your data directory (parent of style subfolders).
2. Run:
   ```bash
   tileyolo
   ```
3. Point your XYZ-capable client (browser, QGIS, Leaflet, etc.) at:
   ```text
   http://localhost:8000/tiles/{layer}/{z}/{x}/{y}
   ```
   - **`{layer}`** is the subfolder name or TIFF filename (without extension).

TileYolo will auto-detect styles (`style.txt` or built-in palettes), handle no-data values, and serve tiles on port 8000.

## Styles & Folder Structure

Organize your GeoTIFFs into style-specific subfolders:

```text
./data/
├── default/
│   ├── layer1.tif
│   ├── layer2.tif
│   └── style.txt    # QGIS-exported colour stops
├── viridis/
│   ├── layer3.tif
│   └── layer4.tif   # uses built-in viridis palette
└── grayscale/
    └── layer5.tif   # no style.txt → linear grayscale
```

- **Custom styles**: Folders with a `style.txt` (QGIS export) use those exact colour stops.
- **Built-in palettes**: Folders named `viridis`, `magma`, `plasma`, `inferno`, `turbo`, `cubehelix_default`, `rainbow`, `spectral`, or `sinebow` apply the corresponding gradient.
- **Grayscale fallback**: Other folders without any style file render in linear grayscale.

## Sample Output

```text
$ tileyolo
✅ All files loaded!
[████████████████████████████████████████] 28/28 100%
Style summary:
     Style    Layers  Breaks                        Min        Max       Colourbar
⚠   default  27      0,100,200,300,400             -80.59     22613972   █████
     viridis  1       auto                          0.00       746.10    ██████████

Warnings:
  ⚠ default: Colour stops [0.00…400.00] do NOT cover data range [-80.59…22613972.00]
  
📦 Total layers: 28
🚀 Serving tiles at http://0.0.0.0:8000
```

(_More colourful in a true ANSI terminal._)

## Configuration

TileYolo has minimal options:

```bash
$ tileyolo --help
Usage: tileyolo [OPTIONS]

Options:
  --data-folder <DIR>  Path to data folder [default: /home/.../data]
  -h, --help           Print help
  -V, --version        Print version
```

## QGIS `style.txt` Example

```text
# QGIS Generated Colour Map Export File
INTERPOLATION:INTERPOLATED
0,215,25,28,255,0
100,253,174,97,255,100
200,255,255,191,255,200
300,171,221,164,255,300
400,43,131,186,255,400
```

See [QGIS Raster Properties → Symbology → Colour Ramp](https://docs.qgis.org/3.40/en/docs/user_manual/working_with_raster/raster_properties.html#id13) for export instructions.

## Why TileYolo?

I needed a zero‑config, lightweight XYZ tile server for GeoTIFFs. TileYolo does just that: drag, drop, and go.

## Roadmap

- Speed up startup with caching
- Tile caching for performance
- S3 and COG support
- Additional built-in palettes
- Contributions welcome

## Caveats

- Only **band 1** is served
- Output CRS is **EPSG:3857** (Web Mercator)
- Input TIFFs must define a CRS
- Tested on small to medium rasters; large rasters may require more resources
