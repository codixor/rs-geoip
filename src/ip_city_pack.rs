#![allow(clippy::missing_errors_doc)]

use lru::LruCache;
use lz4_flex::{
    block::compress_prepend_size as compress_block_prepend_size, compress_prepend_size,
    decompress_size_prepended,
};
use parking_lot::Mutex;
use rkyv::rancor::Error as RkyvError;
use rkyv::util::AlignedVec;
use rkyv::{
    Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize, from_bytes, to_bytes,
};
#[cfg(not(target_arch = "wasm32"))]
use rkyv::{access, access_unchecked};
#[cfg(not(target_arch = "wasm32"))]
use rs_mmap::ReadOnlyMmap;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
use std::net::Ipv4Addr;
use std::num::NonZeroUsize;
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::{Arc, OnceLock};
use thiserror::Error;

pub const PACKED_IP_CITY_DB_VERSION: u32 = 1;
pub const PAGED_PACKED_IP_CITY_DB_VERSION: u32 = 1;
const PAGED_PACKED_FILE_MAGIC: [u8; 8] = *b"SGEOPG01";
const PAGED_PACKED_FILE_HEADER_VERSION_V1: u32 = 1;
const PAGED_PACKED_FILE_HEADER_VERSION_V2: u32 = 2;

#[repr(C)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    zerocopy::KnownLayout,
    zerocopy::Immutable,
    zerocopy::FromBytes,
    bytemuck::Pod,
    bytemuck::Zeroable,
)]
struct PagedPackedFileHeader {
    magic: [u8; 8],
    payload_len: u32,
    version: u32,
}

#[repr(C)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    zerocopy::KnownLayout,
    zerocopy::Immutable,
    zerocopy::FromBytes,
    bytemuck::Pod,
    bytemuck::Zeroable,
)]
struct PagedPackedFileHeaderV2 {
    prefix: PagedPackedFileHeader,
    block_count: u32,
    block_index_len: u32,
}

#[repr(C)]
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    zerocopy::KnownLayout,
    zerocopy::Immutable,
    zerocopy::FromBytes,
    bytemuck::Pod,
    bytemuck::Zeroable,
)]
struct PagedCityBlockMetaPod {
    city_count: u32,
    compressed_offset: u32,
    compressed_len: u32,
}

#[derive(Debug, Clone, Archive, RkyvSerialize, RkyvDeserialize)]
pub struct PackedIpCityDb {
    pub version: u32,
    pub city_count: u32,
    pub range_count: u32,
    pub cities: Vec<PackedCityRecord>,
    pub ranges: Vec<PackedRangeRecord>,
}

#[derive(Debug, Clone, Archive, RkyvSerialize, RkyvDeserialize, PartialEq)]
pub struct PackedCityRecord {
    pub city_name: String,
    pub country_code: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Copy, Archive, RkyvSerialize, RkyvDeserialize, PartialEq, Eq)]
pub struct PackedRangeRecord {
    pub ip_range_start: u32,
    pub ip_range_end: u32,
    pub city_id: u32,
}

#[derive(Debug, Clone, Archive, RkyvSerialize, RkyvDeserialize)]
pub struct PagedPackedIpCityDb {
    pub version: u32,
    pub block_size: u32,
    pub city_count: u32,
    pub range_count: u32,
    pub block_count: u32,
    pub ranges: Vec<PackedRangeRecord>,
    pub city_blocks: Vec<PagedCityBlockMeta>,
    pub city_block_payloads: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Archive, RkyvSerialize, RkyvDeserialize, PartialEq, Eq)]
pub struct PagedCityBlockMeta {
    pub city_count: u32,
    pub compressed_offset: u32,
    pub compressed_len: u32,
}

#[derive(Debug)]
pub struct PackedIpCityLookup {
    db: PackedIpCityDb,
}

#[derive(Debug)]
pub struct PagedPackedIpCityLookup {
    db: PagedPackedIpCityDb,
    block_cache: PagedBlockCache,
}

#[derive(Debug)]
#[cfg(not(target_arch = "wasm32"))]
pub struct MmapPagedPackedIpCityLookup {
    mmap: ReadOnlyMmap,
    payload_offset: usize,
    pod_block_index: Option<Arc<[PagedCityBlockMeta]>>,
    block_cache: PagedBlockCache,
}

