# Lightweight map outline

`world-outline.json` is a simplified rendering-only copy of Natural Earth 50m
admin-0 geometry already bundled in `src-tauri/assets/geo-data.zip`.
It strips properties and simplifies rings at 0.08 degrees. It renders immediately while the local geographical index is preparing.
Once ready, the offline basemap uses cached administrative geometry from that index.
It does not replace full-resolution GPS region matching.

Natural Earth data is public domain:
https://www.naturalearthdata.com/about/terms-of-use/
