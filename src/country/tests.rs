use super::*;
use std::io::Write;

const CODES: [[u8; 2]; 2] = [*b"AE", *b"GH"];
const LABEL: &[u8] = b"test";
const BUILD_TIME: u64 = 0x0102_0304_0506_0708;

type Row<T> = (T, T, u16);

fn count(n: usize) -> u32 {
    n.try_into().unwrap()
}

fn put_rows<const W: usize, T: Copy>(bytes: &mut Vec<u8>, rows: &[Row<T>], le: fn(T) -> [u8; W]) {
    for &(start, _, _) in rows {
        bytes.extend(le(start));
    }
    for &(_, end, _) in rows {
        bytes.extend(le(end));
    }
    for &(_, _, code) in rows {
        bytes.extend(code.to_le_bytes());
    }
}

// An independent writer for tiny fixtures. Its prefix offsets use a separate
// binary search for every /16, independent of the reader's validation sweep.
fn encode(
    codes: &[[u8; 2]],
    label: &[u8],
    v4: &[Row<u32>],
    v6: &[Row<u128>],
    prefix: bool,
) -> Vec<u8> {
    let mut bytes = b"RSGEOCT1".to_vec();
    bytes.extend(1_u32.to_le_bytes());
    bytes.extend(u32::from(prefix).to_le_bytes());
    bytes.extend(BUILD_TIME.to_le_bytes());
    for n in [codes.len(), v4.len(), v6.len(), label.len()] {
        bytes.extend(count(n).to_le_bytes());
    }
    bytes.extend(label);
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    for code in codes {
        bytes.extend(code);
    }
    put_rows(&mut bytes, v4, u32::to_le_bytes);
    if prefix {
        for p in 0..=u16::MAX {
            let first = v4.partition_point(|&(_, end, _)| end < u32::from(p) << 16);
            bytes.extend(count(first).to_le_bytes());
        }
    }
    put_rows(&mut bytes, v6, u128::to_le_bytes);
    bytes.extend(crc32(&bytes).to_le_bytes());
    bytes
}

fn db(v4: &[Row<u32>], v6: &[Row<u128>], prefix: bool) -> CountryDb {
    CountryDb::from_bytes(&encode(&CODES, LABEL, v4, v6, prefix)).unwrap()
}

fn code(i: u16) -> Option<CountryCode> {
    CODES.get(usize::from(i)).copied().map(CountryCode)
}

fn repair_crc(bytes: &mut [u8]) {
    let (body, crc) = bytes.split_last_chunk_mut::<4>().unwrap();
    *crc = crc32(body).to_le_bytes();
}

#[test]
fn country_code_requires_two_ascii_letters() {
    for s in ["aE", "ae", "AE"] {
        assert_eq!(CountryCode::from_ascii(s).unwrap().as_str(), "AE");
    }
    for s in ["", "A", "ABC", " A", "A1", "Ä", "ß", "🇦🇪"] {
        assert_eq!(CountryCode::from_ascii(s), None);
    }
}

#[test]
fn v4_hit_first_and_last_address_of_range() {
    for prefix in [false, true] {
        let table = db(
            &[(0, 0, 0), (42, 100, 1), (u32::MAX - 1, u32::MAX, 0)],
            &[],
            prefix,
        );
        for (addr, expected) in [(0, 0), (42, 1), (100, 1), (u32::MAX, 0)] {
            assert_eq!(table.lookup(IpAddr::V4(addr.into())), code(expected));
        }
    }
}

#[test]
fn v4_gap_returns_none() {
    for prefix in [false, true] {
        let table = db(&[(7, 9, 1), (11, 28, 0)], &[], prefix);
        for addr in [0, 6, 10, 29, u32::MAX] {
            assert_eq!(table.lookup_v4(addr.into()), None);
        }
        assert_eq!(db(&[], &[], prefix).lookup_v4(Ipv4Addr::BROADCAST), None);
    }
}

#[test]
fn v6_hit_and_gap() {
    let base = 0x2001_0db8_0123_4567_89ab_cdef_1020_3040;
    let table = db(
        &[],
        &[(0, 1, 0), (base, base + 9, 1), (u128::MAX, u128::MAX, 0)],
        false,
    );
    for (addr, expected) in [(0, 0), (1, 0), (base, 1), (base + 9, 1), (u128::MAX, 0)] {
        assert_eq!(table.lookup(IpAddr::V6(addr.into())), code(expected));
    }
    for addr in [2, base - 1, base + 10, u128::MAX - 1] {
        assert_eq!(table.lookup_v6(addr.into()), None);
    }
}