#[derive(Debug)]
enum PagedBlockCache {
    None,
    Full(Vec<OnceLock<Arc<[PackedCityRecord]>>>),
    Lru(Mutex<LruCache<u32, Arc<[PackedCityRecord]>>>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IpCityMatch<'a> {
    pub city_id: u32,
    pub range_start: Ipv4Addr,
    pub range_end: Ipv4Addr,
    pub city: &'a PackedCityRecord,
}

#[derive(Debug, Clone)]
pub struct PagedIpCityMatch {
    pub city_id: u32,
    pub range_start: Ipv4Addr,
    pub range_end: Ipv4Addr,
    block: Arc<[PackedCityRecord]>,
    slot_in_block: usize,
}

impl PagedIpCityMatch {
    #[must_use]
    pub fn city(&self) -> &PackedCityRecord {
        &self.block[self.slot_in_block]
    }
}

#[derive(Debug, Error)]
pub enum PackedIpCityError {
    #[error("failed to read packed IP city file")]
    Io(#[from] std::io::Error),
    #[cfg(not(target_arch = "wasm32"))]
    #[error("failed to memory-map packed IP city file")]
    Mmap(#[from] rs_mmap::MmapError),
    #[error("failed to decompress packed IP city payload")]
    Decompress(#[from] lz4_flex::block::DecompressError),
    #[error("failed to decode packed IP city payload")]
    Decode(RkyvError),
    #[error("failed to encode packed IP city payload")]
    Encode(RkyvError),
    #[error("failed to decode paged city block payload")]
    DecodeBlock(RkyvError),
    #[error("unsupported packed IP city version: expected {expected}, got {actual}")]
    UnsupportedVersion { expected: u32, actual: u32 },
    #[error("unsupported paged packed IP city version: expected {expected}, got {actual}")]
    UnsupportedPagedVersion { expected: u32, actual: u32 },
    #[error("city count header mismatch: expected {expected}, got {actual}")]
    CityCountMismatch { expected: usize, actual: usize },
    #[error("range count header mismatch: expected {expected}, got {actual}")]
    RangeCountMismatch { expected: usize, actual: usize },
    #[error("block count header mismatch: expected {expected}, got {actual}")]
    BlockCountMismatch { expected: usize, actual: usize },
    #[error("range {range_index} references missing city_id {city_id}")]
    InvalidCityId { range_index: usize, city_id: u32 },
    #[error("range {range_index} has inverted bounds")]
    InvertedRange { range_index: usize },
    #[error("range {range_index} is not sorted after previous range")]
    UnsortedRange { range_index: usize },
    #[error("paged packed block_size must be non-zero")]
    InvalidBlockSize,
    #[error("paged packed block {block_index} exceeds payload bounds")]
    InvalidBlockOffset { block_index: usize },
    #[error(
        "paged packed block {block_index} decoded city count mismatch: expected {expected}, got {actual}"
    )]
    BlockCityCountMismatch {
        block_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error("paged packed city blocks cover {actual} cities, expected {expected}")]
    CityCoverageMismatch { expected: usize, actual: usize },
    #[error("paged packed file header payload length mismatch: expected {expected}, got {actual}")]
    PagedHeaderLengthMismatch { expected: usize, actual: usize },
    #[error("paged packed file block index length mismatch: expected {expected}, got {actual}")]
    PagedHeaderBlockIndexLengthMismatch { expected: usize, actual: usize },
    #[error("paged packed file block count mismatch: expected {expected}, got {actual}")]
    PagedHeaderBlockCountMismatch { expected: usize, actual: usize },
}

impl PagedPackedFileHeader {
    const LEN: usize = std::mem::size_of::<Self>();

    #[cfg(not(target_arch = "wasm32"))]
    fn has_magic(&self) -> bool {
        self.magic == PAGED_PACKED_FILE_MAGIC
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn payload_len(&self) -> usize {
        self.payload_len as usize
    }
}

impl PagedPackedFileHeaderV2 {
    const LEN: usize = std::mem::size_of::<Self>();

    fn new(
        payload_len: usize,
        block_count: usize,
        block_index_len: usize,
    ) -> Result<Self, PackedIpCityError> {
        Ok(Self {
            prefix: PagedPackedFileHeader {
                magic: PAGED_PACKED_FILE_MAGIC,
                payload_len: u32::try_from(payload_len)
                    .map_err(|_| PackedIpCityError::InvalidBlockSize)?,
                version: PAGED_PACKED_FILE_HEADER_VERSION_V2,
            },
            block_count: u32::try_from(block_count)
                .map_err(|_| PackedIpCityError::InvalidBlockSize)?,
            block_index_len: u32::try_from(block_index_len)
                .map_err(|_| PackedIpCityError::InvalidBlockSize)?,
        })
    }
}

impl From<PagedCityBlockMeta> for PagedCityBlockMetaPod {
    fn from(value: PagedCityBlockMeta) -> Self {
        Self {
            city_count: value.city_count,
            compressed_offset: value.compressed_offset,
            compressed_len: value.compressed_len,
        }
    }
}

impl PagedCityBlockMetaPod {
    #[cfg(not(target_arch = "wasm32"))]
    fn to_owned(self) -> PagedCityBlockMeta {
        PagedCityBlockMeta {
            city_count: self.city_count,
            compressed_offset: self.compressed_offset,
            compressed_len: self.compressed_len,
        }
    }
}

impl PackedIpCityDb {
    pub fn validate(&self) -> Result<(), PackedIpCityError> {
        if self.version != PACKED_IP_CITY_DB_VERSION {
            return Err(PackedIpCityError::UnsupportedVersion {
                expected: PACKED_IP_CITY_DB_VERSION,
                actual: self.version,
            });
        }

        if self.city_count as usize != self.cities.len() {
            return Err(PackedIpCityError::CityCountMismatch {
                expected: self.city_count as usize,
                actual: self.cities.len(),
            });
        }

        if self.range_count as usize != self.ranges.len() {
            return Err(PackedIpCityError::RangeCountMismatch {
                expected: self.range_count as usize,
                actual: self.ranges.len(),
            });
        }

        let mut previous_end = None;
        for (range_index, range) in self.ranges.iter().enumerate() {
            if range.city_id as usize >= self.cities.len() {
                return Err(PackedIpCityError::InvalidCityId {
                    range_index,
                    city_id: range.city_id,
                });
            }

            if range.ip_range_start > range.ip_range_end {
                return Err(PackedIpCityError::InvertedRange { range_index });
            }

            if let Some(end) = previous_end
                && range.ip_range_start <= end
            {
                return Err(PackedIpCityError::UnsortedRange { range_index });
            }

            previous_end = Some(range.ip_range_end);
        }

        Ok(())
    }

    pub fn encode_lz4(&self) -> Result<Vec<u8>, PackedIpCityError> {
        let encoded = to_bytes::<RkyvError>(self).map_err(PackedIpCityError::Encode)?;
        Ok(compress_prepend_size(&encoded))
    }

