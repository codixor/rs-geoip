# Changelog

## 0.2.0

- The standalone package is named `rs-geoip`. Rust code imports it as `rs_geoip`.
- The default `country` feature adds dependency-free IPv4 and IPv6 lookups,
  including IPv4-mapped IPv6 addresses. The `build` feature adds
  `rs-geo-country-build` to create and check local databases.
- **Breaking change.** Enable `city` for the existing packed readers and geolocation API.
  Local city pack embedding also requires `builtin`. Absent packs return
  `GeoIpError::BuiltinPackUnavailable`.
- City imports take an explicit `--ip-city-csv` source. The crate ships source code only.
