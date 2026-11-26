# rs-geoip Plan

Goal: keep `rs-geoip` as the dedicated offline IP geolocation crate with its
own repository, with a simple local lookup API, reproducible import/build tools, and
bench-driven packed runtime formats.

## Direction

- [x] Keep `rs-geoip` separate from `sys-geo`
- [x] Keep the public API focused on local `IP -> approximate location`
- [x] Use `thiserror` for typed error handling
- [x] Keep outbound resolver services out of the default design
- [x] Prefer local packed formats over MMDB on the hot runtime path when
  benchmarks justify it

## Public API

- [x] `GeoPoint`
- [x] `GeoIpError`
- [x] `GeoIpResolver`
- [x] `GeoIpFormat`
- [x] `GeoFix`
- [x] `GeoSource`
- [x] one-off lookup helpers for flat, paged, and paged-mmap readers

## Data Pipeline

- [x] Import from `sapics/ip-location-db` city CSV exports
- [x] Accept local files and remote `.csv.gz` sources
- [x] Accept an explicit city-range source and local city reference TSV
- [x] Keep merge order explicit:
  - [x] exact normalized `country_code + city name`
  - [x] then `country_code + city name + state`
  - [x] only then bounded proximity fallback
- [x] Emit a stable merged JSON artifact

## Runtime Layout

- [x] Build a flat packed two-table format:
  - [x] IP ranges -> `city_id`
  - [x] `city_id` -> city metadata
- [x] Compress the flat runtime format with `lz4_flex`
- [x] Build a paged packed variant with lazy city-block decode
- [x] Add an mmap-backed paged reader
- [x] Move the paged file preamble to POD
- [x] Move the paged block metadata index to POD
- [x] Use `rs-mmap` `zerocopy-views` in the actual runtime path
- [x] Rebuild flat `lz4` and paged `bin` artifacts as one paired refresh

## Bench Decisions

- [x] Compare MMDB against flat packed `lz4`
- [x] Compare flat packed against paged packed
- [x] Compare paged packed against paged mmap
- [x] Keep flat packed as the hot-path winner
- [x] Keep paged mmap as the cold-open winner
- [x] Treat MMDB as the source/import baseline instead of the final runtime
  owner

## Validation And Proof

- [x] Root tests under `tests/`
- [x] Example for real local lookup
- [x] Validation script
- [x] Bench target
- [x] Typed tests for:
  - [x] valid lookup success
  - [x] missing database file
  - [x] invalid packed data
  - [x] invalid coordinates
  - [x] mmap typed-view integration

## Next Useful Work

- [x] Add the state-aware merge pass instead of skipping directly from exact
  match to proximity
- [x] Add explicit duplicate-city ambiguity policy tests
- [ ] Decide whether to freeze the packed file formats as `v1` and document them
  as stable
- [ ] Decide whether IPv6 import/runtime support belongs in the first public
  release or a follow-up batch
