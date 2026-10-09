//! 拍摄地图地理模块：行政区划树索引（GPS → 国家/省/市/县）。
//!
//! 设计定案（2026-09-29，docs/plans/map-module.md）：
//! - **树形独立索引**：`regions`（库内树表，元数据单份存储）+ `asset_regions`
//!   （每资产每层一行挂接）——地图功能只消费索引，`assets.gps_*` 是源头。
//! - **数据包**（不进安装包，按需下载到 `dbDir/geo/`）：
//!   世界国界/省界 = Natural Earth 50m（public domain，全球单文件）；
//!   中国省市县 = 阿里 DataV（中文名 + 自带 center，精度最好）。
//!   层级最深到县（level 3），无更深开关，街道明确不做。
//! - **点解析**：R-tree 只建国家层（~250 节点），命中后沿树逐层 narrowing
//!   （子节点 envelope 预筛 + polygon contains 精判）。
//! - **树缓存 JSON**：回填入库后从库导出 `regions-cache.json`（前端直读，
//!   不走 IPC 递归查询）；指纹 = 数据包文件指纹，不符即重建。

pub mod backfill;
pub mod install;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use geo::{Centroid, Contains, Coord, EuclideanDistance, MultiPolygon, Point, Polygon};
use rstar::{RTree, RTreeObject, AABB};

/// 地理数据目录（应用配置目录下，与 models 同级——静态数据全库共享；
/// 2026-09-30 起由内置包解压而来，见 install.rs）。
pub fn geo_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("geo")
}

/// 数据包清单：世界（Natural Earth 50m，public domain）+ 中国（DataV 递归）。
/// NE 的 NAME_ZH 带 中文名（缺省回退 ADMIN/NAME）；LABEL_X/LABEL_Y 是现成的
/// 标签锚点（比多边形 centroid 更贴近「视觉中心」，海岸国家不落海里）。
pub struct PackageSpec {
    pub id: &'static str,
    pub url: &'static str,
    pub file: &'static str,
}

pub const PACKAGE_WORLD_ADM0: PackageSpec = PackageSpec {
    id: "world-adm0",
    url: "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_0_countries.geojson",
    file: "world-adm0.geojson",
};
pub const PACKAGE_WORLD_ADM1: PackageSpec = PackageSpec {
    id: "world-adm1",
    url: "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_50m_admin_1_states_provinces.geojson",
    file: "world-adm1.geojson",
};
/// DataV 根（中国省级全量；市/县由下载器按 adcode 递归拉取）。
pub const DATAV_BASE: &str = "https://geo.datav.aliyun.com/areas_v3/bound";
pub const DATAV_CHINA_CODE: &str = "100000";
/// NE 中国节点 code（DataV 子树挂接锚点）。
pub const CHINA_ISO3: &str = "CHN";

// ---------------------------------------------------------------------------
// 树模型（Arena：节点 Vec + children 索引）
// ---------------------------------------------------------------------------

/// 树节点。多边形留在内存（点包含用）；库表与缓存 JSON 只落元数据。
/// serde 派生供 bincode 树缓存（geo-index.bin：指纹不变时免解析 400 文件）。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RegionNode {
    pub level: u8, // 0 国家 / 1 省·州 / 2 市 / 3 县·区
    pub name: String,
    pub code: String,
    pub lat: f64,
    pub lon: f64,
    pub source: String,
    pub bbox: (f64, f64, f64, f64), // (min_lat, min_lon, max_lat, max_lon)
    pub shape: Vec<Polygon<f64>>,
    pub children: Vec<usize>,
}

struct CountryEntry {
    envelope: AABB<[f64; 2]>,
    node: usize,
}