#[test]
fn ipv4_mapped_v6_uses_v4_table() {
    let v4 = Ipv4Addr::new(203, 0, 113, 7);
    let mapped = v4.to_ipv6_mapped();
    let table = db(&[(v4.into(), v4.into(), 0)], &[(0, u128::MAX, 1)], true);
    assert_eq!(table.lookup(IpAddr::V6(mapped)), code(0));
    assert_eq!(table.lookup_v6(mapped), code(0));
    assert_eq!(table.lookup_v6(Ipv4Addr::LOCALHOST.to_ipv6_mapped()), None);
    assert_eq!(table.lookup_v6(v4.to_ipv6_compatible()), code(1));
}

struct Generator(u32);

impl Generator {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

fn linear<T: Ord + Copy>(rows: &[Row<T>], addr: T) -> Option<CountryCode> {
    rows.iter()
        .find(|(start, end, _)| *start <= addr && addr <= *end)
        .and_then(|row| code(row.2))
}

#[test]
fn lookup_matches_linear_scan_on_10000_random_ranges() {
    let mut rng = Generator(0xbadc_0ffe);
    let mut end = 0;
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    let base = 0x2a00_0000_1111_2222_3333_4444_0000_0000_u128;
    for _ in 0..10_000 {
        let start = end + 1 + rng.next() % 120_000;
        end = start + rng.next() % 140_000;
        let country = u16::from(rng.next() & 1 == 1);
        v4.push((start, end, country));
        v6.push((base + u128::from(start), base + u128::from(end), country));
    }
    let table = db(&v4, &v6, true);
    for _ in 0..1_024 {
        let n = rng.next() % (end + 2);
        let addr = base + u128::from(n);
        assert_eq!(table.lookup_v4(n.into()), linear(&v4, n));
        assert_eq!(table.lookup_v6(addr.into()), linear(&v6, addr));
    }
}

#[test]
fn prefix_index_and_plain_search_agree() {
    let ranges = [
        (0x0000_0020, 0x0005_0000, 1),
        (0x0005_0003, 0x0005_0009, 0),
        (0x0008_fffe, 0x0009_0002, 1),
        (0xffff_0001, u32::MAX, 0),
    ];
    let indexed = db(&ranges, &[], true);
    let plain = db(&ranges, &[], false);
    for p in 0..=u16::MAX {
        for suffix in [0, 1, 0x7fff, 0xffff] {
            let addr = Ipv4Addr::from((u32::from(p) << 16) | suffix);
            assert_eq!(indexed.lookup_v4(addr), plain.lookup_v4(addr));
        }
    }
    for addr in [0x20, 0x0004_ffff, 0x0005_0000, 0x0008_ffff, 0x0009_0000] {
        assert_eq!(indexed.lookup_v4(addr.into()), code(1));
    }
}

#[test]
fn crc32_standard_check_vector() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32(&[]), 0);
}

#[test]
fn truncated_file_rejected() {
    let bytes = encode(&CODES, LABEL, &[(0, 7, 1)], &[(8, 14, 0)], false);
    for n in 0..bytes.len() {
        assert!(
            CountryDb::from_bytes(&bytes[..n]).is_err(),
            "accepted length {n}"
        );
    }
    let mut oversized = bytes;
    oversized.extend([0]);
    repair_crc(&mut oversized);
    assert!(matches!(
        CountryDb::from_bytes(&oversized),
        Err(CountryDbError::InvalidLength)
    ));
}

#[test]
fn bad_crc_rejected() {
    let mut bytes = encode(&CODES, LABEL, &[], &[], false);
    bytes[40] ^= 1;
    assert!(matches!(
        CountryDb::from_bytes(&bytes),
        Err(CountryDbError::CrcMismatch)
    ));
}

#[test]
fn unsorted_ranges_rejected() {
    let v4 = encode(&CODES, LABEL, &[(8, 10, 0), (0, 7, 1)], &[], false);
    let v6 = encode(&CODES, LABEL, &[], &[(8, 10, 0), (0, 7, 1)], false);
    for data in [v4, v6] {
        assert!(matches!(
            CountryDb::from_bytes(&data),
            Err(CountryDbError::UnsortedRanges { index: 1, .. })
        ));
    }
}

#[test]
fn overlapping_ranges_rejected() {
    for start in [4, 5, 6] {
        let v4 = encode(&CODES, LABEL, &[(4, 6, 0), (start, 9, 1)], &[], false);
        let v6 = encode(
            &CODES,
            LABEL,
            &[],
            &[(4, 6, 0), (start.into(), 9, 1)],
            false,
        );
        for data in [v4, v6] {
            assert!(matches!(
                CountryDb::from_bytes(&data),
                Err(CountryDbError::OverlappingRanges { index: 1, .. })
            ));
        }
    }
}

