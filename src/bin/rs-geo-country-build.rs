use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use rs_geoip::country::{
    CountryCode, CountryDb, IPV4_PREFIX_FLAG, MAGIC, PREFIX_COUNT, VERSION, crc32,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "rs-geo-country-build --ipv4 FILE --ipv6 FILE --label LABEL --out FILE
rs-geo-country-build check FILE";

struct Options {
    ipv4: PathBuf,
    ipv6: PathBuf,
    label: String,
    out: PathBuf,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self> {
        let (mut ipv4, mut ipv6, mut label, mut out) = (None, None, None, None);
        while let Some(flag) = args.next() {
            let slot = match flag.to_str() {
                Some("--ipv4") => &mut ipv4,
                Some("--ipv6") => &mut ipv6,
                Some("--label") => &mut label,
                Some("--out") => &mut out,
                _ => return Err(format!("unknown option {}\n{USAGE}", flag.display()).into()),
            };
            if slot.is_some() {
                return Err(format!("repeated option {}", flag.display()).into());
            }
            *slot = Some(
                args.next()
                    .ok_or_else(|| format!("missing value for {}", flag.display()))?,
            );
        }
        let label = label
            .ok_or("missing --label")?
            .into_string()
            .map_err(|_| "label must be UTF-8")?;
        if label.len() > 255 {
            return Err("dataset label exceeds 255 bytes".into());
        }
        Ok(Self {
            ipv4: ipv4.ok_or("missing --ipv4")?.into(),
            ipv6: ipv6.ok_or("missing --ipv6")?.into(),
            label,
            out: out.ok_or("missing --out")?.into(),
        })
    }
}

#[derive(Default)]
struct Codes {
    entries: Vec<CountryCode>,
    indices: HashMap<CountryCode, u16>,
}

impl Codes {
    fn intern(&mut self, code: CountryCode) -> Result<u16> {
        if let Some(index) = self.indices.get(&code) {
            return Ok(*index);
        }
        let index = u16::try_from(self.entries.len())?;
        self.entries.push(code);
        self.indices.insert(code, index);
        Ok(index)
    }
}

struct Range<T> {
    start: T,
    end: T,
    code: u16,
    line: u64,
}

struct Table<T> {
    ranges: Vec<Range<T>>,
    input_rows: usize,
}

fn parse_v4(field: &str) -> Option<u32> {
    if field.bytes().all(|b| b.is_ascii_digit()) {
        field.parse().ok()
    } else {
        field.parse::<Ipv4Addr>().ok().map(u32::from)
    }
}

fn parse_v6(field: &str) -> Option<u128> {
    if field.bytes().all(|b| b.is_ascii_digit()) {
        field.parse().ok()
    } else {
        field.parse::<Ipv6Addr>().ok().map(u128::from)
    }
}

fn read_table<T: Ord + Copy>(
    path: &Path,
    codes: &mut Codes,
    parse: fn(&str) -> Option<T>,
    successor: fn(T) -> Option<T>,
) -> Result<Table<T>> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // End every record with LF, including the last one. The reader's ending line then
    // identifies the physical row even if it skipped blank lines before that record.
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .terminator(csv::Terminator::Any(b'\n'))
        .from_reader(file.chain(&b"\n"[..]));
    let mut record = csv::StringRecord::new();
    let mut ranges = Vec::new();
    while reader
        .read_record(&mut record)
        .map_err(|e| format!("{}: {e}", path.display()))?
    {
        let line = reader.position().line() - 1;
        let invalid = |why: &str| format!("{}: line {line}: {why}", path.display());
        if record.len() == 1 && &record[0] == "\r" {
            continue; // Blank CRLF line.
        }
        if record.len() != 3 {
            return Err(invalid("expected start, end and country (three CSV fields)").into());
        }
        if record.iter().any(|field| field.contains('\n')) {
            return Err(invalid("unexpected newline in CSV field").into());
        }
        let start = parse(record[0].trim()).ok_or_else(|| invalid("invalid start address"))?;
        let end = parse(record[1].trim()).ok_or_else(|| invalid("invalid end address"))?;
        if start > end {
            return Err(invalid("start address exceeds end address").into());
        }
        let code = CountryCode::from_ascii(record[2].trim())
            .ok_or_else(|| invalid("invalid country code"))?;
        ranges.push(Range {
            start,
            end,
            code: codes.intern(code)?,
            line,
        });
    }
    ranges.sort_unstable_by_key(|r| (r.start, r.line));
    // Detect conflicts on original rows, so a merged run cannot hide the source line.
    for pair in ranges.windows(2) {
        if pair[1].start <= pair[0].end {
            return Err(format!(
                "{}: overlapping ranges on lines {} and {}",
                path.display(),
                pair[0].line,
                pair[1].line
            )
            .into());
        }
    }
    let input_rows = ranges.len();
    ranges.dedup_by(|next, previous| {
        if previous.code == next.code && successor(previous.end) == Some(next.start) {
            previous.end = next.end;
            true
        } else {
            false
        }
    });
    Ok(Table { ranges, input_rows })
}