impl RTreeObject for CountryEntry {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// 解析结果：根→叶的命中路径（node = Arena 索引；库 id 恒为 node+1，
/// 由 rewrite_regions 的显式分配保证同构）。
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRegion {
    pub node: usize,
    pub level: u8,
    pub code: String,
    pub name: String,
}

/// 已加载的地理索引（进程内常驻，回填/编辑联动/状态查询共用）。
pub struct GeoIndex {
    pub nodes: Vec<RegionNode>,
    roots: Vec<usize>,
    countries: RTree<CountryEntry>,
    /// 数据包指纹（文件名+长度+mtime 串）；存 geo/fingerprint.txt，变化即重刷。
    pub fingerprint: String,
}

/// GeoIndex 的可序列化形态（bincode 缓存 geo-index.bin；国家 RTree 量小
/// 不序列化，加载后现建）。
#[derive(serde::Serialize, serde::Deserialize)]
struct GeoIndexData {
    nodes: Vec<RegionNode>,
    roots: Vec<usize>,
    fingerprint: String,
}

impl GeoIndex {
    /// 从根可达的节点标记（DataV 替换中国子树后，被摘除的 NE 省是 Arena 孤儿：
    /// 节点仍在 Vec 里（children 索引不能重排），但入库/缓存只写真子树）。
    pub fn reachable(&self) -> Vec<bool> {
        let mut seen = vec![false; self.nodes.len()];
        let mut stack = self.roots.clone();
        while let Some(i) = stack.pop() {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            stack.extend(self.nodes[i].children.iter().copied());
        }
        seen
    }

