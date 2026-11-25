use thiserror::Error;

/// Typed errors exposed by `rs-geoip`.
#[derive(Debug, Error)]
pub enum GeoIpError {
    #[error("provider returned invalid coordinates")]
    InvalidCoordinates,
    #[error("builtin geo-ip pack `{0}` is not available in this build")]
    BuiltinPackUnavailable(&'static str),
    #[error("failed to use the packed rs-geoip database")]
    PackedIp(#[from] crate::ip_city_pack::PackedIpCityError),
}
