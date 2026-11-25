use rs_geoip::ip_city_pack::PagedPackedIpCityLookup;
use serde::Serialize;
use std::error::Error;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::str::FromStr;

type DynError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug)]
struct Args {
    input_bin: PathBuf,
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
    let lookup = PagedPackedIpCityLookup::from_file(&args.input_bin)?;
    let Some(found) = lookup.find_ipv4(args.ip) else {
        eprintln!("no match for {}", args.ip);
        std::process::exit(2);
    };

    let city = found.city();
    let output = LookupOutput {
        city_id: found.city_id,
        range_start: found.range_start.to_string(),
        range_end: found.range_end.to_string(),
        city_name: &city.city_name,
        country_code: &city.country_code,
        latitude: city.latitude,
        longitude: city.longitude,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

impl Args {
    fn parse<I>(mut args: I) -> Result<Self, DynError>
    where
        I: Iterator<Item = String>,
    {
        let mut input_bin = None;
        let mut ip = None;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--input-bin" => input_bin = args.next().map(PathBuf::from),
                "--ip" => ip = args.next(),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unexpected argument: {other}").into()),
            }
        }

        Ok(Self {
            input_bin: input_bin
                .ok_or_else(|| "missing required --input-bin <path> argument".to_string())?,
            ip: Ipv4Addr::from_str(
                &ip.ok_or_else(|| "missing required --ip <ipv4> argument".to_string())?,
            )?,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --features city,build --bin lookup_ip_city_pack_paged -- \\\n    --input-bin <paged output.bin> \\\n    --ip <ipv4>"
    );
}