    /// 点解析：国家 R-tree 预筛 → contains 精判 → 沿树逐层 narrowing。
    /// 返回根→叶路径；无命中（远海/未覆盖区）返回空。
    /// 近岸兜底：50m 数据海岸线粗糙（纽约/沿海城市常落在多边形外），
    /// 预筛 envelope 外扩 1°（约百公里），无 contains 命中时吸附最近多边形。
    pub fn resolve(&self, lat: f64, lon: f64) -> Vec<ResolvedRegion> {
        const SNAP_DEG: f64 = 1.0;
        let point = Point::new(lon, lat); // geo 约定 x=lon y=lat
        let query = AABB::from_corners(
            [lon - SNAP_DEG, lat - SNAP_DEG],
            [lon + SNAP_DEG, lat + SNAP_DEG],
        );
        let mut path: Vec<usize> = Vec::new();
        let mut nearest: Option<(f64, usize)> = None;
        for entry in self.countries.locate_in_envelope_intersecting(&query) {
            let country = &self.nodes[entry.node];
            if polygons_contain(&country.shape, &point) {
                path.push(entry.node);
                break; // 国家互斥（边界重叠取首个命中）
            }
            let d = min_polygon_distance(&country.shape, &point);
            if nearest.map_or(true, |(bd, _)| d < bd) {
                nearest = Some((d, entry.node));
            }
        }
        if path.is_empty() {
            if let Some((_, node)) = nearest.filter(|(d, _)| *d <= SNAP_DEG) {
                path.push(node);
            }
        }
        if path.is_empty() {
            return Vec::new();
        }
        // 逐层下钻：children envelope 预筛 + contains；一层最多取一个命中
        loop {
            let current = *path.last().expect("path 非空");
            let next = self.nodes[current]
                .children
                .iter()
                .find(|&&child| {
                    let node = &self.nodes[child];
                    bbox_contains(node.bbox, lat, lon) && polygons_contain(&node.shape, &point)
                })
                .copied();
            match next {
                Some(child) => path.push(child),
                None => break,
            }
        }
        path.iter()
            .map(|&i| {
                let n = &self.nodes[i];
                ResolvedRegion {
                    node: i,
                    level: n.level,
                    code: n.code.clone(),
                    name: n.name.clone(),
                }
            })
            .collect()
    }
}

fn bbox_contains(bbox: (f64, f64, f64, f64), lat: f64, lon: f64) -> bool {
    lat >= bbox.0 && lat <= bbox.2 && lon >= bbox.1 && lon <= bbox.3
}

fn polygons_contain(shape: &[Polygon<f64>], point: &Point) -> bool {
    shape.iter().any(|poly| poly.contains(point))
}

/// 点到多边形组的最小欧氏距离（度；近岸吸附用，量级精度足够）。
fn min_polygon_distance(shape: &[Polygon<f64>], point: &Point) -> f64 {
    shape
        .iter()
        .map(|poly| point.euclidean_distance(poly))
        .fold(f64::INFINITY, f64::min)
}

// ---------------------------------------------------------------------------
// GeoJSON 解析（NE + DataV 两族；自定义结构最小化内存）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct RawFeature {
    properties: serde_json::Value,
    geometry: Option<RawGeometry>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
enum RawGeometry {
    Polygon {
        coordinates: Vec<Vec<Vec<f64>>>,
    },
    MultiPolygon {
        coordinates: Vec<Vec<Vec<Vec<f64>>>>,
    },
}

#[derive(serde::Deserialize)]
struct RawCollection {
    features: Vec<RawFeature>,
}

fn parse_collection(reader: impl std::io::Read) -> Result<RawCollection, String> {
    serde_json::from_reader(reader).map_err(|e| format!("GeoJSON 解析失败: {e}"))
}

fn polygons_of(geometry: &Option<RawGeometry>) -> Vec<Polygon<f64>> {
    match geometry {
        Some(RawGeometry::Polygon { coordinates }) => vec![to_polygon(coordinates)],
        Some(RawGeometry::MultiPolygon { coordinates }) => {
            coordinates.iter().map(|poly| to_polygon(poly)).collect()
        }
        None => Vec::new(),
    }
}

fn to_polygon(rings: &[Vec<Vec<f64>>]) -> Polygon<f64> {
    let exterior: Vec<Coord> = rings
        .first()
        .map(|ring| {
            ring.iter()
                .map(|c| Coord {
                    x: *c.first().unwrap_or(&0.0),
                    y: *c.get(1).unwrap_or(&0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let interiors: Vec<Vec<Coord>> = rings[1..]
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|c| Coord {
                    x: *c.first().unwrap_or(&0.0),
                    y: *c.get(1).unwrap_or(&0.0),
                })
                .collect()
        })
        .collect();
    Polygon::new(
        geo::LineString::new(exterior),
        interiors.into_iter().map(geo::LineString::new).collect(),
    )
}

fn bbox_of(shape: &[Polygon<f64>]) -> (f64, f64, f64, f64) {
    let mut bbox = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for poly in shape {
        for coord in poly.exterior().coords() {
            bbox.0 = bbox.0.min(coord.y);
            bbox.1 = bbox.1.min(coord.x);
            bbox.2 = bbox.2.max(coord.y);
            bbox.3 = bbox.3.max(coord.x);
        }
    }
    bbox
}

fn centroid_of(shape: &[Polygon<f64>]) -> Option<(f64, f64)> {
    match shape.len() {
        0 => None,
        1 => Polygon::centroid(&shape[0]).map(|p| (p.y(), p.x())),
        _ => MultiPolygon::new(shape.to_vec())
            .centroid()
            .map(|p| (p.y(), p.x())),
    }
}

/// properties 候选字段取第一个非空字符串（NE 中文/英文字段族 + DataV）。
fn prop_str(props: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        props
            .get(*k)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}

fn prop_f64(props: &serde_json::Value, key: &str) -> Option<f64> {
    props.get(key).and_then(|v| v.as_f64())
}

/// adcode 类编码字段：DataV 是 JSON 数字、NE 是字符串——两者兼容。
pub(crate) fn prop_code(props: &serde_json::Value, key: &str) -> Option<String> {
    let v = props.get(key)?;
    v.as_str()
        .map(str::to_owned)
        .or_else(|| v.as_i64().map(|n| n.to_string()))
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// 包加载：目录 → GeoIndex
// ---------------------------------------------------------------------------

/// bincode 树缓存文件（指纹不变时免解析 400 文件；2026-09-30 提速：
/// 全量解析 debug ~60s / release 数秒 → bincode 反序列化 ~1s）。
pub const INDEX_CACHE_FILE: &str = "geo-index.bin";

/// 并行预解析 DataV 文件族（解析占加载大头；按物理核分块）。
/// progress 按文件计数回调；坏文件跳过（树装配时缺文件按需报错）。
fn parse_datav_parallel(
    dir: &Path,
    progress: &(dyn Fn(u32, u32) + Sync),
) -> HashMap<String, RawCollection> {
    use std::sync::atomic::{AtomicU32, Ordering};
    let mut paths: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(code) = name
                .strip_prefix("datav-")
                .and_then(|n| n.strip_suffix(".json"))
            {
                paths.push((code.to_string(), e.path()));
            }
        }
    }
    let total = paths.len() as u32;
    let done_counter = AtomicU32::new(0);
    let done = &done_counter;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let chunk_size = (paths.len() / threads).max(1);
    let mut out: HashMap<String, RawCollection> = HashMap::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut local: HashMap<String, RawCollection> = HashMap::new();
                    for (code, path) in chunk {
                        if let Ok(file) = std::fs::File::open(path) {
                            if let Ok(collection) = parse_collection(file) {
                                local.insert(code.clone(), collection);
                            }
                        }
                        let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                        progress(n, total);
                    }
                    local
                })
            })
            .collect();
        for handle in handles {
            for (k, v) in handle.join().unwrap_or_default() {
                out.insert(k, v);
            }
        }
    });
    out
}

