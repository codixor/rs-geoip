use super::{CountryCode, CountryDb, CountryDbError as Error, Ranges};

pub const MAGIC: &[u8; 8] = b"RSGEOCT1";
pub const VERSION: u32 = 1;
pub const IPV4_PREFIX_FLAG: u32 = 1;
pub const PREFIX_COUNT: usize = 1 << 16;

const CRC_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut i = 0_u8;
    loop {
        let mut value = i as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[i as usize] = value;
        if i == u8::MAX {
            break;
        }
        i += 1;
    }
    table
};

/// Computes CRC-32/ISO-HDLC, the format's trailing checksum.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        let index = crc.to_le_bytes()[0] ^ byte;
        crc = (crc >> 8) ^ CRC_TABLE[usize::from(index)];
    }
    !crc
}

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let (bytes, rest) = self.0.split_at_checked(n).ok_or(Error::InvalidLength)?;
        self.0 = rest;
        Ok(bytes)
    }

    fn word<const W: usize>(&mut self) -> Result<[u8; W], Error> {
        let (word, rest) = self.0.split_first_chunk().ok_or(Error::InvalidLength)?;
        self.0 = rest;
        Ok(*word)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        self.word().map(u32::from_le_bytes)
    }

    fn array<const W: usize, T>(
        &mut self,
        count: usize,
        decode: impl Fn([u8; W]) -> T,
    ) -> Result<Vec<T>, Error> {
        let size = count.checked_mul(W).ok_or(Error::InvalidLength)?;
        Ok(self
            .take(size)?
            .as_chunks::<W>()
            .0
            .iter()
            .copied()
            .map(decode)
            .collect())
    }

    fn ranges<const W: usize, T: Ord>(
        &mut self,
        count: usize,
        code_count: usize,
        family: &'static str,
        decode: fn([u8; W]) -> T,
    ) -> Result<Ranges<T>, Error> {
        let starts = self.array(count, decode)?;
        let ends = self.array(count, decode)?;
        let countries = self.array(count, u16::from_le_bytes)?;
        for i in 0..count {
            if starts[i] > ends[i] {
                return Err(Error::ReversedRange { family, index: i });
            }
            if i > 0 {
                if starts[i] < starts[i - 1] {
                    return Err(Error::UnsortedRanges { family, index: i });
                }
                if starts[i] <= ends[i - 1] {
                    return Err(Error::OverlappingRanges { family, index: i });
                }
            }
            if usize::from(countries[i]) >= code_count {
                return Err(Error::InvalidCountryIndex { family, index: i });
            }
        }
        Ok(Ranges {
            starts,
            ends,
            countries,
        })
    }
}

fn validate_prefix(prefix: &[u32], ends: &[u32]) -> Result<(), Error> {
    let mut first = 0;
    for (p, &offset) in (0..=u16::MAX).zip(prefix) {
        let network = u32::from(p) << 16;
        while first < ends.len() && ends[first] < network {
            first += 1;
        }
        // Exact equality also guarantees monotonicity and bounds.
        if offset as usize != first {
            return Err(Error::InvalidPrefix {
                prefix: usize::from(p),
            });
        }
    }
    Ok(())
}

pub(super) fn decode(bytes: &[u8]) -> Result<CountryDb, Error> {
    let (body, stored_crc) = bytes.split_last_chunk::<4>().ok_or(Error::InvalidLength)?;
    let mut input = Input(body);
    if input.word::<8>()? != *MAGIC {
        return Err(Error::InvalidMagic);
    }
    let version = input.u32()?;
    if version != VERSION {
        return Err(Error::UnsupportedVersion(version));
    }
    let flags = input.u32()?;
    if flags & !IPV4_PREFIX_FLAG != 0 {
        return Err(Error::UnsupportedFlags(flags));
    }
    let has_prefix = flags & IPV4_PREFIX_FLAG != 0;
    let build_time = u64::from_le_bytes(input.word()?);
    let code_count = input.u32()?;
    let v4_count = input.u32()?;
    let v6_count = input.u32()?;
    let label_len = input.u32()?;
    if label_len > 255 {
        return Err(Error::LabelTooLong(label_len));
    }
    // All counts are u32 and the sum fits u64, including on 32-bit hosts.
    // No count-derived allocation or narrowing occurs before this exact check.
    let padded_label_len = label_len.next_multiple_of(8);
    let prefix_len = if has_prefix {
        4 * PREFIX_COUNT as u64
    } else {
        0
    };
    let length = 40
        + u64::from(padded_label_len)
        + 2 * u64::from(code_count)
        + 10 * u64::from(v4_count)
        + prefix_len
        + 34 * u64::from(v6_count);
    if body.len() as u64 != length {
        return Err(Error::InvalidLength);
    }
    if crc32(body) != u32::from_le_bytes(*stored_crc) {
        return Err(Error::CrcMismatch);
    }

    let label = std::str::from_utf8(input.take(label_len as usize)?)
        .map_err(|_| Error::InvalidLabel)?
        .to_owned();
    input.take((padded_label_len - label_len) as usize)?;
    let code_bytes = input.take(code_count as usize * 2)?;
    let codes = code_bytes
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
        .map(|(index, code)| {
            if code.iter().all(u8::is_ascii_uppercase) {
                Ok(CountryCode(*code))
            } else {
                Err(Error::InvalidCountryCode { index })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let v4 = input.ranges(v4_count as usize, codes.len(), "IPv4", u32::from_le_bytes)?;
    let prefix = if has_prefix {
        let table = input.array(PREFIX_COUNT, u32::from_le_bytes)?;
        validate_prefix(&table, &v4.ends)?;
        Some(table)
    } else {
        None
    };
    let v6 = input.ranges(v6_count as usize, codes.len(), "IPv6", u128::from_le_bytes)?;
    Ok(CountryDb {
        build_time,
        label,
        codes,
        v4,
        prefix,
        v6,
    })
}
