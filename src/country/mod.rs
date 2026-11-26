//! A read-only, in-memory country lookup for IPv4 and IPv6.
//!
//! Loading validates the entire little-endian RSGEOCT1 file before returning a
//! database. Lookups borrow the resulting arrays and need no allocation or lock.
#![forbid(unsafe_code)]

mod error;
mod format;
#[cfg(test)]
mod tests;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Range;
use std::path::Path;

pub use error::CountryDbError;
#[doc(hidden)]
pub use format::{IPV4_PREFIX_FLAG, MAGIC, PREFIX_COUNT, VERSION, crc32};

/// A two-letter, uppercase ASCII country code.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CountryCode([u8; 2]);

impl CountryCode {
    /// Returns the uppercase spelling of this code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Both constructors accept only ASCII letters.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// Accepts exactly two ASCII letters in either case.
    #[must_use]
    pub fn from_ascii(code: &str) -> Option<Self> {
        let mut bytes: [u8; 2] = code.as_bytes().try_into().ok()?;
        bytes.make_ascii_uppercase();
        bytes
            .iter()
            .all(u8::is_ascii_uppercase)
            .then_some(Self(bytes))
    }
}

/// Metadata recorded in the table header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DbInfo<'a> {
    /// Build time in seconds since the Unix epoch.
    pub build_time: u64,
    /// UTF-8 dataset description supplied by the builder.
    pub dataset_label: &'a str,
    /// Number of entries in the country code table.
    pub country_count: usize,
    /// Number of IPv4 ranges in the table.
    pub ipv4_range_count: usize,
    /// Number of IPv6 ranges in the table.
    pub ipv6_range_count: usize,
}

#[derive(Debug)]
struct Ranges<T> {
    starts: Vec<T>,
    ends: Vec<T>,
    countries: Vec<u16>,
}

impl<T: Ord + Copy> Ranges<T> {
    fn lookup(&self, ip: T, search: Range<usize>) -> Option<u16> {
        let first = search.start;
        let after = self.starts[search].partition_point(|start| *start <= ip);
        let i = first + after.checked_sub(1)?;
        (ip <= self.ends[i]).then_some(self.countries[i])
    }
}

/// A validated country table that can be shared across threads.
#[derive(Debug)]
pub struct CountryDb {
    build_time: u64,
    label: String,
    codes: Vec<CountryCode>,
    v4: Ranges<u32>,
    prefix: Option<Vec<u32>>,
    v6: Ranges<u128>,
}

impl CountryDb {
    /// Reads a local file and validates its entire contents.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file cannot be read, or a format error
    /// if its contents fail validation.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, CountryDbError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// Loads a table into owned arrays. The input buffer can then be discarded.
    ///
    /// # Errors
    ///
    /// Returns a typed error if the header, length, checksum, codes, ranges
    /// or prefix index are invalid.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CountryDbError> {
        format::decode(bytes)
    }

    /// Looks up either address family. IPv4-mapped IPv6 uses the IPv4 table.
    #[must_use]
    pub fn lookup(&self, ip: IpAddr) -> Option<CountryCode> {
        match ip {
            IpAddr::V4(addr) => self.lookup_v4(addr),
            IpAddr::V6(addr) => self.lookup_v6(addr),
        }
    }

    /// Looks up an IPv4 address. Gaps return `None`.
    #[must_use]
    pub fn lookup_v4(&self, ip: Ipv4Addr) -> Option<CountryCode> {
        let addr = u32::from(ip);
        let count = self.v4.starts.len();
        let search = self.prefix.as_ref().map_or(0..count, |prefix| {
            let p = (addr >> 16) as usize;
            // The first range reaching the *next* /16 might also cover the
            // address in this /16. Include it in the binary search.
            let end = prefix
                .get(p + 1)
                .map_or(count, |&next| (next as usize).saturating_add(1).min(count));
            prefix[p] as usize..end
        });
        self.v4
            .lookup(addr, search)
            .map(|i| self.codes[usize::from(i)])
    }

    /// Looks up an IPv6 address, routing IPv4-mapped addresses to IPv4.
    #[must_use]
    pub fn lookup_v6(&self, ip: Ipv6Addr) -> Option<CountryCode> {
        if let Some(v4) = ip.to_ipv4_mapped() {
            return self.lookup_v4(v4);
        }
        self.v6
            .lookup(u128::from(ip), 0..self.v6.starts.len())
            .map(|i| self.codes[usize::from(i)])
    }

    /// Returns the table's metadata and array sizes.
    #[must_use]
    pub fn info(&self) -> DbInfo<'_> {
        DbInfo {
            build_time: self.build_time,
            dataset_label: &self.label,
            country_count: self.codes.len(),
            ipv4_range_count: self.v4.starts.len(),
            ipv6_range_count: self.v6.starts.len(),
        }
    }
}
