use std::{error, fmt, io};

/// A read or validation failure while loading a country table.
#[derive(Debug)]
#[non_exhaustive]
pub enum CountryDbError {
    /// The source file could not be read.
    Io(io::Error),
    /// The file does not begin with the country table magic.
    InvalidMagic,
    /// The file uses an unsupported format version.
    UnsupportedVersion(u32),
    /// The file sets undefined header flags.
    UnsupportedFlags(u32),
    /// The file length disagrees with its header or a required field is missing.
    InvalidLength,
    /// The label exceeds 255 bytes.
    LabelTooLong(u32),
    /// The dataset label is not valid UTF-8.
    InvalidLabel,
    /// The calculated CRC-32 differs from the trailing checksum.
    CrcMismatch,
    /// A country code contains a byte outside A–Z.
    InvalidCountryCode { index: usize },
    /// A range's end precedes its start.
    ReversedRange { family: &'static str, index: usize },
    /// Range starts are out of order.
    UnsortedRanges { family: &'static str, index: usize },
    /// This range and the preceding range intersect.
    OverlappingRanges { family: &'static str, index: usize },
    /// A range refers to a missing country code.
    InvalidCountryIndex { family: &'static str, index: usize },
    /// An IPv4 prefix offset is incorrect.
    InvalidPrefix { prefix: usize },
}

impl fmt::Display for CountryDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "could not read country table: {err}"),
            Self::InvalidMagic => f.write_str("invalid country table magic"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported country table version {v}"),
            Self::UnsupportedFlags(flags) => {
                write!(f, "unsupported country table flags {flags:#x}")
            }
            Self::InvalidLength => f.write_str("invalid country table length"),
            Self::LabelTooLong(n) => write!(f, "dataset label length {n} exceeds 255 bytes"),
            Self::InvalidLabel => f.write_str("dataset label is not UTF-8"),
            Self::CrcMismatch => f.write_str("country table CRC-32 mismatch"),
            Self::InvalidCountryCode { index } => {
                write!(f, "invalid country code at index {index}")
            }
            Self::ReversedRange { family, index } => {
                write!(f, "{family} range {index} ends before its start")
            }
            Self::UnsortedRanges { family, index } => {
                write!(f, "{family} range {index} is out of order")
            }
            Self::OverlappingRanges { family, index } => {
                write!(f, "{family} range {index} overlaps the preceding range")
            }
            Self::InvalidCountryIndex { family, index } => {
                write!(f, "{family} range {index} references a missing country")
            }
            Self::InvalidPrefix { prefix } => {
                write!(f, "invalid IPv4 prefix index at /16 {prefix}")
            }
        }
    }
}

impl error::Error for CountryDbError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for CountryDbError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