impl GeoIndex {
    fn build_countries(nodes: &[RegionNode], roots: &[usize]) -> RTree<CountryEntry> {
        RTree::bulk_load(
            roots
                .iter()
                .map(|&i| CountryEntry {
                    envelope: AABB::from_corners(
                        [nodes[i].bbox.1, nodes[i].bbox.0],
                        [nodes[i].bbox.3, nodes[i].bbox.2],
                    ),
                    node: i,
                })
                .collect(),
        )
    }

    fn from_data(data: GeoIndexData) -> GeoIndex {
        let countries = Self::build_countries(&data.nodes, &data.roots);
        GeoIndex {
            nodes: data.nodes,
            roots: data.roots,
            countries,
            fingerprint: data.fingerprint,
        }
    }

    /// 从 geo 目录加载全部就绪的数据包。世界两包必须齐；中国包可选
    /// （未装则中国照片下钻到 NE 英文省名，属可接受降级）。
    pub fn load(dir: &Path) -> Result<GeoIndex, String> {
        Self::load_reporting(dir, &|_, _| {})
    }

    /// 带解析进度的加载（progress = 已解析文件数/总数；仅全量解析路径回调）。
    /// 指纹与上次一致且有 bincode 缓存 → 直接反序列化（跳过全部解析）。
    pub fn load_reporting(
        dir: &Path,
        progress: &(dyn Fn(u32, u32) + Sync),
    ) -> Result<GeoIndex, String> {
        let fingerprint = fingerprint_of_dir(dir);
        if std::fs::read_to_string(dir.join(backfill::FINGERPRINT_FILE))
            .ok()
            .as_deref()
            == Some(fingerprint.as_str())
        {
            if let Ok(bytes) = std::fs::read(dir.join(INDEX_CACHE_FILE)) {
                if let Ok(data) = bincode::deserialize::<GeoIndexData>(&bytes) {
                    progress(1, 1);
                    return Ok(Self::from_data(data));
                }
            }
        }
        // DataV 文件族并行预解析（progress 由此路汇报）
        let parsed = parse_datav_parallel(dir, progress);
        let mut nodes: Vec<RegionNode> = Vec::new();
        let mut roots: Vec<usize> = Vec::new();
        let mut code_to_idx: HashMap<(u8, String), usize> = HashMap::new();

        // 1) 世界国家层（NE admin0）
        let adm0_path = dir.join(PACKAGE_WORLD_ADM0.file);
        let adm0: RawCollection = parse_collection(
            std::fs::File::open(&adm0_path)
                .map_err(|e| format!("世界国界包缺失（{}）: {e}", adm0_path.display()))?,
        )?;
        for feature in &adm0.features {
            let shape = polygons_of(&feature.geometry);
            if shape.is_empty() {
                continue;
            }
            let props = &feature.properties;
            let name = prop_str(props, &["NAME_ZH", "ADMIN", "NAME", "name"])
                .unwrap_or_else(|| "未知地区".into());
            let code = prop_str(props, &["ADM0_A3", "ISO_A3", "ISO_A3_EH"])
                .unwrap_or_else(|| name.clone());
            // LABEL_X/Y 是 NE 现成标签锚点；缺省回退多边形 centroid
            let (lat, lon) = match (prop_f64(props, "LABEL_Y"), prop_f64(props, "LABEL_X")) {
                (Some(y), Some(x)) if x != 0.0 || y != 0.0 => (y, x),
                _ => centroid_of(&shape).unwrap_or((0.0, 0.0)),
            };
            let idx = nodes.len();
            nodes.push(RegionNode {
                level: 0,
                name,
                code: code.clone(),
                lat,
                lon,
                source: "ne50m".into(),
                bbox: bbox_of(&shape),
                shape,
                children: Vec::new(),
            });
            code_to_idx.insert((0, code), idx);
            roots.push(idx);
        }

        // 2) 世界省/州层（NE admin1）：按 ADM0_A3 挂国家
        let adm1_path = dir.join(PACKAGE_WORLD_ADM1.file);
        if adm1_path.is_file() {
            let adm1: RawCollection =
                parse_collection(std::fs::File::open(&adm1_path).map_err(|e| e.to_string())?)?;
            for feature in &adm1.features {
                let shape = polygons_of(&feature.geometry);
                if shape.is_empty() {
                    continue;
                }
                let props = &feature.properties;
                let country_code =
                    prop_str(props, &["adm0_a3", "ADM0_A3", "iso_a2_eh"]).unwrap_or_default();
                let Some(&parent) = code_to_idx.get(&(0, country_code.clone())) else {
                    continue; // 父国家不在（数据裁剪/字段缺失）：跳过该省
                };
                let name = prop_str(props, &["name_zh", "NAME_ZH", "name", "NAME"])
                    .unwrap_or_else(|| "未知地区".into());
                let code = prop_str(props, &["iso_3166_2", "ISO_3166_2", "fips", "FIPS"])
                    .unwrap_or_else(|| format!("{country_code}-{}", nodes.len()));
                let (lat, lon) = centroid_of(&shape).unwrap_or((0.0, 0.0));
                let idx = nodes.len();
                nodes.push(RegionNode {
                    level: 1,
                    name,
                    code: code.clone(),
                    lat,
                    lon,
                    source: "ne50m".into(),
                    bbox: bbox_of(&shape),
                    shape,
                    children: Vec::new(),
                });
                code_to_idx.insert((1, code), idx);
                nodes[parent].children.push(idx);
            }
        }

        // 3) 中国包（DataV 递归文件族）：省/市/县三层；挂 NE 中国节点。
        //    DataV 全 34 省成套，就绪即整体替换 NE 的中国省级子树
        //    （中文名 + 精确边界优先，NE 中国省丢弃）。
        if parsed.contains_key(DATAV_CHINA_CODE) {
            if let Some(&china) = code_to_idx.get(&(0, CHINA_ISO3.to_string())) {
                nodes[china].children.clear();
                load_datav_tree(&parsed, DATAV_CHINA_CODE, china, &mut nodes, 0)?;
            }
        }

        // 4) 国家 R-tree（仅 level0）
        let data = GeoIndexData {
            nodes,
            roots,
            fingerprint,
        };
        // 落 bincode 缓存（best effort；fingerprint.txt 由回填管线成功后写，
        // 两者齐了下轮走快路径）
        if let Ok(bytes) = bincode::serialize(&data) {
            let _ = std::fs::write(dir.join(INDEX_CACHE_FILE), bytes);
        }
        Ok(Self::from_data(data))
    }
}

