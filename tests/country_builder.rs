#![cfg(feature = "build")]

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use rs_geoip::country::{CountryCode, CountryDb};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let dir = std::env::temp_dir().join(format!(
                "rs-geo-country-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&dir) {
                Ok(()) => return Self(dir),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => panic!("creating test directory: {e}"),
            }
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn build(&self, v4: &str, v6: &str) -> Output {
        fs::write(self.path("v4.csv"), v4).unwrap();
        fs::write(self.path("v6.csv"), v6).unwrap();
        self.run([
            "--ipv4",
            "v4.csv",
            "--ipv6",
            "v6.csv",
            "--label",
            "test country",
            "--out",
            "country.bin",
        ])
    }

    fn run(&self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Output {
        Command::new(env!("CARGO_BIN_EXE_rs-geo-country-build"))
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }

    fn db(&self) -> CountryDb {
        CountryDb::from_file(self.path("country.bin")).unwrap()
    }

    fn assert_no_build_temps(&self) {
        for entry in fs::read_dir(&self.0).unwrap() {
            assert_ne!(entry.unwrap().path().extension(), Some(OsStr::new("tmp")));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn success(output: &Output) -> &str {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::str::from_utf8(&output.stdout).unwrap()
}

fn failure(output: &Output) -> &str {
    assert!(
        !output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    std::str::from_utf8(&output.stderr).unwrap()
}

#[test]
fn builder_merges_adjacent_ranges() {
    let f = Fixture::new();
    let built = f.build(
        "7,8,CA\n2,3,US\n0,1,US\n4,4,US\n6,6,US\n",
        "8,10,AE\n2,7,AE\n",
    );
    let log = success(&built);
    assert!(log.contains("IPv4: 5 rows, 3 merged ranges"));
    assert!(log.contains("IPv6: 2 rows, 1 merged ranges"));
    let db = f.db();
    assert_eq!(db.info().ipv4_range_count, 3);
    assert_eq!(db.info().ipv6_range_count, 1);
    assert_eq!(db.lookup_v4(Ipv4Addr::from(5)), None);
    assert_eq!(
        db.lookup_v4(Ipv4Addr::from(6)),
        CountryCode::from_ascii("US")
    );
    assert_eq!(
        db.lookup_v4(Ipv4Addr::from(7)),
        CountryCode::from_ascii("CA")
    );
}

#[test]
fn builder_rejects_overlap_with_line_numbers() {
    let f = Fixture::new();
    // Lines 4 and 1 conflict after sorting. Reporting a prior merge's first line (3) is wrong.
    let out = f.build("22,25,CA\r\n\r\n10,15,US\r\n16,22,US", "");
    let message = failure(&out);
    assert!(
        message.contains("v4.csv") && message.contains("lines 4 and 1"),
        "{message}"
    );
    assert!(!f.path("country.bin").exists());
    let out = f.build("", "1,5,AE\n5,7,AE\n");
    assert!(failure(&out).contains("lines 1 and 2"));
}

#[test]
fn builder_output_loads_and_matches_input_on_samples() {
    let f = Fixture::new();
    let v4 = [
        (2, 210_000, "AE"),
        (210_001, 210_010, "US"),
        (251_100, 510_000, "CA"),
    ];
    let v6 = [
        (0x1234u128 << 64, (0x1234u128 << 64) + 120_000, "FR"),
        (2, 35, "US"),
    ];
    let mut v4_csv = String::new();
    for &(start, end, code) in v4.iter().rev() {
        writeln!(
            v4_csv,
            "{},{},\"{code}\"\r",
            Ipv4Addr::from(start),
            Ipv4Addr::from(end)
        )
        .unwrap();
    }
    let mut v6_csv = String::new();
    for &(start, end, code) in &v6 {
        writeln!(
            v6_csv,
            "{},{},{code}",
            Ipv6Addr::from(start),
            Ipv6Addr::from(end)
        )
        .unwrap();
    }
    success(&f.build(&v4_csv, &v6_csv));
    let db = f.db();
    assert_eq!(db.info().dataset_label, "test country");
    assert_eq!(db.info().country_count, 4);
    for n in (0..550_000).step_by(293).chain([
        0,
        2,
        65_535,
        65_536,
        210_000,
        210_001,
        210_010,
        210_011,
        251_100,
        510_000,
        510_001,
        u32::MAX,
    ]) {
        let expected = v4
            .iter()
            .find(|&&(s, e, _)| (s..=e).contains(&n))
            .and_then(|r| CountryCode::from_ascii(r.2));
        assert_eq!(db.lookup_v4(Ipv4Addr::from(n)), expected, "{n}");
    }
    for &(start, end, code) in &v6 {
        for n in [start - 1, start, start.midpoint(end), end, end + 1] {
            let expected = (start..=end)
                .contains(&n)
                .then(|| CountryCode::from_ascii(code).unwrap());
            assert_eq!(db.lookup_v6(Ipv6Addr::from(n)), expected, "{n}");
        }
    }
    let checked = f.run(["check", "country.bin"]);
    let info = success(&checked);
    assert!(
        info.contains("test country")
            && info.contains("IPv4 ranges: 3")
            && info.contains("IPv6 ranges: 2")
    );
    fs::write(f.path("country.bin"), b"damaged country file").unwrap();
    failure(&f.run(["check", "country.bin"]));
}

#[test]
fn empty_ipv6_input_is_allowed() {
    let f = Fixture::new();
    success(&f.build("1,1,US", ""));
    let db = f.db();
    assert_eq!(db.info().ipv6_range_count, 0);
    assert_eq!(db.lookup_v6(Ipv6Addr::LOCALHOST), None);
}

#[test]
fn maximum_addresses_merge_without_wrapping() {
    let f = Fixture::new();
    let (max4, max6) = (u32::MAX, u128::MAX);
    success(&f.build(
        &format!("{max4},{max4},AE\n0,0,AE\n{}, {},AE\n", max4 - 2, max4 - 1),
        &format!("{max6},{max6},CA\n{}, {},CA\n0,0,CA\n", max6 - 2, max6 - 1),
    ));
    let db = f.db();
    assert_eq!(db.info().ipv4_range_count, 2);
    assert_eq!(db.info().ipv6_range_count, 2);
    assert_eq!(
        db.lookup_v4(Ipv4Addr::from(max4)),
        CountryCode::from_ascii("AE")
    );
    assert_eq!(
        db.lookup_v6(Ipv6Addr::from(max6)),
        CountryCode::from_ascii("CA")
    );
}

#[test]
fn repeated_build_replaces_only_after_success() {
    let f = Fixture::new();
    success(&f.build("0,1,AE", ""));
    let old = f.db();
    success(&f.build("1,3,US", ""));
    assert_eq!(
        old.lookup_v4(Ipv4Addr::from(1)),
        CountryCode::from_ascii("AE")
    );
    assert_eq!(
        f.db().lookup_v4(Ipv4Addr::from(1)),
        CountryCode::from_ascii("US")
    );
    let installed = fs::read(f.path("country.bin")).unwrap();
    for bad in [
        "4,5,US\n5,7,US",
        "4294967296,4294967296,US",
        "::1,::1,AE",
        "5,4,US",
        "0,1,U1",
        "0,1,US,extra",
        "0,1",
    ] {
        failure(&f.build(bad, ""));
        assert_eq!(fs::read(f.path("country.bin")).unwrap(), installed);
    }
    failure(&f.build("", "1,340282366920938463463374607431768211456,US"));
    assert_eq!(fs::read(f.path("country.bin")).unwrap(), installed);
    f.assert_no_build_temps();
}

#[test]
fn failed_rename_cleans_up_temporary_file() {
    let f = Fixture::new();
    fs::create_dir(f.path("country.bin")).unwrap();
    failure(&f.build("1,2,US", ""));
    assert!(Path::is_dir(&f.path("country.bin")));
    f.assert_no_build_temps();
}

#[cfg(windows)]
#[test]
fn locked_destination_survives_failed_replacement() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = Fixture::new();
    success(&f.build("1,2,US", ""));
    let installed = fs::read(f.path("country.bin")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(f.path("country.bin"))
        .unwrap();
    failure(&f.build("1,2,AE", ""));
    drop(lock);
    assert_eq!(fs::read(f.path("country.bin")).unwrap(), installed);
    f.assert_no_build_temps();
}
