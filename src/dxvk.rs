use sha1::{
    Sha1,
    Digest,
};
use std::{
    collections::HashSet,
    hash::{
        Hash,
        Hasher,
    },
    num::NonZeroU32,
    io::{
        self,
        Read,
        Write,
    },
    fmt,
    path::Path,
    fs,
};
use byteorder::{
    ReadBytesExt,
    WriteBytesExt,
    NativeEndian,
};
use crate::{
    read::FromReader,
};

pub type Sha1Hash = [u8; HASH_SIZE];
pub const LEGACY_VERSION: u32 = 7;
pub const HASH_SIZE: usize = 20;
pub const MAGIC_STRING: [u8; 4] = *b"DXVK";
const SHA1_EMPTY: Sha1Hash = [
    218, 57, 163, 238, 94, 107, 75, 13, 50, 85, 191, 239, 149, 96, 24, 144, 175, 216, 7, 9
];
type DxvkEndian = NativeEndian;
type EntryHash = [u8; HASH_SIZE];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DxvkStateCacheEdition {
    Standard,
    Legacy
}

impl Default for DxvkStateCacheEdition {
    #[inline(always)]
    fn default() -> Self {
        DxvkStateCacheEdition::Standard
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DxvkStateCacheHeader {
    pub magic:      [u8; 4],
    pub version:    NonZeroU32,
    pub entry_size: u32
}

impl DxvkStateCacheHeader {
    pub const fn new(version: NonZeroU32, entry_size: u32) -> Self {
        DxvkStateCacheHeader {
            magic: MAGIC_STRING,
            version: version,
            entry_size: entry_size,
        }
    }

    #[inline]
    pub fn edition(&self) -> DxvkStateCacheEdition {
        if self.version.get() > LEGACY_VERSION {
            DxvkStateCacheEdition::Standard
        } else {
            DxvkStateCacheEdition::Legacy
        }
    }

    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        writer.write_all(&MAGIC_STRING)?;
        writer.write_u32::<DxvkEndian>(self.version.get())?;
        writer.write_u32::<DxvkEndian>(self.entry_size)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Magic string mismatch")]
    MagicStringMismatch,
    #[error("Header contained invalid zero version")]
    InvalidVersion,
}

impl FromReader for DxvkStateCacheHeader {
    type Error = HeaderError;
    fn from_reader<R>(mut reader: R) -> Result<Self, Self::Error>
    where
        R: Read,
    {
        Ok(DxvkStateCacheHeader {
            magic: {
                let mut magic = [0u8; 4];
                reader.read_exact(&mut magic)?;
                if magic != MAGIC_STRING {
                    return Err(HeaderError::MagicStringMismatch);
                }
                magic
            },
            version: {
                let v = reader.read_u32::<DxvkEndian>()?;
                NonZeroU32::new(v)
                    .map(Ok)
                    .unwrap_or(Err(HeaderError::InvalidVersion))?
            },
            entry_size: reader.read_u32::<DxvkEndian>()?,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DxvkStateCacheEntryHeader {
    pub stage_mask: u8,
    pub entry_size: u32
}

impl fmt::Debug for DxvkStateCacheEntryHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DxvkStateCacheEntryHeader")
            .field("stage_mask", &format_args!("{:#b}", self.stage_mask))
            .field("entry_size", &self.entry_size)
            .finish()
    }
}

impl FromReader for DxvkStateCacheEntryHeader {
    type Error = io::Error;
    fn from_reader<R>(mut reader: R) -> Result<Self, Self::Error>
    where
        R: Read,
    {
        Ok(DxvkStateCacheEntryHeader {
            stage_mask: reader.read_u8()?,
            entry_size: reader.read_u24::<DxvkEndian>()?,
        })
    }
}

impl DxvkStateCacheEntryHeader {
    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        writer.write_u8(self.stage_mask)?;
        writer.write_u24::<DxvkEndian>(self.entry_size)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct DxvkStateCacheEntry {
    pub header: Option<DxvkStateCacheEntryHeader>,
    pub hash:   [u8; HASH_SIZE],
    pub data:   Vec<u8>
}

#[derive(Debug, thiserror::Error)]
pub enum EntryError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Entry invalid due to hash mismatch")]
    HashMismatch,
}

impl DxvkStateCacheEntry {
    fn from_reader_legacy<R>(mut reader: R, size: usize) -> Result<Self, io::Error>
    where
        R: Read,
    {
        let mut entry = DxvkStateCacheEntry::with_length(size);
        reader.read_exact(&mut entry.data)?;
        reader.read_exact(&mut entry.hash)?;
        Ok(entry)
    }

    fn from_reader_standard<R>(mut reader: R) -> Result<Self, io::Error>
    where
        R: Read,
    {
        let header = DxvkStateCacheEntryHeader::from_reader(&mut reader)?;
        let mut entry = DxvkStateCacheEntry::with_header(header);
        reader.read_exact(&mut entry.hash)?;
        reader.read_exact(&mut entry.data)?;
        Ok(entry)
    }

    pub fn from_reader<R>(reader: R, top_header: &DxvkStateCacheHeader) -> Result<Self, EntryError>
    where
        R: Read,
    {
        let ret = match top_header.edition() {
            DxvkStateCacheEdition::Standard =>
                Self::from_reader_standard(reader),
            DxvkStateCacheEdition::Legacy =>
                Self::from_reader_legacy(reader, top_header.entry_size as usize),
        }?;
        if !ret.is_valid() {
            return Err(EntryError::HashMismatch);
        }
        Ok(ret)
    }

    fn write_standard<W>(&self, mut writer: W) -> Result<(), io::Error>
    where
        W: Write,
    {
        if let Some(h) = self.header.as_ref() {
            h.write_to(&mut writer)?;
        }
        writer.write_all(&self.hash)?;
        writer.write_all(&self.data)?;
        Ok(())
    }

    fn write_legacy<W>(&self, mut writer: W) -> Result<(), io::Error>
    where
        W: Write,
    {
        writer.write_all(&self.hash)
            .and_then(|_| writer.write_all(&self.data))
    }

    pub fn write_to<W: Write>(&self, w: W, edition: DxvkStateCacheEdition) -> Result<(), io::Error> {
        match edition {
            DxvkStateCacheEdition::Legacy => self.write_legacy(w),
            DxvkStateCacheEdition::Standard => self.write_standard(w),
        }
    }

    #[inline(always)]
    pub fn hash_display<'a>(&'a self) -> HashDisplay<'a> {
        HashDisplay(&self.hash)
    }
}

impl DxvkStateCacheEntry {
    fn with_length(length: usize) -> Self {
        DxvkStateCacheEntry {
            data:   vec![0; length - HASH_SIZE],
            hash:   [0; HASH_SIZE],
            header: None
        }
    }

    fn with_header(header: DxvkStateCacheEntryHeader) -> Self {
        DxvkStateCacheEntry {
            data:   vec![0; header.entry_size as usize],
            hash:   [0; HASH_SIZE],
            header: Some(header)
        }
    }

    pub fn is_valid(&self) -> bool {
        let mut hasher = Sha1::default();
        hasher.update(&self.data);
        if self.header.is_none() {
            hasher.update(&SHA1_EMPTY);
        }
        let hash = hasher.finalize();
        let hash: EntryHash = unsafe { std::mem::transmute(hash) };

        hash == self.hash
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("Error reading header: {0}")]
    ReadHeader(#[from] HeaderError),
    #[error("Error reading entry: {0}")]
    ReadEntry(#[from] EntryError),
    #[error("Duplicate entry in state cache")]
    DuplicateEntry,
}

#[derive(Debug, Clone)]
#[repr(transparent)]
pub struct EntryWrapper(DxvkStateCacheEntry);

impl Hash for EntryWrapper {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.0.data.as_slice());
    }
}

impl PartialEq for EntryWrapper {
    fn eq(&self, other: &EntryWrapper) -> bool {
        self.0.hash == other.0.hash
    }
}

impl Eq for EntryWrapper {}

impl From<DxvkStateCacheEntry> for EntryWrapper {
    #[inline(always)]
    fn from(e: DxvkStateCacheEntry) -> Self {
        EntryWrapper(e)
    }
}

impl EntryWrapper {
    #[inline(always)]
    pub fn unwrap(self) -> DxvkStateCacheEntry {
        self.0
    }
}

#[derive(Debug)]
pub struct DxvkStateCache {
    pub header: DxvkStateCacheHeader,
    pub entries: HashSet<EntryWrapper>,
}

impl FromReader for DxvkStateCache {
    type Error = ReadError;

    fn from_reader<R: Read>(mut reader: R) -> Result<Self, Self::Error> {
        let mut entries: HashSet<EntryWrapper> = HashSet::new();
        let header = DxvkStateCacheHeader::from_reader(&mut reader)?;
        let mut try_read_entry = || {
            match DxvkStateCacheEntry::from_reader(&mut reader, &header) {
                Ok(v) => Ok(Some(v)),
                Err(EntryError::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
                Err(e) => Err(e),
            }
        };
        while let Some(e) = try_read_entry()?.map(EntryWrapper::from) {
            if !entries.insert(e) {
                return Err(ReadError::DuplicateEntry);
            }
        }
        Ok(DxvkStateCache {
            header: header,
            entries: entries,
        })
    }
}

impl DxvkStateCache {
    pub fn write_to<W: Write>(&self, mut writer: W) -> Result<(), io::Error> {
        if self.entries.len() < 1 {
            return Err(io::Error::new(io::ErrorKind::Other, "No entries to write"));
        }
        self.header.write_to(&mut writer)?;
        let edition = self.header.edition();
        for e in self.entries.iter() {
            e.0.write_to(&mut writer, edition)?;
        }
        Ok(())
    }

    pub fn iter<'a>(&'a self) -> impl ExactSizeIterator<Item=&'a DxvkStateCacheEntry> + 'a {
        self.entries.iter().map(|v| &v.0)
    }

    pub fn from_file<P: AsRef<Path>>(p: P) -> Result<Self, ReadError> {
        fs::OpenOptions::new()
            .read(true)
            .open(p)
            .map(io::BufReader::new)
            .map_err(ReadError::from)
            .and_then(Self::from_reader)
    }
}

#[repr(transparent)]
pub struct HashDisplay<'a>(&'a [u8; HASH_SIZE]);

impl<'a> fmt::Display for HashDisplay<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HASH_STR_SIZE: usize = HASH_SIZE * 2;
        let mut buf = [0u8; HASH_STR_SIZE];
        {
            let mut w = io::Cursor::new(&mut buf as &mut [u8]);
            for &b in self.0 {
                write!(w, "{:02x}", b).unwrap();
            }
        }
        let s = unsafe { std::str::from_utf8_unchecked(&buf) };
        write!(f, "{}", s)
    }
}