/// DataV 子树递归加载：`datav-<adcode>.json`（本层 full）→ 本层 features
/// 即子节点，逐层读文件（文件由下载器预拉齐，这里纯本地）。
/// `parent_level` = 父节点层级（根调用传 0，文件内 features 是 level1 省）。
fn load_datav_tree(
    parsed: &HashMap<String, RawCollection>,
    adcode: &str,
    parent: usize,
    nodes: &mut Vec<RegionNode>,
    parent_level: u8,
) -> Result<(), String> {
    let collection = parsed
        .get(adcode)
        .ok_or_else(|| format!("DataV 文件缺失（datav-{adcode}.json）"))?;
    for feature in &collection.features {
        let shape = polygons_of(&feature.geometry);
        let props = &feature.properties;
        let Some(code) = prop_code(props, "adcode") else {
            continue;
        };
        // 九段线等界线要素（100000_JD）不是行政区：不入树
        if !code.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        // 轮廓文件的自引用要素（_full 404 回退包只含自身，如济源市 419001
        // level 仍为 "city"）：不是子级，跳过——否则节点自嵌套且递归无限
        // （2026-09-30 栈溢出真因，树构建侧）
        if code == adcode {
            continue;
        }
        // 层级用 DataV 语义字段（直辖市下辖区县按 district 正确落 level 3）；
        // 缺失时回退计数推导
        let level = match prop_str(props, &["level"]).as_deref() {
            Some("province") => 1,
            Some("city") => 2,
            Some("district") => 3,
            _ => parent_level + 1,
        };
        let name = prop_str(props, &["name"]).unwrap_or_else(|| code.clone());
        // DataV 自带 center [lng, lat]；缺省回退 centroid
        let (lat, lon) = match props.get("center").and_then(|v| v.as_array()) {
            Some(pair) if pair.len() == 2 => (
                pair[1].as_f64().unwrap_or(0.0),
                pair[0].as_f64().unwrap_or(0.0),
            ),
            _ => {
                if shape.is_empty() {
                    (0.0, 0.0)
                } else {
                    centroid_of(&shape).unwrap_or((0.0, 0.0))
                }
            }
        };
        let bbox = if shape.is_empty() {
            (lat, lon, lat, lon) // 点级行政区（极少数）：退化为点 bbox
        } else {
            bbox_of(&shape)
        };
        let idx = nodes.len();
        nodes.push(RegionNode {
            level,
            name,
            code: code.clone(),
            lat,
            lon,
            source: "datav".into(),
            bbox,
            shape,
            children: Vec::new(),
        });
        nodes[parent].children.push(idx);
        // 市县两级继续递归（省 level1 → 市 level2 → 县 level3 即停，2026-09-29 定案）
        if level < 3 && parsed.contains_key(&code) {
            load_datav_tree(parsed, &code, idx, nodes, level)?;
        }
    }
    Ok(())
}