    pub fn decode_lz4(bytes: &[u8]) -> Result<Self, PackedIpCityError> {
        let decompressed = decompress_size_prepended(bytes)?;
        let aligned = aligned_bytes(&decompressed);
        let db = from_bytes::<Self, RkyvError>(&aligned).map_err(PackedIpCityError::Decode)?;
        db.validate()?;
        Ok(db)
    }
}

impl PagedPackedIpCityDb {
    pub fn from_flat(flat: &PackedIpCityDb, block_size: usize) -> Result<Self, PackedIpCityError> {
        if block_size == 0 {
            return Err(PackedIpCityError::InvalidBlockSize);
        }

        flat.validate()?;

        let mut city_blocks = Vec::new();
        let mut city_block_payloads = Vec::new();

        for chunk in flat.cities.chunks(block_size) {
            let chunk = chunk.to_vec();
            let encoded = to_bytes::<RkyvError>(&chunk).map_err(PackedIpCityError::Encode)?;
            let compressed = compress_block_prepend_size(&encoded);
            let compressed_offset = u32::try_from(city_block_payloads.len())
                .map_err(|_| PackedIpCityError::InvalidBlockSize)?;
            let compressed_len =
                u32::try_from(compressed.len()).map_err(|_| PackedIpCityError::InvalidBlockSize)?;
            let city_count =
                u32::try_from(chunk.len()).map_err(|_| PackedIpCityError::InvalidBlockSize)?;

            city_blocks.push(PagedCityBlockMeta {
                city_count,
                compressed_offset,
                compressed_len,
            });
            city_block_payloads.extend_from_slice(&compressed);
        }

        let block_size =
            u32::try_from(block_size).map_err(|_| PackedIpCityError::InvalidBlockSize)?;

        let db = Self {
            version: PAGED_PACKED_IP_CITY_DB_VERSION,
            block_size,
            city_count: flat.city_count,
            range_count: flat.range_count,
            block_count: u32::try_from(city_blocks.len())
                .map_err(|_| PackedIpCityError::InvalidBlockSize)?,
            ranges: flat.ranges.clone(),
            city_blocks,
            city_block_payloads,
        };
        db.validate()?;
        Ok(db)
    }

    pub fn validate(&self) -> Result<(), PackedIpCityError> {
        if self.version != PAGED_PACKED_IP_CITY_DB_VERSION {
            return Err(PackedIpCityError::UnsupportedPagedVersion {
                expected: PAGED_PACKED_IP_CITY_DB_VERSION,
                actual: self.version,
            });
        }

        if self.block_size == 0 {
            return Err(PackedIpCityError::InvalidBlockSize);
        }

        if self.city_count as usize == 0 && !self.city_blocks.is_empty() {
            return Err(PackedIpCityError::CityCoverageMismatch {
                expected: 0,
                actual: self.city_blocks.iter().map(|b| b.city_count as usize).sum(),
            });
        }

        if self.range_count as usize != self.ranges.len() {
            return Err(PackedIpCityError::RangeCountMismatch {
                expected: self.range_count as usize,
                actual: self.ranges.len(),
            });
        }

        if self.block_count as usize != self.city_blocks.len() {
            return Err(PackedIpCityError::BlockCountMismatch {
                expected: self.block_count as usize,
                actual: self.city_blocks.len(),
            });
        }

        let mut previous_end = None;
        for (range_index, range) in self.ranges.iter().enumerate() {
            if range.city_id as usize >= self.city_count as usize {
                return Err(PackedIpCityError::InvalidCityId {
                    range_index,
                    city_id: range.city_id,
                });
            }
            if range.ip_range_start > range.ip_range_end {
                return Err(PackedIpCityError::InvertedRange { range_index });
            }
            if let Some(end) = previous_end
                && range.ip_range_start <= end
            {
                return Err(PackedIpCityError::UnsortedRange { range_index });
            }
            previous_end = Some(range.ip_range_end);
        }

        let mut covered_cities = 0usize;
        for (block_index, block) in self.city_blocks.iter().enumerate() {
            let start = block.compressed_offset as usize;
            let end = start + block.compressed_len as usize;
            if self.city_block_payloads.get(start..end).is_none() {
                return Err(PackedIpCityError::InvalidBlockOffset { block_index });
            }
            covered_cities += block.city_count as usize;
        }

        if covered_cities != self.city_count as usize {
            return Err(PackedIpCityError::CityCoverageMismatch {
                expected: self.city_count as usize,
                actual: covered_cities,
            });
        }

        Ok(())
    }

    pub fn encode_bytes(&self) -> Result<Vec<u8>, PackedIpCityError> {
        self.validate()?;
        let payload = to_bytes::<RkyvError>(self)
            .map_err(PackedIpCityError::Encode)?
            .to_vec();
        let pod_block_index: Vec<PagedCityBlockMetaPod> = self
            .city_blocks
            .iter()
            .copied()
            .map(PagedCityBlockMetaPod::from)
            .collect();
        let block_index_len = std::mem::size_of_val(pod_block_index.as_slice());
        let header =
            PagedPackedFileHeaderV2::new(payload.len(), pod_block_index.len(), block_index_len)?;
        let mut bytes =
            Vec::with_capacity(PagedPackedFileHeaderV2::LEN + block_index_len + payload.len());
        bytes.extend_from_slice(bytemuck::bytes_of(&header));
        bytes.extend_from_slice(bytemuck::cast_slice(&pod_block_index));
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }

