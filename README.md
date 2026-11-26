# rs-geoip

Look up the country of an IPv4 or IPv6 address offline. The default reader uses only the Rust standard library. It loads and validates the table once, then searches immutable arrays without allocating or locking. Missing ranges return `None`. An IPv4-mapped IPv6 address uses the IPv4 table. Version 0.2.0 requires Rust 1.100 or newer.
[crates.io](https://crates.io/crates/rs-geoip/0.2.0) · [API reference](https://docs.rs/rs-geoip/0.2.0/rs_geoip/)

## Build a country database

The crate ships without a database. Install the CLI to create `country.bin` and check it before loading it in your application.

```sh
cargo install rs-geoip --version 0.2.0 --locked --features build --bin rs-geo-country-build
```

Download the numeric IPv4 and IPv6 CSVs and build the database locally. The default source prioritizes the end user's country. Its maintainers publish daily updates. [Dataset and licence](https://github.com/sapics/ip-location-db#country)

```sh
base=https://github.com/sapics/ip-location-db/releases/download/latest
curl -fL "$base/user-country-ipv4-num.csv" -o ipv4.csv
curl -fL "$base/user-country-ipv6-num.csv" -o ipv6.csv
rs-geo-country-build --ipv4 ipv4.csv --ipv6 ipv6.csv \
  --label "user-country PDDL-1.0" --out country.bin
rs-geo-country-build check country.bin
```

For repeated updates, run the following script from an `rs-geoip` checkout with the builder on your `PATH`. It downloads both inputs, prints their SHA-256 digests and the output digest, and checks the new database before installing it. The updater expects records in both feeds. For an intentionally IPv4-only table, pass an empty IPv6 CSV to the builder directly. [Update script](https://github.com/codixor/rs-geoip/blob/main/scripts/update-country-db.sh)

```sh
bash scripts/update-country-db.sh \
  "$base/user-country-ipv4-num.csv" "$base/user-country-ipv6-num.csv" country.bin
```

## Look up an address

```toml
[dependencies]
rs-geoip = "0.2.0"
```

```rust
use rs_geoip::country::CountryDb;
use std::net::IpAddr;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = CountryDb::from_file("country.bin")?;
    let ip: IpAddr = "1.1.1.1".parse()?;
    if let Some(country) = db.lookup(ip) {
        println!("{}", country.as_str());
    }
    Ok(())
}
```

`CountryDb::from_bytes` also accepts a database loaded by your application. `db.info()` reports its label, build time and range counts. Load a replacement to pick up new data. Country estimates can be inaccurate, particularly on VPNs and mobile networks.

## C2 country database validation

On 26 November 2026, the published 0.2.0 builder rebuilt `country.bin` from the two `user-country` numeric CSVs used in C2. The CLI's `check` command reopened the file and reported **253 country codes**. The file measured **13,352,522 bytes**. [Original C2 build and input SHA-256 digests](https://github.com/codixor/rs-geoip/pull/3)

| Address family | Source rows | Ranges in `country.bin` | Sampled lookups matching the CSV |
| --- | ---: | ---: | ---: |
| IPv4 | 303,665 | 303,665 | 500 / 500 |
| IPv6 | 295,681 | 295,681 | 500 / 500 |

These inputs already had adjacent same-country ranges merged. The check used the published reader and sampled range starts, ends and interior addresses. Future daily datasets can have different counts. The binary header includes the build time, so a rebuild can have a different checksum even with identical CSVs.

## City lookup and migration

If you used the former `rs-geo-ip` workspace crate, change the dependency to `rs-geoip`, the Rust import to `rs_geoip`, and enable `city`.

```toml
[dependencies]
rs-geoip = { version = "0.2.0", default-features = false, features = ["city"] }
```

`GeoIpResolver::from_flat_packed_file` opens a compressed flat pack. `GeoIpResolver::from_paged_packed_file` opens a paged pack and decodes city blocks on demand. These packs cover IPv4. Supply packs you built locally. The optional `builtin` feature can embed `ip-city-packed.lz4` and `ip-city-packed-paged.bin` when present in the crate's source directory at build time. A missing pack yields `GeoIpError::BuiltinPackUnavailable` when its resolver is requested.

| Feature | Enables |
| --- | --- |
| `country` (default) | IPv4 and IPv6 country reader |
| `city` | Flat and paged city lookups, plus the native file APIs |
| `builtin` | City lookup with optional local packs embedded at compile time |
| `build` | Country builder with `country`, local city tools with `city` |
| `native-tools` | City tools with HTTP input support and the city benchmark |

The crate contains source code only. The code is available under MIT or Apache-2.0. Country and city datasets have their own terms. [Data licences](https://github.com/codixor/rs-geoip/blob/main/LICENSE-DATA.md)