fn encode_ranges<T: Copy, const WIDTH: usize>(
    bytes: &mut Vec<u8>,
    ranges: &[Range<T>],
    little_endian: fn(T) -> [u8; WIDTH],
) {
    for r in ranges {
        bytes.extend_from_slice(&little_endian(r.start));
    }
    for r in ranges {
        bytes.extend_from_slice(&little_endian(r.end));
    }
    for r in ranges {
        bytes.extend_from_slice(&r.code.to_le_bytes());
    }
}

fn encode(label: &str, codes: &Codes, v4: &Table<u32>, v6: &Table<u128>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&IPV4_PREFIX_FLAG.to_le_bytes());
    bytes.extend_from_slice(
        &SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_le_bytes(),
    );
    for count in [
        codes.entries.len(),
        v4.ranges.len(),
        v6.ranges.len(),
        label.len(),
    ] {
        bytes.extend_from_slice(&u32::try_from(count)?.to_le_bytes());
    }
    bytes.extend_from_slice(label.as_bytes());
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    for code in &codes.entries {
        bytes.extend_from_slice(code.as_str().as_bytes());
    }
    encode_ranges(&mut bytes, &v4.ranges, u32::to_le_bytes);
    let mut index = 0;
    for prefix in 0..u32::try_from(PREFIX_COUNT)? {
        let first_address = prefix << 16;
        while index < v4.ranges.len() && v4.ranges[index].end < first_address {
            index += 1;
        }
        bytes.extend_from_slice(&u32::try_from(index)?.to_le_bytes());
    }
    encode_ranges(&mut bytes, &v6.ranges, u128::to_le_bytes);
    bytes.extend_from_slice(&crc32(&bytes).to_le_bytes());
    Ok(bytes)
}

struct TemporaryFile {
    path: PathBuf,
    renamed: bool,
}

impl TemporaryFile {
    fn create(parent: &Path, output_name: &OsStr) -> io::Result<(Self, File)> {
        // Distinct first characters also prevent aliasing a user-supplied destination
        // on case-insensitive filesystems.
        let prefix = if output_name.as_encoded_bytes().starts_with(b".") {
            "_"
        } else {
            "."
        };
        for attempt in 0..u64::MAX {
            let path = parent.join(format!(
                "{prefix}rs-geo-country-{}-{attempt}.tmp",
                std::process::id()
            ));
            match File::create_new(&path) {
                Ok(file) => {
                    return Ok((
                        Self {
                            path,
                            renamed: false,
                        },
                        file,
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::other("unable to reserve a temporary filename"))
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn install(bytes: &[u8], destination: &Path) -> Result<()> {
    let name = destination.file_name().ok_or("missing output filename")?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    #[cfg(unix)]
    let parent_handle = File::open(parent)?;
    let (mut temp, mut file) = TemporaryFile::create(parent, name)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    CountryDb::from_file(&temp.path)?;
    fs::rename(&temp.path, destination)?;
    temp.renamed = true;
    #[cfg(unix)]
    parent_handle
        .sync_all()
        .map_err(|e| format!("installed database, but syncing its directory failed: {e}"))?;
    Ok(())
}

fn build(options: &Options) -> Result<()> {
    let mut codes = Codes::default();
    let v4 = read_table(&options.ipv4, &mut codes, parse_v4, |x| x.checked_add(1))?;
    let v6 = read_table(&options.ipv6, &mut codes, parse_v6, |x| x.checked_add(1))?;
    let bytes = encode(&options.label, &codes, &v4, &v6)?;
    install(&bytes, &options.out).map_err(|e| format!("{}: {e}", options.out.display()))?;
    println!(
        "IPv4: {} rows, {} merged ranges",
        v4.input_rows,
        v4.ranges.len()
    );
    println!(
        "IPv6: {} rows, {} merged ranges",
        v6.input_rows,
        v6.ranges.len()
    );
    println!(
        "Countries: {}\nOutput: {} bytes",
        codes.entries.len(),
        bytes.len()
    );
    Ok(())
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1).peekable();
    if args
        .peek()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!("{USAGE}");
        return Ok(());
    }
    if args.peek().is_some_and(|arg| arg == "check") {
        args.next();
        let path = args.next().ok_or(USAGE)?;
        if args.next().is_some() {
            return Err(USAGE.into());
        }
        let db = CountryDb::from_file(&path)
            .map_err(|e| format!("{}: {e}", Path::new(&path).display()))?;
        let info = db.info();
        println!(
            "Label: {}\nBuild time (Unix seconds): {}\nCountries: {}\nIPv4 ranges: {}\nIPv6 ranges: {}",
            info.dataset_label,
            info.build_time,
            info.country_count,
            info.ipv4_range_count,
            info.ipv6_range_count
        );
        return Ok(());
    }
    build(&Options::parse(args)?)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
