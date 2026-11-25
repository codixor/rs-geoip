use rs_geoip::ip_city_pack::{PackedIpCityDb, PagedPackedIpCityDb};
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

type DynError = Box<dyn Error + Send + Sync + 'static>;

fn main() -> Result<(), DynError> {
    let args = Args::parse(std::env::args().skip(1))?;
    let flat = PackedIpCityDb::decode_lz4(&std::fs::read(&args.input_lz4)?)?;
    let paged = PagedPackedIpCityDb::from_flat(&flat, args.block_size)?;
    let bytes = paged.encode_bytes()?;

    let mut writer = BufWriter::new(File::create(&args.output_bin)?);
    writer.write_all(&bytes)?;
    writer.flush()?;

    eprintln!(
        "built paged pack {} -> {} (cities={}, ranges={}, blocks={}, block_size={}, bytes={})",
        args.input_lz4.display(),
        args.output_bin.display(),
        paged.city_count,
        paged.range_count,
        paged.block_count,
        paged.block_size,
        bytes.len()
    );

    Ok(())
}

struct Args {
    input_lz4: PathBuf,
    output_bin: PathBuf,
    block_size: usize,
}

impl Args {
    fn parse<I>(mut args: I) -> Result<Self, DynError>
    where
        I: Iterator<Item = String>,
    {
        let mut input_lz4 = None;
        let mut output_bin = None;
        let mut block_size = 256usize;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--input-lz4" => input_lz4 = args.next().map(PathBuf::from),
                "--output-bin" => output_bin = args.next().map(PathBuf::from),
                "--block-size" => {
                    block_size = args
                        .next()
                        .ok_or_else(|| "missing value for --block-size".to_string())?
                        .parse()?;
                }
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
            output_bin: output_bin
                .ok_or_else(|| "missing required --output-bin <path> argument".to_string())?,
            block_size,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage:\n  cargo run --features city,build --bin build_ip_city_pack_paged -- \\\n    --input-lz4 <flat packed output.lz4> \\\n    --output-bin <paged output.bin> \\\n    [--block-size <n>]"
    );
}
