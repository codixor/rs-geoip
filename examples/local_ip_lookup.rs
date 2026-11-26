use rs_geoip::GeoIpResolver;
use std::error::Error;
use std::net::IpAddr;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let packed_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ip-city-packed.lz4");
    if !packed_path.is_file() {
        eprintln!(
            "missing {}. Build or place the local packed artifact first.",
            packed_path.display()
        );
        return Ok(());
    }

    let resolver = GeoIpResolver::from_flat_packed_file(&packed_path)?;
    let ip: IpAddr = "1.0.64.1".parse()?;
    let fix = resolver.lookup_ip(ip)?;

    match fix {
        Some(fix) => {
            println!(
                "{} {} {} {} {}",
                ip,
                fix.city_name.as_deref().unwrap_or("<unknown-city>"),
                fix.country_code.as_deref().unwrap_or("<unknown-country>"),
                fix.point.latitude,
                fix.point.longitude
            );
        }
        None => {
            println!("{ip} <no-local-match>");
        }
    }

    Ok(())
}
