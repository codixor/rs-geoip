use rs_geoip::ip_city_pack::PackedIpCityLookup;
use serde::Serialize;
use std::error::Error;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

type DynError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug)]
struct Args {
    input_lz4: PathBuf,
    ip: Ipv4Addr,
}

#[derive(Debug, Serialize)]
struct LookupOutput<'a> {
    city_id: u32,
    range_start: String,
    range_end: String,
    city_name: &'a str,
    country_code: &'a str,
    latitude: f64,
    longitude: f64,
}

fn main() -> Result<(), DynError> {
    let args = Args::parse(std::env::args().skip(1))?;
    let lookup = PackedIpCityLookup::from_lz4_file(&args.input_lz4)?;
    let Some(found) = lookup.find_ipv4(args.ip) else {
        eprintln!("no match for {}", args.ip);
        std::process::exit(2);
    };

    let output = LookupOutput {
        city_id: found.city_id,
        range_start: found.range_start.to_string(),
        range_end: found.range_end.to_string(),
        city_name: &found.city.city_name,
        country_code: &found.city.country_code,
        latitude: found.city.latitude,
        longitude: found.city.longitude,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

impl Args {
    fn parse<I>(mut args: I) -> Result<Self, DynError>
    where
        I: Iterator<Item = String>,
    {
        let mut input_lz4 = None;
        let mut ip = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--input-lz4" => input_lz4 = args.next().map(PathBuf::from),
                "--ip" => ip = args.next(),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unexpected argument: {other}").into()),
            }
        }

        Ok(Self {
            input_lz4: input_lz4
                .ok_or_else(|| "missing required --input-lz4 <path> argument".to_string())?,
            ip: Ipv4Addr::from_str(
                &ip.ok_or_else(|| "missing required --ip <ipv4> argument".to_string())?,
            )?,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --features city,build --bin lookup_ip_city_pack -- \\\n    --input-lz4 <packed output.lz4> \\\n    --ip <ipv4>"
    );
}
