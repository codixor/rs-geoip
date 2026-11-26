#![allow(unsafe_code)]

use rs_geoip::ip_city_pack::{
    PACKED_IP_CITY_DB_VERSION, PackedCityRecord, PackedIpCityDb, PackedRangeRecord,
    PagedPackedIpCityDb,
};
use rs_geoip::{
    AccuracyKind, GeoIpError, GeoIpFormat, GeoIpResolver, GeoSource, lookup_ip_in_flat_packed_file,
    lookup_ip_in_paged_packed_file,
};
use rs_mmap::ReadOnlyMmap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn sample_db() -> PackedIpCityDb {
    PackedIpCityDb {
        version: PACKED_IP_CITY_DB_VERSION,
        city_count: 2,
        range_count: 2,
        cities: vec![
            PackedCityRecord {
                city_name: "Paris".to_owned(),
                country_code: "FR".to_owned(),
                latitude: 48.8566,
                longitude: 2.3522,
            },
            PackedCityRecord {
                city_name: "Lyon".to_owned(),
                country_code: "FR".to_owned(),
                latitude: 45.764,
                longitude: 4.8357,
            },
        ],
        ranges: vec![
            PackedRangeRecord {
                ip_range_start: u32::from(Ipv4Addr::new(1, 0, 0, 0)),
                ip_range_end: u32::from(Ipv4Addr::new(1, 0, 0, 255)),
                city_id: 0,
            },
            PackedRangeRecord {
                ip_range_start: u32::from(Ipv4Addr::new(1, 0, 1, 0)),
                ip_range_end: u32::from(Ipv4Addr::new(1, 0, 1, 255)),
                city_id: 1,
            },
        ],
    }
}

fn unique_path(suffix: &str) -> PathBuf {
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let number = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("rs-geoip-{pid}-{nanos}-{number}-{suffix}"))
}

fn write_flat_fixture() -> PathBuf {
    let path = unique_path("flat.lz4");
    let bytes = sample_db().encode_lz4().expect("encode flat fixture");
    fs::write(&path, bytes).expect("write flat fixture");
    path
}

fn write_paged_fixture() -> PathBuf {
    let path = unique_path("paged.bin");
    let paged = PagedPackedIpCityDb::from_flat(&sample_db(), 1).expect("build paged fixture");
    let bytes = paged.encode_bytes().expect("encode paged fixture");
    fs::write(&path, bytes).expect("write paged fixture");
    path
}

fn write_invalid_fixture() -> PathBuf {
    let path = unique_path("invalid.bin");
    fs::write(&path, b"not-a-valid-packed-db").expect("write invalid fixture");
    path
}

fn write_invalid_coordinate_flat_fixture() -> PathBuf {
    let path = unique_path("flat-invalid-coords.lz4");
    let mut db = sample_db();
    db.cities[0].latitude = f64::NAN;
    let bytes = db
        .encode_lz4()
        .expect("encode invalid coordinate flat fixture");
    fs::write(&path, bytes).expect("write invalid coordinate flat fixture");
    path
}

fn write_u32_fixture(values: &[u32]) -> PathBuf {
    let path = unique_path("typed-view.bin");
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    fs::write(&path, bytes).expect("write typed view fixture");
    path
}

#[test]
fn flat_resolver_returns_approximate_fix() {
    let path = write_flat_fixture();
    let resolver = GeoIpResolver::from_flat_packed_file(&path).expect("open flat resolver");
    let fix = resolver
        .lookup_ipv4(Ipv4Addr::new(1, 0, 1, 23))
        .expect("lookup succeeds")
        .expect("ip present");

    assert_eq!(resolver.format(), GeoIpFormat::FlatPackedLz4);
    assert_eq!(fix.source, GeoSource::PackedLz4);
    assert_eq!(fix.accuracy_kind, AccuracyKind::ApproximateIp);
    assert_eq!(fix.city_name.as_deref(), Some("Lyon"));
    assert_eq!(fix.country_code.as_deref(), Some("FR"));
    assert_eq!(fix.point.as_tuple(), (45.764, 4.8357));

    let _ = fs::remove_file(path);
}

#[test]
fn paged_resolver_returns_approximate_fix() {
    let path = write_paged_fixture();
    let resolver = GeoIpResolver::from_paged_packed_file(&path).expect("open paged resolver");
    let fix = resolver
        .lookup_ipv4(Ipv4Addr::new(1, 0, 0, 42))
        .expect("lookup succeeds")
        .expect("ip present");

    assert_eq!(resolver.format(), GeoIpFormat::PagedPacked);
    assert_eq!(fix.source, GeoSource::PackedPaged);
    assert_eq!(fix.city_name.as_deref(), Some("Paris"));

    let _ = fs::remove_file(path);
}

#[test]
fn ipv6_returns_none_for_current_dataset() {
    let path = write_flat_fixture();
    let resolver = GeoIpResolver::from_flat_packed_file(&path).expect("open flat resolver");
    let fix = resolver
        .lookup_ip(IpAddr::V6(Ipv6Addr::LOCALHOST))
        .expect("lookup succeeds");

    assert!(fix.is_none());

    let _ = fs::remove_file(path);
}