#[test]
fn country_index_out_of_range_rejected() {
    let v4 = encode(&CODES, LABEL, &[(0, 9, 2)], &[], false);
    let v6 = encode(&CODES, LABEL, &[], &[(0, 9, u16::MAX)], false);
    for data in [v4, v6] {
        assert!(matches!(
            CountryDb::from_bytes(&data),
            Err(CountryDbError::InvalidCountryIndex { index: 0, .. })
        ));
    }
}

#[test]
fn count_larger_than_file_rejected_without_large_allocation() {
    for offset in [24, 28, 32] {
        let mut bytes = encode(&CODES, LABEL, &[], &[], false);
        bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        repair_crc(&mut bytes);
        assert!(matches!(
            CountryDb::from_bytes(&bytes),
            Err(CountryDbError::InvalidLength)
        ));
    }
}

#[test]
fn reversed_ranges_rejected() {
    let v4 = encode(&CODES, LABEL, &[(11, 10, 0)], &[], false);
    let v6 = encode(&CODES, LABEL, &[], &[(11, 10, 0)], false);
    for data in [v4, v6] {
        assert!(matches!(
            CountryDb::from_bytes(&data),
            Err(CountryDbError::ReversedRange { index: 0, .. })
        ));
    }
}

#[test]
fn incorrect_prefix_offsets_rejected_even_when_monotone_and_in_bounds() {
    let ranges = [(5, 7, 0), (0x0001_0001, 0x0002_0010, 1)];
    let bytes = encode(&CODES, LABEL, &ranges, &[], true);
    let prefix_start = 48 + 2 * CODES.len() + 10 * ranges.len();
    for (p, offset) in [(0, 1_u32), (2, 0), (3, 3), (65535, u32::MAX)] {
        let mut bad = bytes.clone();
        let start = prefix_start + 4 * p;
        bad[start..start + 4].copy_from_slice(&offset.to_le_bytes());
        repair_crc(&mut bad);
        assert!(
            matches!(CountryDb::from_bytes(&bad), Err(CountryDbError::InvalidPrefix { prefix }) if prefix == p)
        );
    }
}

#[test]
fn magic_version_and_unknown_flags_rejected() {
    let original = encode(&CODES, LABEL, &[], &[], false);
    for (offset, value) in [(0, 1), (8, 2), (12, 2)] {
        let mut bytes = original.clone();
        bytes[offset] = value;
        repair_crc(&mut bytes);
        match offset {
            0 => assert!(matches!(
                CountryDb::from_bytes(&bytes),
                Err(CountryDbError::InvalidMagic)
            )),
            8 => assert!(matches!(
                CountryDb::from_bytes(&bytes),
                Err(CountryDbError::UnsupportedVersion(2))
            )),
            _ => assert!(matches!(
                CountryDb::from_bytes(&bytes),
                Err(CountryDbError::UnsupportedFlags(2))
            )),
        }
    }
}

#[test]
fn validates_label_and_country_codes() {
    let valid_label = vec![b'x'; 255];
    assert_eq!(
        CountryDb::from_bytes(&encode(&[], &valid_label, &[], &[], false))
            .unwrap()
            .info()
            .dataset_label
            .len(),
        255
    );
    assert!(matches!(
        CountryDb::from_bytes(&encode(&[], &[b'x'; 256], &[], &[], false)),
        Err(CountryDbError::LabelTooLong(256))
    ));
    assert!(matches!(
        CountryDb::from_bytes(&encode(&[], &[0xff], &[], &[], false)),
        Err(CountryDbError::InvalidLabel)
    ));
    for invalid in [*b"aE", *b" A", [0xfe, 0xff]] {
        let bytes = encode(&[*b"AE", invalid], LABEL, &[], &[], false);
        assert!(matches!(
            CountryDb::from_bytes(&bytes),
            Err(CountryDbError::InvalidCountryCode { index: 1 })
        ));
    }
}

#[test]
fn file_load_owns_data_and_reports_metadata() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CountryDb>();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("country-reader-{}-{nonce}", std::process::id()));
    let bytes = encode(&CODES, "tést".as_bytes(), &[(8, 8, 1)], &[], false);
    let mut file = std::fs::File::create_new(&path).unwrap();
    file.write_all(&bytes).unwrap();
    drop(file);
    let loaded = CountryDb::from_file(&path);
    std::fs::remove_file(&path).unwrap();
    let table = loaded.unwrap();
    assert_eq!(
        table.info(),
        DbInfo {
            build_time: BUILD_TIME,
            dataset_label: "tést",
            country_count: 2,
            ipv4_range_count: 1,
            ipv6_range_count: 0
        }
    );
    assert_eq!(table.lookup_v4(8.into()), code(1));
    assert!(matches!(
        CountryDb::from_file(&path),
        Err(CountryDbError::Io(_))
    ));
}
