# rs-geoip

Offline IP geolocation for Rust. The `city` feature looks up IPv4 addresses in local city packs and returns an approximate position, city name, and country code. The repository and crate ship no databases.

## City lookup

Add `rs-geoip = { version = "0.2", features = ["city"] }` to your dependencies. Use `GeoIpResolver` to open a local file once and reuse it across lookups.

```rust
use rs_geoip::{GeoIpError, GeoIpResolver};
use std::{net::Ipv4Addr, path::Path};

fn lookup(path: &Path, ip: Ipv4Addr) -> Result<(), GeoIpError> {
    let db = GeoIpResolver::from_flat_packed_file(path)?;
    if let Some(fix) = db.lookup_ipv4(ip)? {
        println!("{:?} {:?}", fix.city_name, fix.point.as_tuple());
    }
    Ok(())
}
```

Flat LZ4 packs decompress when opened. Paged packs load city blocks on demand. Use `from_paged_packed_file` for a paged file, or the byte-based constructors for an in-memory pack. With `builtin`, building from a local checkout embeds any `ip-city-packed.lz4` or `ip-city-packed-paged.bin` in the crate root. A missing pack returns `GeoIpError::BuiltinPackUnavailable` when its resolver is requested.

## Building local city packs

Supply a local city-range CSV (optionally gzip-compressed) and a tab-separated city reference file. City data derived from GeoLite2 is subject to its source terms. Keep downloaded and generated files outside version control.

```sh
cargo run --features city,build --bin build_ip_city_json -- \
  --ip-city-csv city-ranges.csv.gz --cities-tsv cities.tsv --output-json ip-city-merged.json
bash scripts/build-ip-city-artifacts.sh
```

The script builds the flat and paged outputs from the merged JSON. The existing `native-tools` feature also supports importing a URL explicitly passed to `--ip-city-csv` and compiling the optional benchmark. Tests use small synthetic fixtures. The benchmark expects local MMDB, flat, and paged files.

See the [city format notes](PLAN.md) and [lookup example](examples/local_ip_lookup.rs).
