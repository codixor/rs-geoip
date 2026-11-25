#[cfg(not(target_arch = "wasm32"))]
use crate::ip_city_pack::MmapPagedPackedIpCityLookup;
use crate::{
    GeoIpError, GeoPoint,
    ip_city_pack::{PackedIpCityLookup, PagedPackedIpCityLookup},
};
use std::net::{IpAddr, Ipv4Addr};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;

/// Distinguishes the packed local lookup formats supported by `rs-geoip`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoIpFormat {
    FlatPackedLz4,
    PagedPacked,
}

/// Identifies where a [`GeoFix`] originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoSource {
    PackedLz4,
    PackedPaged,
    PackedPagedMmap,
}

/// Describes how precise a location fix should be interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccuracyKind {
    ApproximateIp,
}

/// Richer optional result surface for additive fallback layers.
#[derive(Debug, Clone, PartialEq)]
pub struct GeoFix {
    pub point: GeoPoint,
    pub source: GeoSource,
    pub accuracy_kind: AccuracyKind,
    pub city_name: Option<String>,
    pub country_code: Option<String>,
}

/// Optional local packed-IP resolver for approximate geolocation.
#[derive(Debug)]
pub struct GeoIpResolver {
    inner: GeoIpResolverInner,
}

#[derive(Debug)]
enum GeoIpResolverInner {
    Flat(PackedIpCityLookup),
    Paged(PagedPackedIpCityLookup),
    #[cfg(not(target_arch = "wasm32"))]
    PagedMmap(MmapPagedPackedIpCityLookup),
}

impl GeoIpResolver {
    /// Opens the flat packed artifact staged by `build.rs` into `OUT_DIR`.
    ///
    /// This is convenient for wasm and other embed-first targets that want to
    /// ship the built-in packed bytes without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the built-in artifact is unavailable in this
    /// build or the payload cannot be decoded.
    pub fn from_builtin_flat_packed() -> Result<Self, GeoIpError> {
        let bytes =
            crate::builtin_flat_packed_bytes().ok_or(GeoIpError::BuiltinPackUnavailable("flat"))?;
        Self::from_flat_packed_bytes(bytes)
    }

    /// Opens the paged packed artifact staged by `build.rs` into `OUT_DIR`.
    ///
    /// This is convenient for wasm and other embed-first targets that want to
    /// ship the built-in paged bytes without touching the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the built-in artifact is unavailable in this
    /// build or the payload cannot be decoded.
    pub fn from_builtin_paged_packed() -> Result<Self, GeoIpError> {
        let bytes = crate::builtin_paged_packed_bytes()
            .ok_or(GeoIpError::BuiltinPackUnavailable("paged"))?;
        Self::from_paged_packed_bytes(bytes)
    }

    /// Opens the flat packed `lz4_flex` artifact from bytes.
    ///
    /// This constructor is wasm-safe and is the preferred entrypoint when the
    /// packed database is bundled or fetched separately from the filesystem.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the payload cannot be decoded.
    pub fn from_flat_packed_bytes(bytes: &[u8]) -> Result<Self, GeoIpError> {
        let lookup = PackedIpCityLookup::from_lz4_bytes(bytes)?;
        Ok(Self {
            inner: GeoIpResolverInner::Flat(lookup),
        })
    }

    /// Opens the flat packed `lz4_flex` artifact.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the file cannot be read or decoded.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_flat_packed_file(path: impl AsRef<Path>) -> Result<Self, GeoIpError> {
        let lookup = PackedIpCityLookup::from_lz4_file(path)?;
        Ok(Self {
            inner: GeoIpResolverInner::Flat(lookup),
        })
    }

    /// Opens the paged packed artifact from bytes.
    ///
    /// This constructor is wasm-safe and keeps the lazy per-block decode logic
    /// without requiring filesystem access.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the payload cannot be decoded.
    pub fn from_paged_packed_bytes(bytes: &[u8]) -> Result<Self, GeoIpError> {
        let lookup = PagedPackedIpCityLookup::from_bytes(bytes)?;
        Ok(Self {
            inner: GeoIpResolverInner::Paged(lookup),
        })
    }

    /// Opens the paged packed artifact with lazy city-block decode.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the file cannot be read or decoded.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_paged_packed_file(path: impl AsRef<Path>) -> Result<Self, GeoIpError> {
        let lookup = PagedPackedIpCityLookup::from_file(path)?;
        Ok(Self {
            inner: GeoIpResolverInner::Paged(lookup),
        })
    }

    /// Opens the paged packed artifact using the `rs-mmap` backend.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the mapped file is not modified or truncated
    /// while the resolver exists.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the mapped file cannot be opened or the
    /// paged packed database cannot be decoded.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(unsafe_code)]
    pub unsafe fn from_paged_packed_mmap_file(path: impl AsRef<Path>) -> Result<Self, GeoIpError> {
        // SAFETY: The caller guarantees the mapped file remains immutable and
        // not truncated while the resolver exists.
        let lookup = unsafe { MmapPagedPackedIpCityLookup::from_mmap_file(path)? };
        Ok(Self {
            inner: GeoIpResolverInner::PagedMmap(lookup),
        })
    }

    /// Returns the packed format backing this resolver.
    #[must_use]
    pub fn format(&self) -> GeoIpFormat {
        match self.inner {
            GeoIpResolverInner::Flat(_) => GeoIpFormat::FlatPackedLz4,
            GeoIpResolverInner::Paged(_) => GeoIpFormat::PagedPacked,
            #[cfg(not(target_arch = "wasm32"))]
            GeoIpResolverInner::PagedMmap(_) => GeoIpFormat::PagedPacked,
        }
    }

    /// Resolves an IP address through the local packed fallback.
    ///
    /// IPv6 currently returns `Ok(None)` because the current imported dataset is
    /// IPv4-only.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the packed reader fails.
    pub fn lookup_ip(&self, ip: IpAddr) -> Result<Option<GeoFix>, GeoIpError> {
        match ip {
            IpAddr::V4(ip) => self.lookup_ipv4(ip),
            IpAddr::V6(_) => Ok(None),
        }
    }

    /// Resolves an IPv4 address through the local packed fallback.
    ///
    /// # Errors
    ///
    /// Returns [`GeoIpError`] when the packed reader fails.
    pub fn lookup_ipv4(&self, ip: Ipv4Addr) -> Result<Option<GeoFix>, GeoIpError> {
        let fix = match &self.inner {
            GeoIpResolverInner::Flat(lookup) => lookup
                .find_ipv4(ip)
                .map(|found| fix_from_city(found.city, GeoSource::PackedLz4))
                .transpose()?,
            GeoIpResolverInner::Paged(lookup) => lookup
                .find_ipv4(ip)
                .map(|found| fix_from_city(found.city(), GeoSource::PackedPaged))
                .transpose()?,
            #[cfg(not(target_arch = "wasm32"))]
            GeoIpResolverInner::PagedMmap(lookup) => lookup
                .find_ipv4(ip)
                .map(|found| fix_from_city(found.city(), GeoSource::PackedPagedMmap))
                .transpose()?,
        };

        Ok(fix)
    }
}