    pub fn decode_bytes(bytes: &[u8]) -> Result<Self, PackedIpCityError> {
        let aligned = aligned_bytes(paged_payload_bytes(bytes)?);
        let db = from_bytes::<Self, RkyvError>(&aligned).map_err(PackedIpCityError::Decode)?;
        db.validate()?;
        Ok(db)
    }
}

fn aligned_bytes(bytes: &[u8]) -> AlignedVec {
    let mut aligned = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

fn parse_paged_header_prefix(bytes: &[u8]) -> Option<(usize, u32)> {
    if bytes.len() < PagedPackedFileHeader::LEN {
        return None;
    }

    let magic = <[u8; 8]>::try_from(&bytes[..8]).ok()?;
    if magic != PAGED_PACKED_FILE_MAGIC {
        return None;
    }

    let payload_len = u32::from_ne_bytes(bytes[8..12].try_into().ok()?);
    let version = u32::from_ne_bytes(bytes[12..16].try_into().ok()?);
    Some((payload_len as usize, version))
}

#[derive(Debug, Clone, Copy)]
struct PagedPackedFileLayout {
    payload_offset: usize,
    #[cfg(not(target_arch = "wasm32"))]
    pod_block_index_offset: Option<usize>,
    #[cfg(not(target_arch = "wasm32"))]
    pod_block_index_len: usize,
    #[cfg(not(target_arch = "wasm32"))]
    pod_block_count: Option<usize>,
}

fn paged_layout_from_bytes(bytes: &[u8]) -> Result<PagedPackedFileLayout, PackedIpCityError> {
    let Some((expected, version)) = parse_paged_header_prefix(bytes) else {
        return Ok(PagedPackedFileLayout {
            payload_offset: 0,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_index_offset: None,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_index_len: 0,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_count: None,
        });
    };

    if version == PAGED_PACKED_FILE_HEADER_VERSION_V1 {
        let payload_offset = PagedPackedFileHeader::LEN;
        let actual = bytes
            .len()
            .checked_sub(payload_offset)
            .ok_or(PackedIpCityError::InvalidBlockSize)?;
        if actual != expected {
            return Err(PackedIpCityError::PagedHeaderLengthMismatch { expected, actual });
        }
        return Ok(PagedPackedFileLayout {
            payload_offset,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_index_offset: None,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_index_len: 0,
            #[cfg(not(target_arch = "wasm32"))]
            pod_block_count: None,
        });
    }

    if version != PAGED_PACKED_FILE_HEADER_VERSION_V2 {
        return Err(PackedIpCityError::UnsupportedPagedVersion {
            expected: PAGED_PACKED_FILE_HEADER_VERSION_V2,
            actual: version,
        });
    }

    if bytes.len() < PagedPackedFileHeaderV2::LEN {
        return Err(PackedIpCityError::InvalidBlockSize);
    }

    let block_count =
        u32::from_ne_bytes(bytes[16..20].try_into().expect("slice length checked")) as usize;
    let block_index_len =
        u32::from_ne_bytes(bytes[20..24].try_into().expect("slice length checked")) as usize;
    let expected_block_index_len = block_count * std::mem::size_of::<PagedCityBlockMetaPod>();
    if block_index_len != expected_block_index_len {
        return Err(PackedIpCityError::PagedHeaderBlockIndexLengthMismatch {
            expected: expected_block_index_len,
            actual: block_index_len,
        });
    }

    let payload_offset = PagedPackedFileHeaderV2::LEN + block_index_len;
    let actual = bytes
        .len()
        .checked_sub(payload_offset)
        .ok_or(PackedIpCityError::InvalidBlockSize)?;
    if actual != expected {
        return Err(PackedIpCityError::PagedHeaderLengthMismatch { expected, actual });
    }

    Ok(PagedPackedFileLayout {
        payload_offset,
        #[cfg(not(target_arch = "wasm32"))]
        pod_block_index_offset: Some(PagedPackedFileHeaderV2::LEN),
        #[cfg(not(target_arch = "wasm32"))]
        pod_block_index_len: block_index_len,
        #[cfg(not(target_arch = "wasm32"))]
        pod_block_count: Some(block_count),
    })
}

fn paged_payload_bytes(bytes: &[u8]) -> Result<&[u8], PackedIpCityError> {
    let layout = paged_layout_from_bytes(bytes)?;
    Ok(&bytes[layout.payload_offset..])
}

#[cfg(not(target_arch = "wasm32"))]
fn paged_layout_from_mmap(mmap: &ReadOnlyMmap) -> Result<PagedPackedFileLayout, PackedIpCityError> {
    if mmap.len() < PagedPackedFileHeader::LEN {
        return Ok(PagedPackedFileLayout {
            payload_offset: 0,
            pod_block_index_offset: None,
            pod_block_index_len: 0,
            pod_block_count: None,
        });
    }

    let header_slice = mmap.slice(0..PagedPackedFileHeader::LEN)?;
    let header = header_slice.view::<PagedPackedFileHeader>()?;
    if !header.has_magic() {
        return Ok(PagedPackedFileLayout {
            payload_offset: 0,
            pod_block_index_offset: None,
            pod_block_index_len: 0,
            pod_block_count: None,
        });
    }

    if header.version == PAGED_PACKED_FILE_HEADER_VERSION_V1 {
        let expected = header.payload_len();
        let actual = mmap
            .len()
            .checked_sub(PagedPackedFileHeader::LEN)
            .ok_or(PackedIpCityError::InvalidBlockSize)?;
        if actual != expected {
            return Err(PackedIpCityError::PagedHeaderLengthMismatch { expected, actual });
        }
        return Ok(PagedPackedFileLayout {
            payload_offset: PagedPackedFileHeader::LEN,
            pod_block_index_offset: None,
            pod_block_index_len: 0,
            pod_block_count: None,
        });
    }

    if header.version != PAGED_PACKED_FILE_HEADER_VERSION_V2 {
        return Err(PackedIpCityError::UnsupportedPagedVersion {
            expected: PAGED_PACKED_FILE_HEADER_VERSION_V2,
            actual: header.version,
        });
    }

    let header_v2_slice = mmap.slice(0..PagedPackedFileHeaderV2::LEN)?;
    let header_v2 = header_v2_slice.view::<PagedPackedFileHeaderV2>()?;
    let block_count = header_v2.block_count as usize;
    let block_index_len = header_v2.block_index_len as usize;
    let expected_block_index_len = block_count * std::mem::size_of::<PagedCityBlockMetaPod>();
    if block_index_len != expected_block_index_len {
        return Err(PackedIpCityError::PagedHeaderBlockIndexLengthMismatch {
            expected: expected_block_index_len,
            actual: block_index_len,
        });
    }

    let expected = header.payload_len();
    let payload_offset = PagedPackedFileHeaderV2::LEN + block_index_len;
    let actual = mmap
        .len()
        .checked_sub(payload_offset)
        .ok_or(PackedIpCityError::InvalidBlockSize)?;
    if actual != expected {
        return Err(PackedIpCityError::PagedHeaderLengthMismatch { expected, actual });
    }

    Ok(PagedPackedFileLayout {
        payload_offset,
        pod_block_index_offset: Some(PagedPackedFileHeaderV2::LEN),
        pod_block_index_len: block_index_len,
        pod_block_count: Some(block_count),
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn load_pod_block_index_from_mmap(
    mmap: &ReadOnlyMmap,
    layout: PagedPackedFileLayout,
) -> Result<Option<Arc<[PagedCityBlockMeta]>>, PackedIpCityError> {
    let Some(offset) = layout.pod_block_index_offset else {
        return Ok(None);
    };
    let end = offset + layout.pod_block_index_len;
    let block_index_slice = mmap.slice(offset..end)?;
    let metas = block_index_slice.view_slice::<PagedCityBlockMetaPod>()?;
    let owned: Vec<PagedCityBlockMeta> = metas
        .iter()
        .copied()
        .map(PagedCityBlockMetaPod::to_owned)
        .collect();
    Ok(Some(Arc::<[PagedCityBlockMeta]>::from(owned)))
}

#[cfg(not(target_arch = "wasm32"))]
fn archived_u32<T>(value: T) -> u32
where
    T: Into<u32>,
{
    value.into()
}

#[cfg(not(target_arch = "wasm32"))]
fn archived_usize<T>(value: T) -> Result<usize, PackedIpCityError>
where
    T: Into<u32>,
{
    usize::try_from(value.into()).map_err(|_| PackedIpCityError::InvalidBlockSize)
}

fn default_paged_block_cache_capacity(db: &PagedPackedIpCityDb) -> usize {
    // The paged reader lazily fills `OnceLock` slots rather than preloading
    // blocks, so using a direct slot per block avoids LRU lock churn without a
    // meaningful cold-open penalty on the current dataset.
    db.block_count as usize
}

#[cfg(not(target_arch = "wasm32"))]
fn default_paged_block_cache_capacity_from_count(block_count: usize) -> usize {
    block_count
}

fn build_paged_block_cache(block_count: usize, block_cache_capacity: usize) -> PagedBlockCache {
    if block_cache_capacity == 0 {
        PagedBlockCache::None
    } else if block_cache_capacity >= block_count {
        PagedBlockCache::Full(
            std::iter::repeat_with(OnceLock::new)
                .take(block_count)
                .collect(),
        )
    } else {
        let cache = NonZeroUsize::new(block_cache_capacity)
            .map(LruCache::new)
            .map(Mutex::new)
            .expect("non-zero block cache capacity");
        PagedBlockCache::Lru(cache)
    }
}

impl PackedIpCityLookup {
    pub fn from_db(db: PackedIpCityDb) -> Result<Self, PackedIpCityError> {
        db.validate()?;
        Ok(Self { db })
    }

    pub fn from_lz4_bytes(bytes: &[u8]) -> Result<Self, PackedIpCityError> {
        Self::from_db(PackedIpCityDb::decode_lz4(bytes)?)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_lz4_file(path: impl AsRef<Path>) -> Result<Self, PackedIpCityError> {
        Self::from_lz4_bytes(&fs::read(path)?)
    }

    #[must_use]
    pub fn db(&self) -> &PackedIpCityDb {
        &self.db
    }

    #[must_use]
    pub fn find_ipv4(&self, ip: Ipv4Addr) -> Option<IpCityMatch<'_>> {
        self.find_ipv4_u32(u32::from(ip))
    }

    #[must_use]
    pub fn find_ipv4_u32(&self, ip: u32) -> Option<IpCityMatch<'_>> {
        let index = self
            .db
            .ranges
            .partition_point(|range| range.ip_range_end < ip);
        let range = self.db.ranges.get(index)?;
        if ip < range.ip_range_start || ip > range.ip_range_end {
            return None;
        }

        let city = self.db.cities.get(range.city_id as usize)?;
        Some(IpCityMatch {
            city_id: range.city_id,
            range_start: Ipv4Addr::from(range.ip_range_start),
            range_end: Ipv4Addr::from(range.ip_range_end),
            city,
        })
    }
}

impl PagedPackedIpCityLookup {
    pub fn from_db(db: PagedPackedIpCityDb) -> Result<Self, PackedIpCityError> {
        let block_cache_capacity = default_paged_block_cache_capacity(&db);
        Self::from_db_with_cache_capacity(db, block_cache_capacity)
    }

    pub fn from_db_with_cache_capacity(
        db: PagedPackedIpCityDb,
        block_cache_capacity: usize,
    ) -> Result<Self, PackedIpCityError> {
        db.validate()?;
        let block_cache = build_paged_block_cache(db.block_count as usize, block_cache_capacity);
        Ok(Self { db, block_cache })
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackedIpCityError> {
        Self::from_db(PagedPackedIpCityDb::decode_bytes(bytes)?)
    }

    pub fn from_bytes_with_cache_capacity(
        bytes: &[u8],
        block_cache_capacity: usize,
    ) -> Result<Self, PackedIpCityError> {
        Self::from_db_with_cache_capacity(
            PagedPackedIpCityDb::decode_bytes(bytes)?,
            block_cache_capacity,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, PackedIpCityError> {
        Self::from_bytes(&fs::read(path)?)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file_with_cache_capacity(
        path: impl AsRef<Path>,
        block_cache_capacity: usize,
    ) -> Result<Self, PackedIpCityError> {
        Self::from_bytes_with_cache_capacity(&fs::read(path)?, block_cache_capacity)
    }

    pub fn db(&self) -> &PagedPackedIpCityDb {
        &self.db
    }

    pub fn find_ipv4(&self, ip: Ipv4Addr) -> Option<PagedIpCityMatch> {
        self.find_ipv4_u32(u32::from(ip))
    }

    pub fn find_ipv4_u32(&self, ip: u32) -> Option<PagedIpCityMatch> {
        let index = self
            .db
            .ranges
            .partition_point(|range| range.ip_range_end < ip);
        let range = self.db.ranges.get(index)?;
        if ip < range.ip_range_start || ip > range.ip_range_end {
            return None;
        }

        let city_id = range.city_id as usize;
        let block_size = self.db.block_size as usize;
        let block_id = city_id / block_size;
        let slot_in_block = city_id % block_size;
        let block = self.decode_city_block(u32::try_from(block_id).ok()?).ok()?;
        if slot_in_block >= block.len() {
            return None;
        }

        Some(PagedIpCityMatch {
            city_id: range.city_id,
            range_start: Ipv4Addr::from(range.ip_range_start),
            range_end: Ipv4Addr::from(range.ip_range_end),
            block,
            slot_in_block,
        })
    }

    fn decode_city_block_uncached(
        &self,
        block_id: u32,
    ) -> Result<Arc<[PackedCityRecord]>, PackedIpCityError> {
        let block_index = block_id as usize;
        let block = self
            .db
            .city_blocks
            .get(block_index)
            .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
        let start = block.compressed_offset as usize;
        let end = start + block.compressed_len as usize;
        let compressed = self
            .db
            .city_block_payloads
            .get(start..end)
            .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
        let decoded = decompress_size_prepended(compressed)?;
        let aligned = aligned_bytes(&decoded);
        let cities = from_bytes::<Vec<PackedCityRecord>, RkyvError>(&aligned)
            .map_err(PackedIpCityError::DecodeBlock)?;
        if cities.len() != block.city_count as usize {
            return Err(PackedIpCityError::BlockCityCountMismatch {
                block_index,
                expected: block.city_count as usize,
                actual: cities.len(),
            });
        }

        Ok(Arc::<[PackedCityRecord]>::from(cities))
    }

    fn decode_city_block(
        &self,
        block_id: u32,
    ) -> Result<Arc<[PackedCityRecord]>, PackedIpCityError> {
        match &self.block_cache {
            PagedBlockCache::None => self.decode_city_block_uncached(block_id),
            PagedBlockCache::Full(cache) => {
                let block_index = block_id as usize;
                let slot = cache
                    .get(block_index)
                    .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
                if let Some(block) = slot.get() {
                    return Ok(block.clone());
                }

                let decoded = self.decode_city_block_uncached(block_id)?;
                let _ = slot.set(decoded.clone());
                Ok(slot.get().cloned().unwrap_or(decoded))
            }
            PagedBlockCache::Lru(cache) => {
                if let Some(block) = cache.lock().get(&block_id).cloned() {
                    return Ok(block);
                }

                let decoded = self.decode_city_block_uncached(block_id)?;
                cache.lock().put(block_id, decoded.clone());
                Ok(decoded)
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_archived_paged_db(db: &ArchivedPagedPackedIpCityDb) -> Result<(), PackedIpCityError> {
    let version = archived_u32(db.version);
    if version != PAGED_PACKED_IP_CITY_DB_VERSION {
        return Err(PackedIpCityError::UnsupportedPagedVersion {
            expected: PAGED_PACKED_IP_CITY_DB_VERSION,
            actual: version,
        });
    }

    if archived_u32(db.block_size) == 0 {
        return Err(PackedIpCityError::InvalidBlockSize);
    }

    let city_count = archived_usize(db.city_count)?;
    let range_count = archived_usize(db.range_count)?;
    let block_count = archived_usize(db.block_count)?;

    if city_count == 0 && !db.city_blocks.is_empty() {
        return Err(PackedIpCityError::CityCoverageMismatch {
            expected: 0,
            actual: db
                .city_blocks
                .iter()
                .map(|b| archived_u32(b.city_count) as usize)
                .sum(),
        });
    }

    if range_count != db.ranges.len() {
        return Err(PackedIpCityError::RangeCountMismatch {
            expected: range_count,
            actual: db.ranges.len(),
        });
    }

    if block_count != db.city_blocks.len() {
        return Err(PackedIpCityError::BlockCountMismatch {
            expected: block_count,
            actual: db.city_blocks.len(),
        });
    }

    let mut previous_end = None;
    for (range_index, range) in db.ranges.iter().enumerate() {
        let city_id = archived_u32(range.city_id);
        let range_start = archived_u32(range.ip_range_start);
        let range_end = archived_u32(range.ip_range_end);

        if city_id as usize >= city_count {
            return Err(PackedIpCityError::InvalidCityId {
                range_index,
                city_id,
            });
        }
        if range_start > range_end {
            return Err(PackedIpCityError::InvertedRange { range_index });
        }
        if let Some(end) = previous_end
            && range_start <= end
        {
            return Err(PackedIpCityError::UnsortedRange { range_index });
        }
        previous_end = Some(range_end);
    }

    let mut covered_cities = 0usize;
    for (block_index, block) in db.city_blocks.iter().enumerate() {
        let start = archived_usize(block.compressed_offset)?;
        let end = start + archived_usize(block.compressed_len)?;
        if db.city_block_payloads.get(start..end).is_none() {
            return Err(PackedIpCityError::InvalidBlockOffset { block_index });
        }
        covered_cities += archived_u32(block.city_count) as usize;
    }

    if covered_cities != city_count {
        return Err(PackedIpCityError::CityCoverageMismatch {
            expected: city_count,
            actual: covered_cities,
        });
    }

    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
impl MmapPagedPackedIpCityLookup {
    /// Opens a paged packed artifact using the `rs-mmap` backend.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the mapped file is not modified or truncated
    /// while the lookup exists.
    #[allow(unsafe_code)]
    pub unsafe fn from_mmap_file(path: impl AsRef<Path>) -> Result<Self, PackedIpCityError> {
        // SAFETY: The caller guarantees the file stays immutable until this lookup is dropped.
        unsafe { Self::open_mapped(path.as_ref(), None) }
    }

    /// Opens a paged packed artifact using the `rs-mmap` backend with
    /// an explicit block-cache capacity.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the mapped file is not modified or truncated
    /// while the lookup exists.
    #[allow(unsafe_code)]
    pub unsafe fn from_mmap_file_with_cache_capacity(
        path: impl AsRef<Path>,
        block_cache_capacity: usize,
    ) -> Result<Self, PackedIpCityError> {
        // SAFETY: The caller guarantees the file stays immutable until this lookup is dropped.
        unsafe { Self::open_mapped(path.as_ref(), Some(block_cache_capacity)) }
    }

    unsafe fn open_mapped(
        path: &Path,
        cache_capacity: Option<usize>,
    ) -> Result<Self, PackedIpCityError> {
        #[cfg(any(unix, windows))]
        // SAFETY: Both constructors require the backing file to remain immutable
        // through every handle for the lifetime of the returned lookup.
        let mmap = ReadOnlyMmap::from_mmap(unsafe {
            rs_mmap::MmapOptions::new().path(path).map_stable()?
        });
        #[cfg(not(any(unix, windows)))]
        let mmap = ReadOnlyMmap::open(path)?;
        let layout = paged_layout_from_mmap(&mmap)?;
        let db = access::<ArchivedPagedPackedIpCityDb, RkyvError>(
            &mmap.as_slice()[layout.payload_offset..],
        )
        .map_err(PackedIpCityError::Decode)?;
        validate_archived_paged_db(db)?;
        let block_count = archived_usize(db.block_count)?;
        if let Some(pod_block_count) = layout.pod_block_count
            && pod_block_count != block_count
        {
            return Err(PackedIpCityError::PagedHeaderBlockCountMismatch {
                expected: block_count,
                actual: pod_block_count,
            });
        }
        let pod_block_index = load_pod_block_index_from_mmap(&mmap, layout)?;
        let capacity = cache_capacity
            .unwrap_or_else(|| default_paged_block_cache_capacity_from_count(block_count));
        let block_cache = build_paged_block_cache(block_count, capacity);
        Ok(Self {
            mmap,
            payload_offset: layout.payload_offset,
            pod_block_index,
            block_cache,
        })
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        archived_u32(self.archived_db().block_count) as usize
    }

    #[must_use]
    pub fn find_ipv4(&self, ip: Ipv4Addr) -> Option<PagedIpCityMatch> {
        self.find_ipv4_u32(u32::from(ip))
    }

    #[must_use]
    pub fn find_ipv4_u32(&self, ip: u32) -> Option<PagedIpCityMatch> {
        let db = self.archived_db();
        let index = db.ranges.partition_point(|range| range.ip_range_end < ip);
        let range = db.ranges.get(index)?;
        let range_start = archived_u32(range.ip_range_start);
        let range_end = archived_u32(range.ip_range_end);
        if ip < range_start || ip > range_end {
            return None;
        }

        let city_id = archived_u32(range.city_id) as usize;
        let block_size = archived_u32(db.block_size) as usize;
        let block_id = city_id / block_size;
        let slot_in_block = city_id % block_size;
        let block = self.decode_city_block(u32::try_from(block_id).ok()?).ok()?;
        if slot_in_block >= block.len() {
            return None;
        }

        Some(PagedIpCityMatch {
            city_id: archived_u32(range.city_id),
            range_start: Ipv4Addr::from(range_start),
            range_end: Ipv4Addr::from(range_end),
            block,
            slot_in_block,
        })
    }

    fn archived_db(&self) -> &ArchivedPagedPackedIpCityDb {
        // SAFETY: The mmap bytes were validated with `rkyv::access` during
        // construction and the underlying file must remain immutable while this
        // lookup exists.
        unsafe {
            access_unchecked::<ArchivedPagedPackedIpCityDb>(
                &self.mmap.as_slice()[self.payload_offset..],
            )
        }
    }

    fn pod_block_meta(
        &self,
        block_index: usize,
    ) -> Result<Option<PagedCityBlockMeta>, PackedIpCityError> {
        let Some(index) = &self.pod_block_index else {
            return Ok(None);
        };
        let meta = index
            .get(block_index)
            .copied()
            .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
        Ok(Some(meta))
    }

    fn decode_city_block_uncached(
        &self,
        block_id: u32,
    ) -> Result<Arc<[PackedCityRecord]>, PackedIpCityError> {
        let db = self.archived_db();
        let block_index = block_id as usize;
        let pod_meta = self.pod_block_meta(block_index)?;
        let (start, end, city_count) = if let Some(block) = pod_meta {
            let start = block.compressed_offset as usize;
            let end = start + block.compressed_len as usize;
            (start, end, block.city_count as usize)
        } else {
            let block = db
                .city_blocks
                .get(block_index)
                .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
            let start = archived_usize(block.compressed_offset)?;
            let end = start + archived_usize(block.compressed_len)?;
            (start, end, archived_u32(block.city_count) as usize)
        };
        let compressed = db
            .city_block_payloads
            .get(start..end)
            .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
        let decoded = decompress_size_prepended(compressed)?;
        let aligned = aligned_bytes(&decoded);
        let cities = from_bytes::<Vec<PackedCityRecord>, RkyvError>(&aligned)
            .map_err(PackedIpCityError::DecodeBlock)?;
        if cities.len() != city_count {
            return Err(PackedIpCityError::BlockCityCountMismatch {
                block_index,
                expected: city_count,
                actual: cities.len(),
            });
        }

        Ok(Arc::<[PackedCityRecord]>::from(cities))
    }

    fn decode_city_block(
        &self,
        block_id: u32,
    ) -> Result<Arc<[PackedCityRecord]>, PackedIpCityError> {
        match &self.block_cache {
            PagedBlockCache::None => self.decode_city_block_uncached(block_id),
            PagedBlockCache::Full(cache) => {
                let block_index = block_id as usize;
                let slot = cache
                    .get(block_index)
                    .ok_or(PackedIpCityError::InvalidBlockOffset { block_index })?;
                if let Some(block) = slot.get() {
                    return Ok(block.clone());
                }

                let decoded = self.decode_city_block_uncached(block_id)?;
                let _ = slot.set(decoded.clone());
                Ok(slot.get().cloned().unwrap_or(decoded))
            }
            PagedBlockCache::Lru(cache) => {
                if let Some(block) = cache.lock().get(&block_id).cloned() {
                    return Ok(block);
                }

                let decoded = self.decode_city_block_uncached(block_id)?;
                cache.lock().put(block_id, decoded.clone());
                Ok(decoded)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_db() -> PackedIpCityDb {
        PackedIpCityDb {
            version: PACKED_IP_CITY_DB_VERSION,
            city_count: 2,
            range_count: 2,
            cities: vec![
                PackedCityRecord {
                    city_name: "Paris".to_string(),
                    country_code: "FR".to_string(),
                    latitude: 48.8566,
                    longitude: 2.3522,
                },
                PackedCityRecord {
                    city_name: "Lyon".to_string(),
                    country_code: "FR".to_string(),
                    latitude: 45.7640,
                    longitude: 4.8357,
                },
            ],
            ranges: vec![
                PackedRangeRecord {
                    ip_range_start: u32::from(Ipv4Addr::new(1, 0, 0, 0)),
                    ip_range_end: u32::from(Ipv4Addr::new(1, 0, 0, 255)),
                    city_id: 0,
                },
                PackedRangeRecord {
                    ip_range_start: u32::from(Ipv4Addr::new(1, 0, 1, 0)),
                    ip_range_end: u32::from(Ipv4Addr::new(1, 0, 1, 255)),
                    city_id: 1,
                },
            ],
        }
    }

    #[test]
    fn lookup_finds_matching_range() {
        let lookup = PackedIpCityLookup::from_db(sample_db()).unwrap();
        let found = lookup.find_ipv4(Ipv4Addr::new(1, 0, 1, 23)).unwrap();

        assert_eq!(found.city_id, 1);
        assert_eq!(found.city.city_name, "Lyon");
        assert_eq!(found.range_start, Ipv4Addr::new(1, 0, 1, 0));
        assert_eq!(found.range_end, Ipv4Addr::new(1, 0, 1, 255));
    }

    #[test]
    fn lookup_returns_none_for_gap() {
        let mut db = sample_db();
        db.ranges[1].ip_range_start = u32::from(Ipv4Addr::new(1, 0, 2, 0));
        db.ranges[1].ip_range_end = u32::from(Ipv4Addr::new(1, 0, 2, 255));
        let lookup = PackedIpCityLookup::from_db(db).unwrap();
        assert!(lookup.find_ipv4(Ipv4Addr::new(1, 0, 1, 23)).is_none());
    }

    #[test]
    fn lz4_round_trip_preserves_lookup() {
        let bytes = sample_db().encode_lz4().unwrap();
        let lookup = PackedIpCityLookup::from_lz4_bytes(&bytes).unwrap();
        assert_eq!(
            lookup
                .find_ipv4(Ipv4Addr::new(1, 0, 0, 42))
                .unwrap()
                .city
                .city_name,
            "Paris"
        );
    }

    #[test]
    fn validate_rejects_invalid_city_id() {
        let mut db = sample_db();
        db.ranges[0].city_id = 9;
        let error = PackedIpCityLookup::from_db(db).unwrap_err();
        assert!(matches!(error, PackedIpCityError::InvalidCityId { .. }));
    }

    #[test]
    fn validate_rejects_unsorted_ranges() {
        let mut db = sample_db();
        db.ranges.swap(0, 1);
        let error = PackedIpCityLookup::from_db(db).unwrap_err();
        assert!(matches!(error, PackedIpCityError::UnsortedRange { .. }));
    }

    #[test]
    fn paged_lookup_finds_matching_range() {
        let flat = sample_db();
        let paged = PagedPackedIpCityDb::from_flat(&flat, 1).unwrap();
        let lookup = PagedPackedIpCityLookup::from_db(paged).unwrap();
        let found = lookup.find_ipv4(Ipv4Addr::new(1, 0, 1, 23)).unwrap();

        assert_eq!(found.city_id, 1);
        assert_eq!(found.city().city_name, "Lyon");
    }

    #[test]
    fn paged_round_trip_preserves_lookup() {
        let flat = sample_db();
        let paged = PagedPackedIpCityDb::from_flat(&flat, 1).unwrap();
        let bytes = paged.encode_bytes().unwrap();
        let lookup = PagedPackedIpCityLookup::from_bytes(&bytes).unwrap();

        assert_eq!(
            lookup
                .find_ipv4(Ipv4Addr::new(1, 0, 0, 42))
                .unwrap()
                .city()
                .city_name,
            "Paris"
        );
    }

    #[test]
    fn paged_decode_still_accepts_legacy_headerless_payloads() {
        let flat = sample_db();
        let paged = PagedPackedIpCityDb::from_flat(&flat, 1).unwrap();
        let legacy_bytes = to_bytes::<RkyvError>(&paged).unwrap().to_vec();
        let lookup = PagedPackedIpCityLookup::from_bytes(&legacy_bytes).unwrap();

        assert_eq!(
            lookup
                .find_ipv4(Ipv4Addr::new(1, 0, 1, 42))
                .unwrap()
                .city()
                .city_name,
            "Lyon"
        );
    }

    #[test]
    fn paged_validate_rejects_bad_block_offset() {
        let flat = sample_db();
        let mut paged = PagedPackedIpCityDb::from_flat(&flat, 1).unwrap();
        paged.city_blocks[0].compressed_offset = u32::MAX;
        let error = PagedPackedIpCityLookup::from_db(paged).unwrap_err();
        assert!(matches!(
            error,
            PackedIpCityError::InvalidBlockOffset { .. }
        ));
    }
}
