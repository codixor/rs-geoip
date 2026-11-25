//! Offline IP geolocation using local databases.

#[cfg(feature = "city")]
mod builtin;
#[cfg(feature = "city")]
mod error;
#[cfg(feature = "city")]
pub mod ip_city_pack;
#[cfg(feature = "city")]
mod local;

#[cfg(feature = "city")]
pub use builtin::{builtin_flat_packed_bytes, builtin_paged_packed_bytes};
#[cfg(feature = "city")]
pub use error::GeoIpError;
#[cfg(feature = "city")]
pub use local::{
    AccuracyKind, GeoFix, GeoIpFormat, GeoIpResolver, GeoSource, lookup_ip_in_builtin_flat_packed,
    lookup_ip_in_builtin_paged_packed, lookup_ip_in_flat_packed_bytes,
    lookup_ip_in_paged_packed_bytes,
};
#[cfg(all(feature = "city", not(target_arch = "wasm32")))]
pub use local::{
    lookup_ip_in_flat_packed_file, lookup_ip_in_paged_packed_file,
    lookup_ip_in_paged_packed_mmap_file,
};

#[cfg(feature = "city")]
/// Uniform latitude/longitude point returned by `rs-geoip`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}

#[cfg(feature = "city")]
impl GeoPoint {
    /// Returns this point as a tuple.
    #[must_use]
    pub fn as_tuple(self) -> (f64, f64) {
        (self.latitude, self.longitude)
    }
}
