pub(super) const INDEX_HTML: &str = r#"<!DOCTYPE html>
  <html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0"/>
    <title>TileYolo</title>
    <link
      rel="stylesheet"
      href="https://unpkg.com/leaflet@1.9.4/dist/leaflet.css"
      integrity="sha256-p4NxAoJBhIIN+hmNHrzRCf9tD/miZyoHS5obTRR9BMY="
      crossorigin=""
    />
    <style>
      html, body { height: 100%; margin: 0; padding: 0; }
      #controls {
        position: absolute;
        top: 12px;
        left: 50px;
        z-index: 1000;
        background: white;
        padding: 6px;
        border-radius: 4px;
        box-shadow: 0 1px 4px rgba(0,0,0,0.3);
        height: 80px;
        line-height: 26px;
      }
      #map { height: 100%; width: 100%; }
      /* Force nearest-neighbor rendering on tile images to prevent blurry interpolation
         during zoom transitions. This is especially important for sparse data where
         bilinear interpolation makes pixels appear to "scrunch" during zoom. */
      .leaflet-tile-container img {
        image-rendering: pixelated;
        image-rendering: -moz-crisp-edges;
        image-rendering: crisp-edges;
      }
    .leaflet-control-zoom .leaflet-control-zoom-to-extent {
      display: block;
      background-color: #fff;
      border-bottom: 1px solid #ccc;
      width: 26px;
      height: 26px;
      line-height: 26px;
      text-align: center;
      text-decoration: none;
      color: #333;
      font: bold 18px 'Lucida Console', Monaco, monospace;
      text-indent: 1px;
    }

    .leaflet-control-zoom .leaflet-control-zoom-to-extent:hover {
      background-color: #f4f4f4;
    }
  </style>
  </head>
  <body>
    <div id="controls">
      <label for="layerSelect">Layer: </label>
      <select id="layerSelect"></select>
      <br />
      <label for="opacitySlider">Opacity: </label>
      <input type="range" id="opacitySlider" min="0" max="1" step="0.1" value="1" />
      <br />
      <label>
        <input type="checkbox" id="osmToggle" />
        Show OSM Basemap
      </label>
      <label style="margin-left: 12px;">
        <input type="checkbox" id="showUnoptimised" />
        Show unoptimised
      </label>
    </div>

    <div id="map"></div>

    <script
      src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"
      integrity="sha256-20nQCchB9co0qIjJZRGuk2/Z9VM+kNiyxNV1lvTlZBo="
      crossorigin=""
    ></script>

    <script>
      const layerSelect = document.getElementById('layerSelect');
      const osmToggle = document.getElementById('osmToggle');
      const opacitySlider = document.getElementById('opacitySlider');
      const showUnoptimised = document.getElementById('showUnoptimised');

      // initialize map
      const map = L.map('map').setView([0, 0], 2);

      let tileLayer;
      let osmLayer;
      let layersData = [];
      let currentLayerGeometry = null;

      // Add OSM basemap layer
      osmLayer = L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
        maxZoom: 19,
        attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors'
      });

      osmLayer.setZIndex(0); // Ensure OSM layer is always at the bottom

      async function initLayers() {
        // fetch available layers
        const res = await fetch('/layers');
        const data = await res.json();  // Structure of JSON: [{ layer, style, geometry, is_tiled }, …]
        layersData = data;
        populateLayerSelect();
      }

      function populateLayerSelect() {
        const includeUnoptimised = showUnoptimised.checked;

        // Filter layers based on checkbox
        const filteredLayers = layersData.filter(layer =>
          includeUnoptimised || layer.is_tiled
        );

        // populate <select> - use original index as value
        layerSelect.innerHTML = '';
        layersData.forEach((layerData, index) => {
          // Skip unoptimised layers if checkbox is unchecked
          if (!includeUnoptimised && !layerData.is_tiled) return;

          const opt = document.createElement('option');
          opt.value = index;  // Use original index to uniquely identify layer+style combo
          const tiledBadge = layerData.is_tiled ? '' : ' [slow]';
          opt.textContent = `${layerData.layer} (${layerData.style})${tiledBadge}`;
          layerSelect.appendChild(opt);
        });

        // add first available layer to map
        if (layerSelect.options.length > 0) {
          const firstIndex = parseInt(layerSelect.value);
          const firstLayerData = layersData[firstIndex];
          addLayerToMap(firstLayerData.layer, firstLayerData.style, firstLayerData.source_geometry);
        }
      }

      function addLayerToMap(layer, style, geometry) {
        if (tileLayer) {
          map.removeLayer(tileLayer);
        }

        // Build tile URL with style query parameter
        const tileUrl = `/tiles/${layer}/{z}/{x}/{y}?style=${encodeURIComponent(style)}`;

        tileLayer = L.tileLayer(tileUrl, {
          maxZoom: 18,
          tileSize: 256,
          opacity: parseFloat(opacitySlider.value), // Set initial opacity
        }).addTo(map);

        tileLayer.setZIndex(1); // Ensure the layer is above the OSM basemap

        // Store the current layer's geometry
        currentLayerGeometry = geometry;

        // Zoom to extent
        zoomToLayerExtent(geometry);
      }

      function zoomToLayerExtent(geometry) {
        if (!geometry) return;

        // source_geometry has { crs_code, extent: { minx, miny, maxx, maxy } }
        const { extent, crs_code } = geometry;

        // Convert from source CRS to lat/lon for Leaflet
        // For now, assume extent is in lon/lat (4326) or close to it
        // Web Mercator (3857) needs conversion
        let minLat, minLon, maxLat, maxLon;

        if (crs_code === 3857) {
          // Convert Web Mercator to lat/lon
          const mercatorToLatLon = (x, y) => {
            const lon = (x / 20037508.342789244) * 180;
            let lat = (y / 20037508.342789244) * 180;
            lat = (180 / Math.PI) * (2 * Math.atan(Math.exp(lat * Math.PI / 180)) - Math.PI / 2);
            return [lat, lon];
          };
          [minLat, minLon] = mercatorToLatLon(extent.minx, extent.miny);
          [maxLat, maxLon] = mercatorToLatLon(extent.maxx, extent.maxy);
        } else {
          // Assume 4326 (lat/lon)
          minLat = extent.miny;
          minLon = extent.minx;
          maxLat = extent.maxy;
          maxLon = extent.maxx;
        }

        const bounds = [
          [minLat, minLon],
          [maxLat, maxLon]
        ];

        map.fitBounds(bounds);
      }

      osmToggle.addEventListener('change', () => {
        if (osmToggle.checked) {
          map.addLayer(osmLayer);
        } else {
          map.removeLayer(osmLayer);
        }
      });

      showUnoptimised.addEventListener('change', () => {
        // Re-populate the layer select with filtered layers
        populateLayerSelect();
      });

      layerSelect.addEventListener('change', () => {
        const selectedIndex = parseInt(layerSelect.value);
        const selectedLayerData = layersData[selectedIndex];
        addLayerToMap(selectedLayerData.layer, selectedLayerData.style, selectedLayerData.source_geometry);
      });

      opacitySlider.addEventListener('input', () => {
        if (tileLayer) {
          tileLayer.setOpacity(parseFloat(opacitySlider.value));
        }
      });

      // Add extent button to the zoom control
      const zoomControl = map.zoomControl;
      const zoomToExtentButton = L.DomUtil.create(
      'a',
      'leaflet-control-zoom-to-extent',
      zoomControl._container
    );
    zoomToExtentButton.innerHTML = '⤢';
    zoomToExtentButton.href = '#';
    zoomToExtentButton.title = 'Zoom to Extent';

    L.DomEvent.on(zoomToExtentButton, 'click', e => {
      L.DomEvent.preventDefault(e);
      if (currentLayerGeometry) {
        zoomToLayerExtent(currentLayerGeometry);
      }
    });
    initLayers().catch(console.error);
  </script>
  </body>
  </html>
"#;