/// 数据包指纹：目录内文件名+长度+mtime 的稳定串（内容变化的廉价代理）。
pub fn fingerprint_of_dir(dir: &Path) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut names: Vec<(String, u64, u64)> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                if !meta.is_file() {
                    return None;
                }
                Some((
                    e.file_name().to_string_lossy().into_owned(),
                    meta.len(),
                    meta.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                ))
            })
            .collect();
        names.sort();
        for (name, len, mtime) in names {
            let _ = writeln!(out, "{name}:{len}:{mtime}");
        }
    }
    out
}

/// DataV 文件族已装数量（省+市+县全量约 375；<250 视为不完整可补全）。
pub fn datav_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("datav-"))
                .count()
        })
        .unwrap_or(0)
}

/// 三个世界包/中国根文件是否就绪（中国市县由下载器保证成套）。
pub fn packages_installed(dir: &Path) -> bool {
    dir.join(PACKAGE_WORLD_ADM0.file).is_file()
        && dir.join(PACKAGE_WORLD_ADM1.file).is_file()
        && dir.join(format!("datav-{DATAV_CHINA_CODE}.json")).is_file()
}

// ---------------------------------------------------------------------------
// 全局状态（下载/加载/回填进度；编辑联动读索引）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum GeoPhase {
    /// 数据包未解压（打开地图页/启动时自动安装，无用户动作）
    NotInstalled,
    Loading,
    Ready,
    Backfilling {
        done: u64,
        total: u64,
    },
    Failed(String),
}

