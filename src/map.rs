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
        const data = await res.json();  // Structure of JSON: [{ layer, style, geometry }, …]
        layersData = data;

        // populate <select>
        layerSelect.innerHTML = '';
        data.forEach(({ layer, style }) => {
          const opt = document.createElement('option');
          opt.value = layer;
          opt.textContent = `${layer} (${style})`; // Display as "layer (style)"
          layerSelect.appendChild(opt);
        });

        // add first layer to map
        const first = layerSelect.value;
        const firstLayerData = data.find(d => d.layer === first);
        addLayerToMap(first, firstLayerData.geometry);
      }

      function addLayerToMap(layer, geometry) {
        if (tileLayer) {
          map.removeLayer(tileLayer);
        }

        tileLayer = L.tileLayer(`/tiles/${layer}/{z}/{x}/{y}`, {
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
        if (geometry && geometry["4326"]) {
          const extent = geometry["4326"].extent; // [minX, minY, maxX, maxY]
          const bounds = [
            [extent[1], extent[0]], // [minY, minX]
            [extent[3], extent[2]]  // [maxY, maxX]
          ];
          map.fitBounds(bounds);
        }
      }

      osmToggle.addEventListener('change', () => {
        if (osmToggle.checked) {
          map.addLayer(osmLayer);
        } else {
          map.removeLayer(osmLayer);
        }
      });

      layerSelect.addEventListener('change', () => {
        const newLayer = layerSelect.value;
        const selectedLayerData = layersData.find(d => d.layer === newLayer);
        addLayerToMap(newLayer, selectedLayerData.geometry);
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
