use indexmap::IndexMap;
use rs_geoip::ip_city_pack::{
    PACKED_IP_CITY_DB_VERSION, PackedCityRecord, PackedIpCityDb, PackedRangeRecord,
};
use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

type DynError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Deserialize)]
struct JsonRecord {
    ip_range_start: String,
    ip_range_end: String,
    city_name: String,
    country_code: String,
    latitude: f64,
    longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PackedCityKey {
    city_name: String,
    country_code: String,
    latitude_bits: u64,
    longitude_bits: u64,
}

#[derive(Debug, Default)]
struct PackStats {
    input_rows: u64,
    city_count: u64,
    range_count: u64,
    merged_adjacent_ranges: u64,
    empty_city_rows: u64,
}

fn main() -> Result<(), DynError> {
    let args = Args::parse(std::env::args().skip(1))?;
    let input = File::open(&args.input_json)?;
    let packed = build_packed_db(BufReader::new(input))?;

    let compressed = packed.db.encode_lz4()?;

    let mut writer = BufWriter::new(File::create(&args.output_lz4)?);
    writer.write_all(&compressed)?;
    writer.flush()?;

    eprintln!(
        "packed {} input rows into {} cities and {} ranges (merged_adjacent={}, empty_city_rows={})",
        packed.stats.input_rows,
        packed.stats.city_count,
        packed.stats.range_count,
        packed.stats.merged_adjacent_ranges,
        packed.stats.empty_city_rows
    );
    eprintln!(
        "compressed bytes={} output={}",
        compressed.len(),
        args.output_lz4.display()
    );

    Ok(())
}

struct Args {
    input_json: PathBuf,
    output_lz4: PathBuf,
}

impl Args {
    fn parse<I>(mut args: I) -> Result<Self, DynError>
    where
        I: Iterator<Item = String>,
    {
        let mut input_json = None;
        let mut output_lz4 = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--input-json" => input_json = args.next().map(PathBuf::from),
                "--output-lz4" => output_lz4 = args.next().map(PathBuf::from),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => {
                    return Err(invalid_input(format!("unexpected argument: {other}")).into());
                }
            }
        }

        Ok(Self {
            input_json: input_json.ok_or_else(|| {
                invalid_input("missing required --input-json <path> argument".to_string())
            })?,
            output_lz4: output_lz4.ok_or_else(|| {
                invalid_input("missing required --output-lz4 <path> argument".to_string())
            })?,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --features city,build --bin build_ip_city_pack -- \\\n    --input-json <merged ip-city json> \\\n    --output-lz4 <packed output.lz4>"
    );
}

struct BuildOutput {
    db: PackedIpCityDb,
    stats: PackStats,
}

fn build_packed_db(reader: impl BufRead) -> Result<BuildOutput, DynError> {
    let mut city_ids: IndexMap<PackedCityKey, u32> = IndexMap::new();
    let mut cities: Vec<PackedCityRecord> = Vec::new();
    let mut ranges: Vec<PackedRangeRecord> = Vec::new();
    let mut stats = PackStats::default();

    for_each_json_record(reader, |record| {
        stats.input_rows += 1;
        if record.city_name.is_empty() {
            stats.empty_city_rows += 1;
        }

        let city_id = intern_city(&mut city_ids, &mut cities, &record)?;
        let range = PackedRangeRecord {
            ip_range_start: parse_ipv4_range(&record.ip_range_start)?,
            ip_range_end: parse_ipv4_range(&record.ip_range_end)?,
            city_id,
        };

        if let Some(previous) = ranges.last_mut()
            && previous.city_id == range.city_id
            && previous.ip_range_end.checked_add(1) == Some(range.ip_range_start)
        {
            previous.ip_range_end = range.ip_range_end;
            stats.merged_adjacent_ranges += 1;
            return Ok(());
        }

        ranges.push(range);
        Ok(())
    })?;

    stats.city_count = cities.len() as u64;
    stats.range_count = ranges.len() as u64;

    Ok(BuildOutput {
        db: PackedIpCityDb {
            version: PACKED_IP_CITY_DB_VERSION,
            city_count: u32::try_from(cities.len())
                .map_err(|_| invalid_input("too many cities".to_string()))?,
            range_count: u32::try_from(ranges.len())
                .map_err(|_| invalid_input("too many ranges".to_string()))?,
            cities,
            ranges,
        },
        stats,
    })
}

fn for_each_json_record(
    reader: impl BufRead,
    mut f: impl FnMut(JsonRecord) -> Result<(), DynError>,
) -> Result<(), DynError> {
    for line in reader.lines() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line == "[" || line == "]" {
            continue;
        }

        let json = line.strip_suffix(',').unwrap_or(line);
        f(serde_json::from_str(json)?)?;
    }