#[test]
fn convenience_helpers_match_resolver_behavior() {
    let flat = write_flat_fixture();
    let paged = write_paged_fixture();
    let ip = IpAddr::V4(Ipv4Addr::new(1, 0, 1, 7));

    let flat_fix = lookup_ip_in_flat_packed_file(&flat, ip)
        .expect("flat lookup succeeds")
        .expect("flat hit");
    let paged_fix = lookup_ip_in_paged_packed_file(&paged, ip)
        .expect("paged lookup succeeds")
        .expect("paged hit");

    assert_eq!(flat_fix.point, paged_fix.point);
    assert_eq!(flat_fix.city_name, paged_fix.city_name);
    assert_eq!(paged_fix.country_code.as_deref(), Some("FR"));

    let _ = fs::remove_file(flat);
    let _ = fs::remove_file(paged);
}

#[test]
fn missing_database_file_returns_typed_error() {
    let error = GeoIpResolver::from_flat_packed_file(unique_path("missing.lz4")).unwrap_err();
    assert!(matches!(error, GeoIpError::PackedIp(_)));
}

#[test]
fn invalid_database_format_returns_typed_error() {
    let path = write_invalid_fixture();
    let error = GeoIpResolver::from_paged_packed_file(&path).unwrap_err();

    assert!(matches!(error, GeoIpError::PackedIp(_)));

    let _ = fs::remove_file(path);
}

#[test]
fn invalid_coordinates_are_rejected() {
    let path = write_invalid_coordinate_flat_fixture();
    let resolver = GeoIpResolver::from_flat_packed_file(&path).expect("open flat resolver");
    let error = resolver
        .lookup_ipv4(Ipv4Addr::new(1, 0, 0, 23))
        .expect_err("invalid coordinates should fail");

    assert!(matches!(error, GeoIpError::InvalidCoordinates));

    let _ = fs::remove_file(path);
}

#[test]
fn mmap_paged_resolver_returns_approximate_fix() {
    let path = write_paged_fixture();
    // SAFETY: The fixture file is written once for the test and not modified
    // or truncated while the mapped resolver exists.
    let resolver = unsafe { GeoIpResolver::from_paged_packed_mmap_file(&path) }
        .expect("open mmap paged resolver");
    let fix = resolver
        .lookup_ipv4(Ipv4Addr::new(1, 0, 1, 42))
        .expect("lookup succeeds")
        .expect("ip present");

    assert_eq!(resolver.format(), GeoIpFormat::PagedPacked);
    assert_eq!(fix.source, GeoSource::PackedPagedMmap);
    assert_eq!(fix.city_name.as_deref(), Some("Lyon"));

    drop(resolver);
    fs::remove_file(path).expect("remove mapped fixture");
}

#[test]
fn dx_mmap_zerocopy_views_work_from_geo_ip() {
    let path = write_u32_fixture(&[11, 22, 33, 44]);
    let mapped = ReadOnlyMmap::open(&path).expect("open typed-view fixture");

    let values = mapped
        .view_slice::<u32>()
        .expect("u32 typed view should succeed");
    assert_eq!(values, &[11, 22, 33, 44]);

    let first_slice = mapped
        .slice(0..std::mem::size_of::<u32>())
        .expect("first u32 slice should be in bounds");
    let first = first_slice
        .view::<u32>()
        .expect("single typed view should succeed");
    assert_eq!(*first, 11);

    let _ = fs::remove_file(path);
}

#[test]
fn builtin_availability_matches_local_inputs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let flat = cfg!(feature = "builtin") && root.join("ip-city-packed.lz4").is_file();
    let paged = cfg!(feature = "builtin") && root.join("ip-city-packed-paged.bin").is_file();
    assert_eq!(rs_geoip::builtin_flat_packed_bytes().is_some(), flat);
    assert_eq!(rs_geoip::builtin_paged_packed_bytes().is_some(), paged);
    if flat {
        GeoIpResolver::from_builtin_flat_packed().expect("open embedded flat city pack");
    } else {
        assert!(matches!(
            GeoIpResolver::from_builtin_flat_packed(),
            Err(GeoIpError::BuiltinPackUnavailable("flat"))
        ));
    }
    if paged {
        GeoIpResolver::from_builtin_paged_packed().expect("open embedded paged city pack");
    } else {
        assert!(matches!(
            GeoIpResolver::from_builtin_paged_packed(),
            Err(GeoIpError::BuiltinPackUnavailable("paged"))
        ));
    }
}

#[test]
fn resolves_both_pack_formats_from_fixture_bytes() {
    let ip = IpAddr::V4(Ipv4Addr::new(1, 0, 0, 42));
    let flat = sample_db();
    let paged = PagedPackedIpCityDb::from_flat(&flat, 1).unwrap();
    let flat_fix = rs_geoip::lookup_ip_in_flat_packed_bytes(&flat.encode_lz4().unwrap(), ip)
        .unwrap()
        .unwrap();
    let paged_fix = rs_geoip::lookup_ip_in_paged_packed_bytes(&paged.encode_bytes().unwrap(), ip)
        .unwrap()
        .unwrap();
    assert_eq!(flat_fix.city_name.as_deref(), Some("Paris"));
    assert_eq!(flat_fix.point, paged_fix.point);
    assert_eq!(flat_fix.city_name, paged_fix.city_name);
}
