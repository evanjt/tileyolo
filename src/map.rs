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
        top: 10px; 
        left: 50px;  
        z-index: 1000; 
        background: white; 
        padding: 6px; 
        border-radius: 4px; 
        box-shadow: 0 1px 4px rgba(0,0,0,0.3); 
      }
      #map { height: 100%; width: 100%; }
    </style>
  </head>
  <body>
    <div id="controls">
      <label for="layerSelect">Layer: </label>
      <select id="layerSelect"></select>
    </div>

    <div id="map"></div>

    <script
      src="https://unpkg.com/leaflet@1.9.4/dist/leaflet.js"
      integrity="sha256-20nQCchB9co0qIjJZRGuk2/Z9VM+kNiyxNV1lvTlZBo="
      crossorigin=""
    ></script>
    
    <script>
      const layerSelect = document.getElementById('layerSelect');

      // initialize map
      const map = L.map('map').setView([0, 0], 2);

      let tileLayer;

      async function initLayers() {
        // fetch available layers
        const res = await fetch('/layers');
        const data = await res.json();  // Structure of JSON: [{ layer, style }, …]

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
        tileLayer = L.tileLayer(`/tiles/${first}/{z}/{x}/{y}`, {
          maxZoom: 18,
          tileSize: 256,
        }).addTo(map);
      }

      layerSelect.addEventListener('change', () => {
        const newLayer = layerSelect.value;
        map.removeLayer(tileLayer);
        tileLayer = L.tileLayer(`/tiles/${newLayer}/{z}/{x}/{y}`, {
          maxZoom: 18,
          tileSize: 256,
        }).addTo(map);
      });

      // run on load
      initLayers().catch(console.error);
    </script>
  </body>
  </html>
"#;
