# Changelog

## 0.2.0

- The standalone package is named `rs-geoip`. Rust code imports it as `rs_geoip`.
- **Breaking change.** Enable `city` for the existing packed readers and geolocation API.
  Local city pack embedding also requires `builtin`. Absent packs return
  `GeoIpError::BuiltinPackUnavailable`.
- City imports take an explicit `--ip-city-csv` source. The crate ships source code only.
