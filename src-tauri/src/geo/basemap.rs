//! Offline display geometry. Reuses the existing index without changing GPS lookup shapes.
//! Only visible administrative regions are prepared; simplified display features are cached.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use geo::{LineString, SimplifyVwPreserve};
use serde_json::{json, Value};

use super::{GeoIndex, RegionNode};

struct DisplayRegion {
    area: Value,
    label: Value,
}

struct DisplayCache {
    index: Arc<GeoIndex>,
    reachable: Vec<bool>,
    regions: HashMap<usize, DisplayRegion>,
}

fn ring_coordinates(ring: &LineString<f64>) -> Vec<[f64; 2]> {
    ring.0.iter().map(|c| [c.x, c.y]).collect()
}

fn display_region(id: usize, node: &RegionNode) -> DisplayRegion {
    // VW epsilon is an area in square degrees. Fine layers retain more detail.
    let epsilon = [0.00001, 0.000001, 0.0000001, 0.00000001][node.level as usize];
    let coordinates: Vec<Vec<Vec<[f64; 2]>>> = node
        .shape
        .iter()
        .map(|polygon| {
            let polygon = polygon.simplify_vw_preserve(&epsilon);
            let mut rings = vec![ring_coordinates(polygon.exterior())];
            rings.extend(polygon.interiors().iter().map(ring_coordinates));
            rings
        })
        .collect();
    let properties = json!({ "name": node.name, "level": node.level });
    DisplayRegion {
        area: json!({
            "type": "Feature", "id": id, "properties": properties,
            "geometry": { "type": "MultiPolygon", "coordinates": coordinates }
        }),
        label: json!({
            "type": "Feature", "id": id, "properties": properties,
            "geometry": { "type": "Point", "coordinates": [node.lon, node.lat] }
        }),
    }
}

fn intersects(node: &RegionNode, bounds: [f64; 4]) -> bool {
    let (min_lat, min_lon, max_lat, max_lon) = node.bbox;
    if min_lat > bounds[3] || max_lat < bounds[1] {
        return false;
    }
    // Bounds may span repeated worlds or the antimeridian (e.g. 170..190).
    if bounds[2] - bounds[0] >= 360.0 {
        return true;
    }
    let west = (bounds[0] + 180.0).rem_euclid(360.0) - 180.0;
    let east = west + bounds[2] - bounds[0];
    (min_lon <= east && max_lon >= west) || (east > 180.0 && min_lon <= east - 360.0)
}

/// level 0 returns the whole world; finer layers return only the padded viewport.
/// One cache is shared by windows and replaced when the geographical index changes.
pub fn viewport(index: Arc<GeoIndex>, level: u8, bounds: [f64; 4]) -> Result<Value, String> {
    if level > 3
        || !bounds.iter().all(|v| v.is_finite())
        || bounds[0] > bounds[2]
        || bounds[1] > bounds[3]
    {
        return Err("无效的地图范围或层级".into());
    }
    static CACHE: OnceLock<Mutex<Option<DisplayCache>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if cache
        .as_ref()
        .is_none_or(|c| !Arc::ptr_eq(&c.index, &index))
    {
        *cache = Some(DisplayCache {
            reachable: index.reachable(),
            index: Arc::clone(&index),
            regions: HashMap::new(),
        });
    }
    let cache = cache.as_mut().expect("display cache initialized");
    let mut areas = Vec::new();
    let mut labels = Vec::new();
    for (id, node) in index.nodes.iter().enumerate() {
        let selected = if level == 0 {
            node.level == 0
        } else {
            node.level > 0 && node.level <= level && intersects(node, bounds)
        };
        if !selected || !cache.reachable[id] || node.shape.is_empty() {
            continue;
        }
        let region = cache
            .regions
            .entry(id)
            .or_insert_with(|| display_region(id, node));
        areas.push(region.area.clone());
        labels.push(region.label.clone());
    }
    Ok(json!({
        "areas": { "type": "FeatureCollection", "features": areas },
        "labels": { "type": "FeatureCollection", "features": labels }
    }))
}