/// Convenience helper for one-off flat packed lookups.
///
/// This helper is wasm-safe because it works from already-available bytes.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the packed payload cannot be decoded.
pub fn lookup_ip_in_flat_packed_bytes(
    bytes: &[u8],
    ip: IpAddr,
) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_flat_packed_bytes(bytes)?.lookup_ip(ip)
}

/// Convenience helper for one-off built-in flat packed lookups.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the built-in packed payload is unavailable or
/// cannot be decoded.
pub fn lookup_ip_in_builtin_flat_packed(ip: IpAddr) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_builtin_flat_packed()?.lookup_ip(ip)
}

/// Convenience helper for one-off flat packed file lookups.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the packed file cannot be read or decoded.
#[cfg(not(target_arch = "wasm32"))]
pub fn lookup_ip_in_flat_packed_file(
    path: impl AsRef<Path>,
    ip: IpAddr,
) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_flat_packed_file(path)?.lookup_ip(ip)
}

/// Convenience helper for one-off paged packed lookups from bytes.
///
/// This helper is wasm-safe because it works from already-available bytes.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the packed payload cannot be decoded.
pub fn lookup_ip_in_paged_packed_bytes(
    bytes: &[u8],
    ip: IpAddr,
) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_paged_packed_bytes(bytes)?.lookup_ip(ip)
}

/// Convenience helper for one-off built-in paged packed lookups.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the built-in packed payload is unavailable or
/// cannot be decoded.
pub fn lookup_ip_in_builtin_paged_packed(ip: IpAddr) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_builtin_paged_packed()?.lookup_ip(ip)
}

/// Convenience helper for one-off paged packed file lookups.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the packed file cannot be read or decoded.
#[cfg(not(target_arch = "wasm32"))]
pub fn lookup_ip_in_paged_packed_file(
    path: impl AsRef<Path>,
    ip: IpAddr,
) -> Result<Option<GeoFix>, GeoIpError> {
    GeoIpResolver::from_paged_packed_file(path)?.lookup_ip(ip)
}

/// Convenience helper for one-off paged packed mmap lookups.
///
/// # Safety
///
/// The caller must ensure that the mapped file is not modified or truncated
/// while the lookup is in progress.
///
/// # Errors
///
/// Returns [`GeoIpError`] when the packed file cannot be opened, mapped, or
/// decoded.
#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
pub unsafe fn lookup_ip_in_paged_packed_mmap_file(
    path: impl AsRef<Path>,
    ip: IpAddr,
) -> Result<Option<GeoFix>, GeoIpError> {
    // SAFETY: The caller guarantees the mapped file remains immutable and not
    // truncated for the duration of this one-off lookup.
    unsafe { GeoIpResolver::from_paged_packed_mmap_file(path)? }.lookup_ip(ip)
}

fn fix_from_city(
    city: &crate::ip_city_pack::PackedCityRecord,
    source: GeoSource,
) -> Result<GeoFix, GeoIpError> {
    Ok(GeoFix {
        point: checked_point(city.latitude, city.longitude)?,
        source,
        accuracy_kind: AccuracyKind::ApproximateIp,
        city_name: normalize_field(&city.city_name),
        country_code: normalize_field(&city.country_code),
    })
}

fn checked_point(latitude: f64, longitude: f64) -> Result<GeoPoint, GeoIpError> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err(GeoIpError::InvalidCoordinates);
    }

    Ok(GeoPoint {
        latitude,
        longitude,
    })
}

fn normalize_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