pub struct GeoState {
    pub phase: GeoPhase,
    pub index: Option<Arc<GeoIndex>>,
}

static STATE: OnceLock<Mutex<GeoState>> = OnceLock::new();

pub fn geo_state() -> &'static Mutex<GeoState> {
    STATE.get_or_init(|| {
        Mutex::new(GeoState {
            phase: GeoPhase::NotInstalled,
            index: None,
        })
    })
}

/// 当前状态快照（IPC 轻查询，锁内只克隆）。
pub fn phase_snapshot() -> GeoPhase {
    geo_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .phase
        .clone()
}

/// 索引若就绪返回克隆（编辑联动单点重解析用）。
pub fn index_snapshot() -> Option<Arc<GeoIndex>> {
    geo_state()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .index
        .clone()
}

// ---------------------------------------------------------------------------
// 单元测试：点包含引擎（内嵌小 GeoJSON，不依赖网络/真实数据包）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 造一个方形 NE 风格国家 + DataV 风格省市县包目录。
    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        // 国家：0-10°N 0-10°E 方形（LABEL 锚点 5,5）
        let adm0 = r#"{"features":[
            {"properties":{"ADMIN":"Testland","NAME_ZH":"测试国","ADM0_A3":"TST","LABEL_Y":5.0,"LABEL_X":5.0},
             "geometry":{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,10],[0,10],[0,0]]]}},
            {"properties":{"ADMIN":"Farland","NAME_ZH":"远方国","ADM0_A3":"FAR","LABEL_Y":-20.0,"LABEL_X":50.0},
             "geometry":{"type":"Polygon","coordinates":[[[45,-25],[55,-25],[55,-15],[45,-15],[45,-25]]]}},
            {"properties":{"ADMIN":"China","NAME_ZH":"中国","ADM0_A3":"CHN","LABEL_Y":30.0,"LABEL_X":104.0},
             "geometry":{"type":"Polygon","coordinates":[[[100,20],[110,20],[110,40],[100,40],[100,20]]]}}
        ]}"#;
        // 省：TST 内上半 0-10°E 5-10°N
        let adm1 = r#"{"features":[
            {"properties":{"name_zh":"北省","name":"North","adm0_a3":"TST","iso_3166_2":"TST-N"},
             "geometry":{"type":"Polygon","coordinates":[[[0,5],[10,5],[10,10],[0,10],[0,5]]]}}
        ]}"#;
        // DataV 中国根风格（挂在 CHN 节点）+ 省文件 + 市文件
        let cn_root = r#"{"features":[
            {"properties":{"adcode":"110000","name":"京省","center":[104.0,30.0]},
             "geometry":{"type":"Polygon","coordinates":[[[100,25],[108,25],[108,35],[100,35],[100,25]]]}}
        ]}"#;
        let cn_prov = r#"{"features":[
            {"properties":{"adcode":"110100","name":"州市","center":[102.0,30.0]},
             "geometry":{"type":"Polygon","coordinates":[[[100,25],[104,25],[104,30],[100,30],[100,25]]]}}
        ]}"#;
        let cn_city = r#"{"features":[
            {"properties":{"adcode":"110101","name":"甲区","center":[101.0,27.0]},
             "geometry":{"type":"Polygon","coordinates":[[[100,25],[102,25],[102,27],[100,27],[100,25]]]}}
        ]}"#;
        let base = dir.path();
        let mut f = std::fs::File::create(base.join(PACKAGE_WORLD_ADM0.file)).unwrap();
        f.write_all(adm0.as_bytes()).unwrap();
        let mut f = std::fs::File::create(base.join(PACKAGE_WORLD_ADM1.file)).unwrap();
        f.write_all(adm1.as_bytes()).unwrap();
        let mut f = std::fs::File::create(base.join("datav-100000.json")).unwrap();
        f.write_all(cn_root.as_bytes()).unwrap();
        let mut f = std::fs::File::create(base.join("datav-110000.json")).unwrap();
        f.write_all(cn_prov.as_bytes()).unwrap();
        let mut f = std::fs::File::create(base.join("datav-110100.json")).unwrap();
        f.write_all(cn_city.as_bytes()).unwrap();
        dir
    }

    #[test]
    fn resolve_full_path_country_province_city_county() {
        let dir = fixture_dir();
        let index = GeoIndex::load(dir.path()).expect("加载");
        // DataV 链：中国内 (101, 26) → 京省 → 州市 → 甲区
        let path = index.resolve(26.0, 101.0);
        let codes: Vec<&str> = path.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, vec!["CHN", "110000", "110100", "110101"]);
        assert_eq!(path[0].name, "中国");
        assert_eq!(path[3].name, "甲区");
        assert_eq!(path[3].level, 3);
    }

    #[test]
    fn resolve_world_path_ne_country_and_province() {
        let dir = fixture_dir();
        let index = GeoIndex::load(dir.path()).expect("加载");
        // NE 链：测试国 (5,7) → 北省（中文优先名）
        let path = index.resolve(7.0, 5.0);
        let codes: Vec<&str> = path.iter().map(|r| r.code.as_str()).collect();
        assert_eq!(codes, vec!["TST", "TST-N"]);
        assert_eq!(path[0].name, "测试国");
        assert_eq!(path[1].name, "北省");
        // 南半部 (5,2)：国家命中、省内不命中 → 只有国家
        let path = index.resolve(2.0, 5.0);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].code, "TST");
    }

    #[test]
    fn resolve_sea_point_returns_empty() {
        let dir = fixture_dir();
        let index = GeoIndex::load(dir.path()).expect("加载");
        // 太平洋中部：无命中
        assert!(index.resolve(0.0, -140.0).is_empty());
        // 远方国（无省）：仅国家层
        let path = index.resolve(-20.0, 50.0);
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].code, "FAR");
    }

    #[test]
    fn resolve_coastal_point_snaps_to_nearest_country() {
        let dir = fixture_dir();
        let index = GeoIndex::load(dir.path()).expect("加载");
        // 测试国多边形外 0.3°（模拟纽约式沿岸点在 50m 粗糙海岸线之外）：
        // 近岸吸附到最近国（≤1°），子层不硬塞
        let path = index.resolve(5.0, -0.3);
        assert_eq!(path.len(), 1, "仅国家级（无省级命中）");
        assert_eq!(path[0].code, "TST");
        // 3° 外仍是海：不吸附
        assert!(index.resolve(5.0, -3.0).is_empty());
    }

    #[test]
    fn fingerprint_changes_with_files() {
        let dir = fixture_dir();
        let a = fingerprint_of_dir(dir.path());
        std::fs::write(dir.path().join("extra.txt"), b"x").unwrap();
        let b = fingerprint_of_dir(dir.path());
        assert_ne!(a, b, "新文件应改变指纹");
    }

    #[test]
    fn packages_installed_requires_all_roots() {
        let dir = fixture_dir();
        assert!(packages_installed(dir.path()));
        std::fs::remove_file(dir.path().join(PACKAGE_WORLD_ADM1.file)).unwrap();
        assert!(!packages_installed(dir.path()));
    }
}
