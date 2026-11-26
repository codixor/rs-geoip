/// Returns the flat city pack embedded at build time, when available.
#[must_use]
pub fn builtin_flat_packed_bytes() -> Option<&'static [u8]> {
    #[cfg(has_flat_city_pack)]
    {
        Some(include_bytes!("../ip-city-packed.lz4"))
    }
    #[cfg(not(has_flat_city_pack))]
    {
        None
    }
}

/// Returns the paged city pack embedded at build time, when available.
#[must_use]
pub fn builtin_paged_packed_bytes() -> Option<&'static [u8]> {
    #[cfg(has_paged_city_pack)]
    {
        Some(include_bytes!("../ip-city-packed-paged.bin"))
    }
    #[cfg(not(has_paged_city_pack))]
    {
        None
    }
}