    Ok(())
}

fn intern_city(
    city_ids: &mut IndexMap<PackedCityKey, u32>,
    cities: &mut Vec<PackedCityRecord>,
    record: &JsonRecord,
) -> Result<u32, DynError> {
    let key = PackedCityKey {
        city_name: record.city_name.clone(),
        country_code: record.country_code.clone(),
        latitude_bits: record.latitude.to_bits(),
        longitude_bits: record.longitude.to_bits(),
    };

    if let Some(city_id) = city_ids.get(&key) {
        return Ok(*city_id);
    }

    let city_id =
        u32::try_from(cities.len()).map_err(|_| invalid_input("too many cities".to_string()))?;
    cities.push(PackedCityRecord {
        city_name: record.city_name.clone(),
        country_code: record.country_code.clone(),
        latitude: record.latitude,
        longitude: record.longitude,
    });
    city_ids.insert(key, city_id);
    Ok(city_id)
}

fn parse_ipv4_range(value: &str) -> Result<u32, DynError> {
    let ip = Ipv4Addr::from_str(value.trim())
        .map_err(|err| invalid_input(format!("invalid IPv4 address {value:?}: {err}")))?;
    Ok(u32::from(ip))
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn streams_json_array_lines() {
        let input = b"[\n{\"ip_range_start\":\"1.0.0.0\",\"ip_range_end\":\"1.0.0.255\",\"city_name\":\"Paris\",\"country_code\":\"FR\",\"latitude\":48.8566,\"longitude\":2.3522},\n{\"ip_range_start\":\"1.0.1.0\",\"ip_range_end\":\"1.0.1.255\",\"city_name\":\"Paris\",\"country_code\":\"FR\",\"latitude\":48.8566,\"longitude\":2.3522}\n]\n";
        let mut records = Vec::new();
        for_each_json_record(Cursor::new(&input[..]), |record| {
            records.push(record);
            Ok(())
        })
        .unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].city_name, "Paris");
    }

    #[test]
    fn packs_and_merges_adjacent_ranges() {
        let input = b"[\n{\"ip_range_start\":\"1.0.0.0\",\"ip_range_end\":\"1.0.0.255\",\"city_name\":\"Paris\",\"country_code\":\"FR\",\"latitude\":48.8566,\"longitude\":2.3522},\n{\"ip_range_start\":\"1.0.1.0\",\"ip_range_end\":\"1.0.1.255\",\"city_name\":\"Paris\",\"country_code\":\"FR\",\"latitude\":48.8566,\"longitude\":2.3522},\n{\"ip_range_start\":\"1.0.2.0\",\"ip_range_end\":\"1.0.2.255\",\"city_name\":\"Lyon\",\"country_code\":\"FR\",\"latitude\":45.7640,\"longitude\":4.8357}\n]\n";

        let packed = build_packed_db(Cursor::new(&input[..])).unwrap();
        assert_eq!(packed.stats.input_rows, 3);
        assert_eq!(packed.stats.city_count, 2);
        assert_eq!(packed.stats.range_count, 2);
        assert_eq!(packed.stats.merged_adjacent_ranges, 1);
        assert_eq!(
            packed.db.ranges[0],
            PackedRangeRecord {
                ip_range_start: u32::from(Ipv4Addr::new(1, 0, 0, 0)),
                ip_range_end: u32::from(Ipv4Addr::new(1, 0, 1, 255)),
                city_id: 0,
            }
        );
    }

    #[test]
    fn compressed_payload_round_trips() {
        let input = b"[\n{\"ip_range_start\":\"1.0.0.0\",\"ip_range_end\":\"1.0.0.255\",\"city_name\":\"\",\"country_code\":\"CN\",\"latitude\":34.7732,\"longitude\":113.7220}\n]\n";

        let packed = build_packed_db(Cursor::new(&input[..])).unwrap();
        let compressed = packed.db.encode_lz4().unwrap();
        let decoded = PackedIpCityDb::decode_lz4(&compressed).unwrap();

        assert_eq!(decoded.range_count, packed.db.range_count);
        assert_eq!(decoded.city_count, packed.db.city_count);
    }
}
