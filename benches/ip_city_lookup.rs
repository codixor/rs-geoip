#![allow(unsafe_code)]

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use maxminddb::{Reader, geoip2};
use rs_geoip::ip_city_pack::{
    MmapPagedPackedIpCityLookup, PackedIpCityLookup, PagedPackedIpCityLookup,
};
use std::env;
use std::hint::black_box;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

fn packed_path() -> PathBuf {
    env::var_os("GEO_IP_PACKED_PATH").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ip-city-packed.lz4"),
        PathBuf::from,
    )
}

fn mmdb_path() -> PathBuf {
    env::var_os("GEO_IP_MMDB_PATH").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("geoip.mmdb"),
        PathBuf::from,
    )
}

fn paged_path() -> PathBuf {
    env::var_os("GEO_IP_PAGED_PATH").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ip-city-packed-paged.bin"),
        PathBuf::from,
    )
}

fn sample_ips() -> Vec<Ipv4Addr> {
    let lookup = PackedIpCityLookup::from_lz4_file(packed_path()).expect("load packed lookup");
    let ranges = &lookup.db().ranges;
    let target = 2048usize.min(ranges.len().max(1));
    let step = (ranges.len() / target).max(1);

    ranges
        .iter()
        .step_by(step)
        .take(target)
        .map(|range| {
            let midpoint = range.ip_range_start + ((range.ip_range_end - range.ip_range_start) / 2);
            Ipv4Addr::from(midpoint)
        })
        .collect()
}

fn lookup_mmdb_point<S: AsRef<[u8]>>(reader: &Reader<S>, ip: Ipv4Addr) -> Option<(f64, f64)> {
    let city = reader
        .lookup(IpAddr::V4(ip))
        .ok()?
        .decode::<geoip2::City<'_>>()
        .ok()??;
    let location = city.location;
    Some((location.latitude?, location.longitude?))
}

fn lookup_packed_point(lookup: &PackedIpCityLookup, ip: Ipv4Addr) -> Option<(f64, f64)> {
    let found = lookup.find_ipv4(ip)?;
    Some((found.city.latitude, found.city.longitude))
}

fn lookup_paged_point(lookup: &PagedPackedIpCityLookup, ip: Ipv4Addr) -> Option<(f64, f64)> {
    let found = lookup.find_ipv4(ip)?;
    let city = found.city();
    Some((city.latitude, city.longitude))
}

fn lookup_paged_point_mmap(
    lookup: &MmapPagedPackedIpCityLookup,
    ip: Ipv4Addr,
) -> Option<(f64, f64)> {
    let found = lookup.find_ipv4(ip)?;
    let city = found.city();
    Some((city.latitude, city.longitude))
}

fn bench_open_and_single_lookup(c: &mut Criterion) {
    let sample_ip = Ipv4Addr::new(1, 0, 64, 1);
    let mmdb = mmdb_path();
    let packed = packed_path();
    let paged = paged_path();

    let mut group = c.benchmark_group("ip_city/open_and_single_lookup");

    group.bench_function("mmdb", |b| {
        b.iter(|| {
            let reader = Reader::open_readfile(&mmdb).expect("open mmdb");
            black_box(lookup_mmdb_point(&reader, sample_ip));
        });
    });

    group.bench_function("packed_lz4", |b| {
        b.iter(|| {
            let lookup = PackedIpCityLookup::from_lz4_file(&packed).expect("open packed");
            black_box(lookup_packed_point(&lookup, sample_ip));
        });
    });

    group.bench_function("packed_paged", |b| {
        b.iter(|| {
            let lookup = PagedPackedIpCityLookup::from_file(&paged).expect("open paged");
            black_box(lookup_paged_point(&lookup, sample_ip));
        });
    });

    group.bench_function("packed_paged_mmap", |b| {
        b.iter(|| {
            // SAFETY: The benchmark maps a static local file and does not
            // modify or truncate it while the reader exists.
            let lookup = unsafe { MmapPagedPackedIpCityLookup::from_mmap_file(&paged) }
                .expect("open paged mmap");
            black_box(lookup_paged_point_mmap(&lookup, sample_ip));
        });
    });

    group.finish();
}

fn bench_hot_lookup(c: &mut Criterion) {
    let mmdb_reader = Reader::open_readfile(mmdb_path()).expect("open mmdb");
    let packed_lookup = PackedIpCityLookup::from_lz4_file(packed_path()).expect("open packed");
    let paged_path = paged_path();
    let paged_lookup = PagedPackedIpCityLookup::from_file(&paged_path).expect("open paged");
    // SAFETY: The benchmark maps a static local file and does not modify or
    // truncate it while the reader exists.
    let paged_mmap_lookup = unsafe { MmapPagedPackedIpCityLookup::from_mmap_file(&paged_path) }
        .expect("open paged mmap");
    let paged_full_cache = PagedPackedIpCityLookup::from_file_with_cache_capacity(
        &paged_path,
        paged_lookup.db().block_count as usize,
    )
    .expect("open paged full cache");
    // SAFETY: The benchmark maps a static local file and does not modify or
    // truncate it while the reader exists.
    let paged_mmap_full_cache = unsafe {
        MmapPagedPackedIpCityLookup::from_mmap_file_with_cache_capacity(
            &paged_path,
            paged_mmap_lookup.block_count(),
        )
    }
    .expect("open paged mmap full cache");
    let samples = sample_ips();

    let mut group = c.benchmark_group("ip_city/hot_lookup");
    group.throughput(Throughput::Elements(samples.len() as u64));

    group.bench_with_input(
        BenchmarkId::new("mmdb", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_mmdb_point(&mmdb_reader, *ip));
                }
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("packed_lz4", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_packed_point(&packed_lookup, *ip));
                }
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("packed_paged", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_paged_point(&paged_lookup, *ip));
                }
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("packed_paged_mmap", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_paged_point_mmap(&paged_mmap_lookup, *ip));
                }
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("packed_paged_full_cache", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_paged_point(&paged_full_cache, *ip));
                }
            });
        },
    );

    group.bench_with_input(
        BenchmarkId::new("packed_paged_mmap_full_cache", samples.len()),
        &samples,
        |b, ips| {
            b.iter(|| {
                for ip in ips {
                    black_box(lookup_paged_point_mmap(&paged_mmap_full_cache, *ip));
                }
            });
        },
    );

    group.finish();
}

fn bench_load_only(c: &mut Criterion) {
    let mmdb = mmdb_path();
    let packed = packed_path();
    let paged = paged_path();

    let mut group = c.benchmark_group("ip_city/load_only");

    group.bench_function("mmdb_open_readfile", |b| {
        b.iter(|| {
            let reader = Reader::open_readfile(&mmdb).expect("open mmdb");
            black_box(reader);
        });
    });

    group.bench_function("packed_from_lz4_file", |b| {
        b.iter(|| {
            let lookup = PackedIpCityLookup::from_lz4_file(&packed).expect("decode packed");
            black_box(lookup);
        });
    });

    group.bench_function("packed_paged_from_file", |b| {
        b.iter(|| {
            let lookup = PagedPackedIpCityLookup::from_file(&paged).expect("decode paged");
            black_box(lookup);
        });
    });

    group.bench_function("packed_paged_from_mmap_file", |b| {
        b.iter(|| {
            // SAFETY: The benchmark maps a static local file and does not
            // modify or truncate it while the reader exists.
            let lookup = unsafe { MmapPagedPackedIpCityLookup::from_mmap_file(&paged) }
                .expect("decode paged mmap");
            black_box(lookup);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_open_and_single_lookup,
    bench_hot_lookup,
    bench_load_only
);
criterion_main!(benches);
