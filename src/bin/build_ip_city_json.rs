use csv::{ReaderBuilder, StringRecord};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

type DynError = Box<dyn Error + Send + Sync + 'static>;
const PROXIMITY_CELL_DEGREES: f64 = 0.5;
const PROXIMITY_SEARCH_RADIUS: i32 = 1;
const PROXIMITY_MAX_KM: f64 = 100.0;
const MAX_PROXIMITY_CANDIDATES_PER_CELL: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CityKey {
    country_code: String,
    normalized_city_name: String,
}

impl CityKey {
    fn new(country_code: &str, city_name: &str) -> Option<Self> {
        let country_code = country_code.trim().to_ascii_uppercase();
        let normalized_city_name = normalize_name(city_name);
        if country_code.is_empty() || normalized_city_name.is_empty() {
            None
        } else {
            Some(Self {
                country_code,
                normalized_city_name,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CityStateKey {
    country_code: String,
    normalized_city_name: String,
    normalized_state_name: String,
}

impl CityStateKey {
    fn new(country_code: &str, city_name: &str, state_name: &str) -> Option<Self> {
        let city_key = CityKey::new(country_code, city_name)?;
        Self::from_city_key(&city_key, &normalize_name(state_name))
    }

    fn from_city_key(city_key: &CityKey, normalized_state_name: &str) -> Option<Self> {
        if normalized_state_name.is_empty() {
            return None;
        }

        Some(Self {
            country_code: city_key.country_code.clone(),
            normalized_city_name: city_key.normalized_city_name.clone(),
            normalized_state_name: normalized_state_name.to_string(),
        })
    }
}

#[derive(Debug, Clone)]
struct CityCandidate {
    city_name: String,
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Default)]
struct ImportNeeds {
    exact_keys: HashSet<CityKey>,
    state_keys: HashSet<CityStateKey>,
    proximity_countries: HashSet<String>,
}

#[derive(Debug, Default)]
struct LoadedCityCandidates {
    exact_candidates: HashMap<CityKey, Vec<CityCandidate>>,
    state_candidates: HashMap<CityStateKey, Vec<CityCandidate>>,
    proximity_indexes: HashMap<String, CountryProximityIndex>,
    skipped_lines: u64,
}

#[derive(Debug, Default)]
struct CountryProximityIndex {
    cells: HashMap<GridCell, Vec<CityCandidate>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GridCell {
    lat_bucket: i32,
    lon_bucket: i32,
}

impl GridCell {
    #[allow(clippy::cast_possible_truncation)]
    fn from_coords(latitude: f64, longitude: f64) -> Self {
        Self {
            lat_bucket: (latitude / PROXIMITY_CELL_DEGREES).floor() as i32,
            lon_bucket: (longitude / PROXIMITY_CELL_DEGREES).floor() as i32,
        }
    }

    fn center(self) -> (f64, f64) {
        let half = PROXIMITY_CELL_DEGREES / 2.0;
        (
            (f64::from(self.lat_bucket) * PROXIMITY_CELL_DEGREES) + half,
            (f64::from(self.lon_bucket) * PROXIMITY_CELL_DEGREES) + half,
        )
    }
}

impl CountryProximityIndex {
    fn insert(&mut self, candidate: CityCandidate) {
        let cell = GridCell::from_coords(candidate.latitude, candidate.longitude);
        let bucket = self.cells.entry(cell).or_default();

        if bucket
            .iter()
            .any(|existing| existing.city_name == candidate.city_name)
        {
            return;
        }

        if bucket.len() < MAX_PROXIMITY_CANDIDATES_PER_CELL {
            bucket.push(candidate);
            return;
        }

        let (center_latitude, center_longitude) = cell.center();
        let new_score = haversine_km(
            center_latitude,
            center_longitude,
            candidate.latitude,
            candidate.longitude,
        );

        if let Some((worst_index, worst_score)) = bucket
            .iter()
            .enumerate()
            .map(|(index, existing)| {
                (
                    index,
                    haversine_km(
                        center_latitude,
                        center_longitude,
                        existing.latitude,
                        existing.longitude,
                    ),
                )
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            && new_score < worst_score
        {
            bucket[worst_index] = candidate;
        }
    }

    fn nearest(&self, latitude: f64, longitude: f64) -> Option<&CityCandidate> {
        let home = GridCell::from_coords(latitude, longitude);
        let mut best: Option<(&CityCandidate, f64)> = None;

        for lat_offset in -PROXIMITY_SEARCH_RADIUS..=PROXIMITY_SEARCH_RADIUS {
            for lon_offset in -PROXIMITY_SEARCH_RADIUS..=PROXIMITY_SEARCH_RADIUS {
                let cell = GridCell {
                    lat_bucket: home.lat_bucket + lat_offset,
                    lon_bucket: home.lon_bucket + lon_offset,
                };

                if let Some(candidates) = self.cells.get(&cell) {
                    for candidate in candidates {
                        let distance = haversine_km(
                            latitude,
                            longitude,
                            candidate.latitude,
                            candidate.longitude,
                        );
                        if distance <= PROXIMITY_MAX_KM
                            && best
                                .as_ref()
                                .is_none_or(|(_, best_distance)| distance < *best_distance)
                        {
                            best = Some((candidate, distance));
                        }
                    }
                }
            }
        }

        best.map(|(candidate, _)| candidate)
    }
}

#[derive(Debug, Clone, Copy)]
struct IpCityHeaderIndexes {
    ip_range_start: usize,
    ip_range_end: usize,
    country_code: usize,
    state1: Option<usize>,
    state2: Option<usize>,
    city: usize,
    latitude: usize,
    longitude: usize,
}

#[derive(Debug, Clone, Copy)]
struct IpCityCsvLayout {
    indexes: IpCityHeaderIndexes,
    has_headers: bool,
}

impl IpCityHeaderIndexes {
    fn from_headers(headers: &StringRecord) -> Result<Self, DynError> {
        Ok(Self {
            ip_range_start: required_header(headers, "ip_range_start")?,
            ip_range_end: required_header(headers, "ip_range_end")?,
            country_code: required_header(headers, "country_code")?,
            state1: optional_header(headers, "state1"),
            state2: optional_header(headers, "state2"),
            city: required_header(headers, "city")?,
            latitude: required_header(headers, "latitude")?,
            longitude: required_header(headers, "longitude")?,
        })
    }

    fn from_geolite2_row(row: &StringRecord) -> Result<Self, DynError> {
        if row.len() < 9 {
            return Err(invalid_input(format!(
                "expected at least 9 CSV columns for headerless geolite2-city rows, found {}",
                row.len()
            ))
            .into());
        }

        Ok(Self {
            ip_range_start: 0,
            ip_range_end: 1,
            country_code: 2,
            state1: Some(3),
            state2: Some(4),
            city: 5,
            latitude: 7,
            longitude: 8,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct OutputRecord {
    ip_range_start: String,
    ip_range_end: String,
    city_name: String,
    country_code: String,
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct MergeStats {
    total_ip_rows: u64,
    exact_city_rows: u64,
    state_city_rows: u64,
    proximity_city_rows: u64,
    passthrough_ip_rows: u64,
    skipped_ip_rows: u64,
    loaded_city_candidates: u64,
    loaded_state_candidates: u64,
}

fn main() -> Result<(), DynError> {
    let args = Args::parse(std::env::args().skip(1))?;
    let ip_city_bytes = load_source_bytes(&args.ip_city_csv)?;

    let import_needs = collect_import_needs(ip_city_bytes.as_slice())?;
    eprintln!(
        "collected {} normalized city keys, {} state-aware city keys, and {} proximity countries from IP CSV",
        import_needs.exact_keys.len(),
        import_needs.state_keys.len(),
        import_needs.proximity_countries.len()
    );

    let city_candidates =
        load_needed_city_candidates(BufReader::new(File::open(&args.cities_tsv)?), &import_needs)?;
    eprintln!(
        "loaded {} exact city candidates, {} state-aware city candidates, and {} proximity cells from {} (skipped malformed city rows={})",
        city_candidates
            .exact_candidates
            .values()
            .map(|entries| entries.len() as u64)
            .sum::<u64>(),
        city_candidates
            .state_candidates
            .values()
            .map(|entries| entries.len() as u64)
            .sum::<u64>(),
        city_candidates
            .proximity_indexes
            .values()
            .map(|index| index.cells.len() as u64)
            .sum::<u64>(),
        args.cities_tsv.display(),
        city_candidates.skipped_lines
    );

    let output_file = File::create(&args.output_json)?;
    let stats = stream_merged_json(
        ip_city_bytes.as_slice(),
        &city_candidates,
        BufWriter::new(output_file),
    )?;

    eprintln!(
        "wrote {} rows to {} (exact={}, state={}, proximity={}, passthrough={}, skipped={}, city_candidates={}, state_candidates={})",
        stats.total_ip_rows.saturating_sub(stats.skipped_ip_rows),
        args.output_json.display(),
        stats.exact_city_rows,
        stats.state_city_rows,
        stats.proximity_city_rows,
        stats.passthrough_ip_rows,
        stats.skipped_ip_rows,
        stats.loaded_city_candidates,
        stats.loaded_state_candidates,
    );

    Ok(())
}

#[derive(Debug)]
struct Args {
    ip_city_csv: String,
    cities_tsv: PathBuf,
    output_json: PathBuf,
}

impl Args {
    fn parse<I>(mut args: I) -> Result<Self, DynError>
    where
        I: Iterator<Item = String>,
    {
        let mut ip_city_csv = None;
        let mut cities_tsv = None;
        let mut output_json = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--ip-city-csv" => ip_city_csv = args.next(),
                "--cities-tsv" => cities_tsv = args.next().map(PathBuf::from),
                "--output-json" => output_json = args.next().map(PathBuf::from),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => {
                    return Err(invalid_input(format!("unexpected argument: {other}")).into());
                }
            }
        }

        let ip_city_csv = ip_city_csv.ok_or_else(|| {
            invalid_input("missing required --ip-city-csv <source> argument".to_owned())
        })?;
        let cities_tsv = cities_tsv.ok_or_else(|| {
            invalid_input("missing required --cities-tsv <path> argument".to_string())
        })?;
        let output_json = output_json.ok_or_else(|| {
            invalid_input("missing required --output-json <path> argument".to_string())
        })?;

        Ok(Self {
            ip_city_csv,
            cities_tsv,
            output_json,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --features city,build --bin build_ip_city_json -- \\\n    --ip-city-csv <local CSV or .csv.gz> \\\n    --cities-tsv <local cities.tsv> \\\n    --output-json <merged output.json>\n\nEnable native-tools to accept an HTTP(S) URL as the CSV source."
    );
}

fn load_source_bytes(source: &str) -> Result<Vec<u8>, DynError> {
    if is_url(source) {
        #[cfg(feature = "native-tools")]
        {
            let runtime = tokio::runtime::Runtime::new()?;
            let bytes = runtime.block_on(async {
                let response = reqwest::get(source).await?.error_for_status()?;
                response.bytes().await
            })?;
            decode_maybe_gzip(source, bytes.as_ref())
        }
        #[cfg(not(feature = "native-tools"))]
        Err(invalid_input("URL input requires the native-tools feature".to_owned()).into())
    } else {
        let bytes = std::fs::read(Path::new(source))?;
        decode_maybe_gzip(source, &bytes)
    }
}

fn decode_maybe_gzip(source_name: &str, bytes: &[u8]) -> Result<Vec<u8>, DynError> {
    if Path::new(source_name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
    {
        let mut decoder = GzDecoder::new(bytes);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    } else {
        Ok(bytes.to_vec())
    }
}

fn is_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

fn collect_import_needs(input: &[u8]) -> Result<ImportNeeds, DynError> {
    let layout = detect_ip_city_csv_layout(input)?;
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(layout.has_headers)
        .from_reader(input);
    let mut needs = ImportNeeds::default();

    for row in csv_reader.records() {
        let row = row?;
        let country_code = row.get(layout.indexes.country_code).unwrap_or_default();
        let city_name = row.get(layout.indexes.city).unwrap_or_default();
        if let Some(key) = CityKey::new(country_code, city_name) {
            for state_name in normalized_row_states(&row, &layout.indexes) {
                if let Some(state_key) = CityStateKey::from_city_key(&key, &state_name) {
                    needs.state_keys.insert(state_key);
                }
            }
            needs.exact_keys.insert(key);
        } else {
            let country_code = country_code.trim().to_ascii_uppercase();
            if !country_code.is_empty() {
                needs.proximity_countries.insert(country_code);
            }
        }
    }

    Ok(needs)
}

fn load_needed_city_candidates(
    reader: impl BufRead,
    needs: &ImportNeeds,
) -> Result<LoadedCityCandidates, DynError> {
    let mut loaded = LoadedCityCandidates::default();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(row) = parse_city_tsv_row(line, line_no + 1) else {
            loaded.skipped_lines += 1;
            continue;
        };
        let Some(key) = CityKey::new(&row.country_code, &row.city_name) else {
            continue;
        };

        let candidate = CityCandidate {
            city_name: row.city_name.clone(),
            latitude: row.latitude,
            longitude: row.longitude,
        };

        if needs.exact_keys.contains(&key) {
            loaded
                .exact_candidates
                .entry(key)
                .or_default()
                .push(candidate.clone());
        }

        if let Some(state_name) = row.state_name.as_deref()
            && let Some(state_key) =
                CityStateKey::new(&row.country_code, &row.city_name, state_name)
            && needs.state_keys.contains(&state_key)
        {
            loaded
                .state_candidates
                .entry(state_key)
                .or_default()
                .push(candidate.clone());
        }

        if needs.proximity_countries.contains(&row.country_code) {
            loaded
                .proximity_indexes
                .entry(row.country_code)
                .or_default()
                .insert(candidate);
        }
    }

    Ok(loaded)
}

fn stream_merged_json(
    input: &[u8],
    city_candidates: &LoadedCityCandidates,
    mut writer: impl Write,
) -> Result<MergeStats, DynError> {
    let layout = detect_ip_city_csv_layout(input)?;
    let mut csv_reader = ReaderBuilder::new()
        .has_headers(layout.has_headers)
        .from_reader(input);
    let mut stats = MergeStats {
        loaded_city_candidates: city_candidates
            .exact_candidates
            .values()
            .map(|entries| entries.len() as u64)
            .sum(),
        loaded_state_candidates: city_candidates
            .state_candidates
            .values()
            .map(|entries| entries.len() as u64)
            .sum(),
        ..MergeStats::default()
    };

    writer.write_all(b"[\n")?;
    let mut first = true;

    for row in csv_reader.records() {
        let row = row?;
        stats.total_ip_rows += 1;

        let ip_range_start = row
            .get(layout.indexes.ip_range_start)
            .unwrap_or_default()
            .trim();
        let ip_range_end = row
            .get(layout.indexes.ip_range_end)
            .unwrap_or_default()
            .trim();
        let country_code = row
            .get(layout.indexes.country_code)
            .unwrap_or_default()
            .trim();
        let state_names = normalized_row_states(&row, &layout.indexes);
        let city_name = row.get(layout.indexes.city).unwrap_or_default().trim();
        let Some(latitude) =
            parse_optional_f64(row.get(layout.indexes.latitude).unwrap_or_default())
        else {
            stats.skipped_ip_rows += 1;
            continue;
        };
        let Some(longitude) =
            parse_optional_f64(row.get(layout.indexes.longitude).unwrap_or_default())
        else {
            stats.skipped_ip_rows += 1;
            continue;
        };

        let merged = merge_row(
            MergeInput {
                ip_range_start,
                ip_range_end,
                country_code,
                ip_city_name: city_name,
                ip_state_names: &state_names,
                ip_latitude: latitude,
                ip_longitude: longitude,
            },
            city_candidates,
        );

        if merged.merged_from_exact_match {
            stats.exact_city_rows += 1;
        } else if merged.merged_from_state_match {
            stats.state_city_rows += 1;
        } else if merged.merged_from_proximity {
            stats.proximity_city_rows += 1;
        } else {
            stats.passthrough_ip_rows += 1;
        }

        if !first {
            writer.write_all(b",\n")?;
        }
        first = false;
        serde_json::to_writer(&mut writer, &merged.record)?;
    }

    writer.write_all(b"\n]\n")?;
    writer.flush()?;
    Ok(stats)
}

fn detect_ip_city_csv_layout(input: &[u8]) -> Result<IpCityCsvLayout, DynError> {
    let mut csv_reader = ReaderBuilder::new().has_headers(false).from_reader(input);
    let first_row = csv_reader
        .records()
        .next()
        .transpose()?
        .ok_or_else(|| invalid_input("IP city CSV is empty".to_string()))?;

    if let Ok(indexes) = IpCityHeaderIndexes::from_headers(&first_row) {
        return Ok(IpCityCsvLayout {
            indexes,
            has_headers: true,
        });
    }

    Ok(IpCityCsvLayout {
        indexes: IpCityHeaderIndexes::from_geolite2_row(&first_row)?,
        has_headers: false,
    })
}

struct MergeResult {
    record: OutputRecord,
    merged_from_exact_match: bool,
    merged_from_state_match: bool,
    merged_from_proximity: bool,
}

#[derive(Clone, Copy)]
struct MergeInput<'a> {
    ip_range_start: &'a str,
    ip_range_end: &'a str,
    country_code: &'a str,
    ip_city_name: &'a str,
    ip_state_names: &'a [String],
    ip_latitude: f64,
    ip_longitude: f64,
}

fn merge_row(input: MergeInput<'_>, city_candidates: &LoadedCityCandidates) -> MergeResult {
    if let Some(key) = CityKey::new(input.country_code, input.ip_city_name)
        && let Some(candidates) = city_candidates.exact_candidates.get(&key)
    {
        if candidates.len() > 1
            && let Some(best) = choose_state_candidate(
                &key,
                input.ip_state_names,
                input.ip_latitude,
                input.ip_longitude,
                city_candidates,
            )
        {
            return MergeResult {
                record: OutputRecord {
                    ip_range_start: input.ip_range_start.to_string(),
                    ip_range_end: input.ip_range_end.to_string(),
                    city_name: best.city_name.clone(),
                    country_code: input.country_code.trim().to_ascii_uppercase(),
                    latitude: best.latitude,
                    longitude: best.longitude,
                },
                merged_from_exact_match: false,
                merged_from_state_match: true,
                merged_from_proximity: false,
            };
        }

        let best = choose_nearest(input.ip_latitude, input.ip_longitude, candidates);
        return MergeResult {
            record: OutputRecord {
                ip_range_start: input.ip_range_start.to_string(),
                ip_range_end: input.ip_range_end.to_string(),
                city_name: best.city_name.clone(),
                country_code: input.country_code.trim().to_ascii_uppercase(),
                latitude: best.latitude,
                longitude: best.longitude,
            },
            merged_from_exact_match: true,
            merged_from_state_match: false,
            merged_from_proximity: false,
        };
    }

    let country_code = input.country_code.trim().to_ascii_uppercase();

    if input.ip_city_name.trim().is_empty()
        && let Some(best) = choose_proximity_candidate(
            &country_code,
            input.ip_latitude,
            input.ip_longitude,
            city_candidates,
        )
    {
        return MergeResult {
            record: OutputRecord {
                ip_range_start: input.ip_range_start.to_string(),
                ip_range_end: input.ip_range_end.to_string(),
                city_name: best.city_name.clone(),
                country_code,
                latitude: best.latitude,
                longitude: best.longitude,
            },
            merged_from_exact_match: false,
            merged_from_state_match: false,
            merged_from_proximity: true,
        };
    }

    MergeResult {
        record: OutputRecord {
            ip_range_start: input.ip_range_start.to_string(),
            ip_range_end: input.ip_range_end.to_string(),
            city_name: input.ip_city_name.trim().to_string(),
            country_code,
            latitude: input.ip_latitude,
            longitude: input.ip_longitude,
        },
        merged_from_exact_match: false,
        merged_from_state_match: false,
        merged_from_proximity: false,
    }
}

fn choose_state_candidate<'a>(
    city_key: &CityKey,
    ip_state_names: &[String],
    latitude: f64,
    longitude: f64,
    city_candidates: &'a LoadedCityCandidates,
) -> Option<&'a CityCandidate> {
    let mut best: Option<(&CityCandidate, f64)> = None;

    for state_name in ip_state_names {
        let Some(state_key) = CityStateKey::from_city_key(city_key, state_name) else {
            continue;
        };
        let Some(candidates) = city_candidates.state_candidates.get(&state_key) else {
            continue;
        };

        for candidate in candidates {
            let distance =
                haversine_km(latitude, longitude, candidate.latitude, candidate.longitude);
            if best
                .as_ref()
                .is_none_or(|(_, best_distance)| distance < *best_distance)
            {
                best = Some((candidate, distance));
            }
        }
    }

    best.map(|(candidate, _)| candidate)
}

fn choose_proximity_candidate<'a>(
    country_code: &str,
    latitude: f64,
    longitude: f64,
    city_candidates: &'a LoadedCityCandidates,
) -> Option<&'a CityCandidate> {
    let country_index = city_candidates.proximity_indexes.get(country_code)?;
    country_index.nearest(latitude, longitude)
}

fn choose_nearest(latitude: f64, longitude: f64, candidates: &[CityCandidate]) -> &CityCandidate {
    candidates
        .iter()
        .min_by(|left, right| {
            let left_distance = haversine_km(latitude, longitude, left.latitude, left.longitude);
            let right_distance = haversine_km(latitude, longitude, right.latitude, right.longitude);
            left_distance.total_cmp(&right_distance)
        })
        .expect("choose_nearest called with at least one candidate")
}

fn haversine_km(latitude_a: f64, longitude_a: f64, latitude_b: f64, longitude_b: f64) -> f64 {
    let earth_radius_km = 6_371.0_f64;
    let latitude_delta = (latitude_b - latitude_a).to_radians();
    let longitude_delta = (longitude_b - longitude_a).to_radians();
    let latitude_a = latitude_a.to_radians();
    let latitude_b = latitude_b.to_radians();

    let a = (latitude_delta / 2.0).sin().powi(2)
        + latitude_a.cos() * latitude_b.cos() * (longitude_delta / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    earth_radius_km * c
}

fn normalize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut last_was_space = true;

    for ch in name.trim().chars().flat_map(char::to_lowercase) {
        let ch = match ch {
            '\u{2018}' | '\u{2019}' | '\u{201B}' | '\u{FF07}' => '\'',
            _ => ch,
        };

        if ch.is_whitespace() {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(ch);
            last_was_space = false;
        }
    }

    if normalized.ends_with(' ') {
        normalized.pop();
    }

    normalized
}

#[derive(Debug)]
struct CityTsvRow {
    city_name: String,
    state_name: Option<String>,
    latitude: f64,
    longitude: f64,
    country_code: String,
}

fn parse_city_tsv_row(line: &str, line_no: usize) -> Result<CityTsvRow, DynError> {
    let mut parts = line.split('\t');
    let city_name = parts
        .next()
        .ok_or_else(|| invalid_input(format!("line {line_no}: missing city name")))?;
    let latitude = parts
        .next()
        .ok_or_else(|| invalid_input(format!("line {line_no}: missing latitude")))?
        .parse::<f64>()
        .map_err(|err| invalid_input(format!("line {line_no}: invalid latitude: {err}")))?;
    let longitude = parts
        .next()
        .ok_or_else(|| invalid_input(format!("line {line_no}: missing longitude")))?
        .parse::<f64>()
        .map_err(|err| invalid_input(format!("line {line_no}: invalid longitude: {err}")))?;
    let country_code = parts
        .next()
        .ok_or_else(|| invalid_input(format!("line {line_no}: missing country code")))?;
    let state_name = parts.next().and_then(|value| {
        let value = value.trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    });

    if parts.next().is_some() {
        return Err(invalid_input(format!(
            "line {line_no}: expected 4 or 5 tab-separated fields"
        ))
        .into());
    }

    Ok(CityTsvRow {
        city_name: city_name.trim().to_string(),
        state_name,
        latitude,
        longitude,
        country_code: country_code.trim().to_ascii_uppercase(),
    })
}

fn required_header(headers: &StringRecord, name: &str) -> Result<usize, DynError> {
    headers
        .iter()
        .position(|header| header == name)
        .ok_or_else(|| invalid_input(format!("missing required CSV header: {name}")).into())
}

fn optional_header(headers: &StringRecord, name: &str) -> Option<usize> {
    headers.iter().position(|header| header == name)
}

fn normalized_row_states(row: &StringRecord, indexes: &IpCityHeaderIndexes) -> Vec<String> {
    let mut states = Vec::new();

    for index in [indexes.state1, indexes.state2].into_iter().flatten() {
        let normalized = normalize_name(row.get(index).unwrap_or_default());
        if !normalized.is_empty() && !states.contains(&normalized) {
            states.push(normalized);
        }
    }

    states
}

fn parse_optional_f64(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        value.parse::<f64>().ok()
    }
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn normalize_name_collapses_whitespace_and_quotes() {
        assert_eq!(normalize_name("  New   York  "), "new york");
        assert_eq!(normalize_name("L’viv"), "l'viv");
    }

    #[test]
    fn args_require_an_explicit_city_csv_source() {
        let result = Args::parse(
            [
                "--cities-tsv",
                "cities.csv",
                "--output-json",
                "ip-city-merged.json",
            ]
            .into_iter()
            .map(String::from),
        );

        assert!(
            result
                .expect_err("missing source")
                .to_string()
                .contains("--ip-city-csv")
        );
    }

    #[test]
    fn args_accept_a_local_city_csv_source() {
        let args = Args::parse(
            [
                "--ip-city-csv",
                "local.csv.gz",
                "--cities-tsv",
                "cities.csv",
                "--output-json",
                "ip-city-merged.json",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse args");

        assert_eq!(args.ip_city_csv, "local.csv.gz");
        assert_eq!(args.output_json, PathBuf::from("ip-city-merged.json"));
        assert_eq!(args.cities_tsv, PathBuf::from("cities.csv"));
    }

    #[test]
    fn parses_city_tsv_rows() {
        let row = parse_city_tsv_row("Paris\t48.8566\t2.3522\tFR", 1).unwrap();
        assert_eq!(row.city_name, "Paris");
        assert_eq!(row.country_code, "FR");
        assert_eq!(row.state_name, None);
        assert_close(row.latitude, 48.8566);
        assert_close(row.longitude, 2.3522);
    }

    #[test]
    fn parses_optional_city_state_from_tsv_rows() {
        let row = parse_city_tsv_row("Springfield\t39.7817\t-89.6501\tUS\tIllinois", 1).unwrap();
        assert_eq!(row.city_name, "Springfield");
        assert_eq!(row.country_code, "US");
        assert_eq!(row.state_name.as_deref(), Some("Illinois"));
        assert_close(row.latitude, 39.7817);
        assert_close(row.longitude, -89.6501);
    }

    #[test]
    fn chooses_nearest_duplicate_candidate() {
        let candidates = vec![
            CityCandidate {
                city_name: "Springfield".to_string(),
                latitude: 39.7817,
                longitude: -89.6501,
            },
            CityCandidate {
                city_name: "Springfield".to_string(),
                latitude: 44.0462,
                longitude: -123.0220,
            },
        ];

        let chosen = choose_nearest(39.80, -89.64, &candidates);
        assert_close(chosen.latitude, 39.7817);
    }

    #[test]
    fn state_match_disambiguates_duplicate_city_candidates() {
        let ip_csv = "\
ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n\
5.0.0.0,5.0.0.255,US,Illinois,,Springfield,,44.0400,-123.0200,America/Chicago\n";
        let cities_tsv = "\
Springfield\t39.7817\t-89.6501\tUS\tIllinois\n\
Springfield\t44.0462\t-123.0220\tUS\tOregon\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        assert_eq!(needed.exact_keys.len(), 1);
        assert_eq!(needed.state_keys.len(), 1);

        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needed).unwrap();
        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.exact_city_rows, 0);
        assert_eq!(stats.state_city_rows, 1);
        assert_eq!(stats.proximity_city_rows, 0);
        assert_eq!(merged[0].city_name, "Springfield");
        assert_close(merged[0].latitude, 39.7817);
        assert_close(merged[0].longitude, -89.6501);
    }

    #[test]
    fn duplicate_city_without_state_keeps_nearest_policy() {
        let ip_csv = "\
ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n\
5.0.1.0,5.0.1.255,US,,,Springfield,,44.0400,-123.0200,America/Los_Angeles\n";
        let cities_tsv = "\
Springfield\t39.7817\t-89.6501\tUS\tIllinois\n\
Springfield\t44.0462\t-123.0220\tUS\tOregon\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        assert_eq!(needed.exact_keys.len(), 1);
        assert!(needed.state_keys.is_empty());

        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needed).unwrap();
        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.exact_city_rows, 1);
        assert_eq!(stats.state_city_rows, 0);
        assert_eq!(merged[0].city_name, "Springfield");
        assert_close(merged[0].latitude, 44.0462);
        assert_close(merged[0].longitude, -123.0220);
    }

    #[test]
    fn streams_merged_json_with_city_candidates() {
        let ip_csv = "\
ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n\
1.0.0.0,1.0.0.255,FR,,,Paris,,48.8570,2.3500,Europe/Paris\n\
2.0.0.0,2.0.0.255,US,,,Austin,,30.2672,-97.7431,America/Chicago\n";
        let cities_tsv = "\
Paris\t48.8566\t2.3522\tFR\n\
Paris\t33.6609\t-95.5555\tUS\n\
Austin\t30.2711\t-97.7437\tUS\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needed).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.total_ip_rows, 2);
        assert_eq!(stats.exact_city_rows, 2);
        assert_eq!(stats.proximity_city_rows, 0);
        assert_eq!(stats.passthrough_ip_rows, 0);
        assert_eq!(
            merged,
            vec![
                OutputRecord {
                    ip_range_start: "1.0.0.0".to_string(),
                    ip_range_end: "1.0.0.255".to_string(),
                    city_name: "Paris".to_string(),
                    country_code: "FR".to_string(),
                    latitude: 48.8566,
                    longitude: 2.3522,
                },
                OutputRecord {
                    ip_range_start: "2.0.0.0".to_string(),
                    ip_range_end: "2.0.0.255".to_string(),
                    city_name: "Austin".to_string(),
                    country_code: "US".to_string(),
                    latitude: 30.2711,
                    longitude: -97.7437,
                },
            ]
        );
    }

    #[test]
    fn falls_back_to_ip_row_when_city_candidate_is_missing() {
        let ip_csv = "\
ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n\
3.0.0.0,3.0.0.255,CA,,,Unknown,,43.6532,-79.3832,America/Toronto\n";
        let cities_tsv = "Paris\t48.8566\t2.3522\tFR\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needed).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.exact_city_rows, 0);
        assert_eq!(stats.proximity_city_rows, 0);
        assert_eq!(stats.passthrough_ip_rows, 1);
        assert_eq!(merged[0].city_name, "Unknown");
        assert_eq!(merged[0].country_code, "CA");
        assert_close(merged[0].latitude, 43.6532);
        assert_close(merged[0].longitude, -79.3832);
    }

    #[test]
    fn skips_rows_without_coordinates() {
        let ip_csv = "\
ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n\
4.0.0.0,4.0.0.255,CA,,,Toronto,,,,America/Toronto\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new("")), &needed).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.skipped_ip_rows, 1);
        assert_eq!(merged, [] as [OutputRecord; 0]);
    }

    #[test]
    fn decodes_plain_and_gzip_sources() {
        let plain = b"hello,world\n".to_vec();
        let decoded_plain = decode_maybe_gzip("sample.csv", &plain).unwrap();
        assert_eq!(decoded_plain, plain);

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"hello,world\n").unwrap();
        let gzipped = encoder.finish().unwrap();
        let decoded_gzip = decode_maybe_gzip("sample.csv.gz", &gzipped).unwrap();
        assert_eq!(decoded_gzip, b"hello,world\n");
    }

    #[test]
    fn detects_named_csv_layout() {
        let csv = concat!(
            "ip_range_start,ip_range_end,country_code,state1,state2,city,postcode,latitude,longitude,timezone\n",
            "1.1.1.0,1.1.1.255,AU,,,Sydney,,-33.8688,151.2093,Australia/Sydney\n"
        );

        let layout = detect_ip_city_csv_layout(csv.as_bytes()).unwrap();
        assert!(layout.has_headers);
        assert_eq!(layout.indexes.ip_range_start, 0);
        assert_eq!(layout.indexes.state1, Some(3));
        assert_eq!(layout.indexes.state2, Some(4));
        assert_eq!(layout.indexes.city, 5);
    }

    #[test]
    fn detects_headerless_geolite2_layout() {
        let csv = "1.0.1.0,1.0.3.255,CN,,,,,34.7732,113.7220,Asia/Shanghai\n";

        let layout = detect_ip_city_csv_layout(csv.as_bytes()).unwrap();
        assert!(!layout.has_headers);
        assert_eq!(layout.indexes.ip_range_start, 0);
        assert_eq!(layout.indexes.ip_range_end, 1);
        assert_eq!(layout.indexes.country_code, 2);
        assert_eq!(layout.indexes.state1, Some(3));
        assert_eq!(layout.indexes.state2, Some(4));
        assert_eq!(layout.indexes.city, 5);
        assert_eq!(layout.indexes.latitude, 7);
        assert_eq!(layout.indexes.longitude, 8);
    }

    #[test]
    fn headerless_geolite2_rows_are_merged() {
        let ip_csv = "\
1.0.1.0,1.0.3.255,FR,,,Paris,,48.8570,2.3500,Europe/Paris\n\
2.0.0.0,2.0.0.255,US,,,Austin,,30.2672,-97.7431,America/Chicago\n";
        let cities_tsv = "\
Paris\t48.8566\t2.3522\tFR\n\
Austin\t30.2711\t-97.7437\tUS\n";

        let needed = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needed).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.total_ip_rows, 2);
        assert_eq!(stats.exact_city_rows, 2);
        assert_eq!(stats.proximity_city_rows, 0);
        assert_eq!(merged[0].city_name, "Paris");
        assert_eq!(merged[1].city_name, "Austin");
    }

    #[test]
    fn blank_city_rows_can_use_proximity_fallback() {
        let ip_csv = "\
1.0.1.0,1.0.3.255,JP,,,,,35.6895,139.6917,Asia/Tokyo\n";
        let cities_tsv = "\
Tokyo\t35.6895\t139.6917\tJP\n\
Osaka\t34.6937\t135.5023\tJP\n";

        let needs = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needs).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.exact_city_rows, 0);
        assert_eq!(stats.proximity_city_rows, 1);
        assert_eq!(merged[0].city_name, "Tokyo");
    }

    #[test]
    fn distant_blank_city_rows_stay_empty() {
        let ip_csv = "\
1.0.1.0,1.0.3.255,AU,,,,,-33.4940,143.2104,Australia/Sydney\n";
        let cities_tsv = "Sydney\t-33.8688\t151.2093\tAU\n";

        let needs = collect_import_needs(ip_csv.as_bytes()).unwrap();
        let candidates =
            load_needed_city_candidates(BufReader::new(Cursor::new(cities_tsv)), &needs).unwrap();

        let mut output = Vec::new();
        let stats = stream_merged_json(ip_csv.as_bytes(), &candidates, &mut output).unwrap();
        let merged: Vec<OutputRecord> = serde_json::from_slice(&output).unwrap();

        assert_eq!(stats.proximity_city_rows, 0);
        assert_eq!(merged[0].city_name, "");
    }
}
