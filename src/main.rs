mod dxvk;
mod error;
mod sep;

use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, prelude::*, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::num::NonZeroU32;
use std::cell::Cell;
use clap::{
    crate_version,
    crate_authors,
    crate_description,
};

use dxvk::*;
use error::{Error, HeaderError};
use linked_hash_map::LinkedHashMap;
use sep::Separated;

#[derive(Debug, clap::Parser)]
#[clap(version = crate_version!(), author = crate_authors!(), about = crate_description!())]
struct ArgsConfig {
    #[clap(short, long, default_value = "output.dxvk-cache", help = "Output file name")]
    output: PathBuf,
    #[clap(required = true, help = "Input files")]
    files: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
struct HeaderInfo {
    version: NonZeroU32,
    entry_size: u32,
    edition: DxvkStateCacheEdition,
}

impl<'a> From<&'a DxvkStateCacheHeader> for HeaderInfo {
    #[inline(always)]
    fn from(header: &'a DxvkStateCacheHeader) -> Self {
        HeaderInfo {
            entry_size: header.entry_size,
            version: header.version,
            edition: header.edition(),
        }
    }
}

impl Into<DxvkStateCacheHeader> for HeaderInfo {
    #[inline(always)]
    fn into(self) -> DxvkStateCacheHeader {
        DxvkStateCacheHeader::new(self.version, self.entry_size)
    }
}

struct Config {
    files:      Vec<PathBuf>,
    output:     PathBuf,
    header_info: Cell<Option<HeaderInfo>>,
}

impl From<ArgsConfig> for Config {
    fn from(cfg: ArgsConfig) -> Self {
        Config {
            output: cfg.output,
            files: cfg.files,
            header_info: Cell::new(None),
        }
    }
}

impl Config {
    pub fn from_args() -> Self {
        use clap::Parser;
        ArgsConfig::parse().into()
    }

    pub fn check_header(&self, header: &DxvkStateCacheHeader) -> Result<(), Error> {
        match self.header_info.get() {
            None => {
                self.header_info.set(Some(HeaderInfo::from(header)));
                println!("Detected state cache version v{}", header.version);
                Ok(())
            },
            Some(HeaderInfo { version, .. }) if version != header.version =>
                Err(Error::version_mismatch(version, header.version)),
            Some(..) => Ok(()),
        }
    }

    #[inline(always)]
    pub fn files<'a>(&'a self) -> impl Iterator<Item=&'a Path> + 'a {
        self.files.iter().map(<PathBuf as AsRef<Path>>::as_ref)
    }
}

impl<R: Read> ReadEx for BufReader<R> {}
trait ReadEx: Read {
    fn read_u32(&mut self) -> io::Result<u32> {
        let mut buf = [0; 4];
        match self.read_exact(&mut buf) {
            Ok(_) => Ok((u32::from(buf[0]))
                + (u32::from(buf[1]) << 8)
                + (u32::from(buf[2]) << 16)
                + (u32::from(buf[3]) << 24)),
            Err(e) => Err(e)
        }
    }

    fn read_u24(&mut self) -> io::Result<u32> {
        let mut buf = [0; 3];
        match self.read_exact(&mut buf) {
            Ok(_) => Ok((u32::from(buf[0])) + (u32::from(buf[1]) << 8) + (u32::from(buf[2]) << 16)),
            Err(e) => Err(e)
        }
    }

    fn read_u8(&mut self) -> io::Result<u8> {
        let mut buf = [0; 1];
        match self.read_exact(&mut buf) {
            Ok(_) => Ok(buf[0]),
            Err(e) => Err(e)
        }
    }
}

impl<W: Write> WriteEx for BufWriter<W> {}
trait WriteEx: Write {
    fn write_u32(&mut self, n: u32) -> io::Result<()> {
        let mut buf = [0; 4];
        buf[0] = n as u8;
        buf[1] = (n >> 8) as u8;
        buf[2] = (n >> 16) as u8;
        buf[3] = (n >> 24) as u8;
        self.write_all(&buf)
    }

    fn write_u24(&mut self, n: u32) -> io::Result<()> {
        let mut buf = [0; 3];
        buf[0] = n as u8;
        buf[1] = (n >> 8) as u8;
        buf[2] = (n >> 16) as u8;
        self.write_all(&buf)
    }

    fn write_u8(&mut self, n: u8) -> io::Result<()> {
        let mut buf = [0; 1];
        buf[0] = n;
        self.write_all(&buf)
    }
}

fn main() -> Result<(), Error> {
    let config = Config::from_args();

    println!("Merging files: {}", Separated::new(" ", || config.files().map(|p| p.display())));
    let mut entries = LinkedHashMap::new();
    for (i, path) in config.files.iter().enumerate() {
        let ext = path.extension().and_then(OsStr::to_str);
        if ext != Some("dxvk-cache") {
            return Err(Error::invalid_input_extension(ext.map(String::from)));
        }

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        let header = read_header(&mut reader)?;
        config.check_header(&header)?;

        let mut omitted = 0;
        let entries_len = entries.len();
        print!(
            "Merging {} ({}/{})... ",
            path.file_name().and_then(OsStr::to_str).unwrap(),
            i + 1,
            config.files.len()
        );
        loop {
            let res = match header.edition() {
                DxvkStateCacheEdition::Standard => read_entry(&mut reader),
                DxvkStateCacheEdition::Legacy => {
                    read_entry_legacy(&mut reader, header.entry_size as usize)
                },
            };
            match res {
                Ok(e) => {
                    if e.is_valid() {
                        entries.insert(e.hash, e);
                    } else {
                        omitted += 1;
                    }
                },
                Err(Error::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e)
            }
        }
        println!("{} new entries", entries.len() - entries_len);
        if omitted > 0 {
            println!("{} entries are omitted as invalid", omitted);
        }
    }

    if entries.is_empty() {
        return Err(Error::NoEntriesFound);
    }

    println!(
        "Writing {} entries to file {}",
        entries.len(),
        config.output.file_name().and_then(OsStr::to_str).unwrap()
    );

    let header = config.header_info.get().unwrap().into();

    let file = File::create(&config.output)?;
    let mut writer = BufWriter::new(file);
    wrtie_header(&mut writer, header)?;
    for (_, entry) in &entries {
        match header.edition() {
            DxvkStateCacheEdition::Standard => write_entry(&mut writer, entry)?,
            DxvkStateCacheEdition::Legacy => write_entry_legacy(&mut writer, entry)?
        };
    }

    println!("Finished");

    Ok(())
}

fn read_header<R: Read>(reader: &mut BufReader<R>) -> Result<DxvkStateCacheHeader, HeaderError> {
    let ret = DxvkStateCacheHeader {
        magic:      {
            let mut magic = [0; 4];
            reader.read_exact(&mut magic)?;
            if magic != MAGIC_STRING {
                return Err(HeaderError::MagicStringMismatch);
            }
            magic
        },
        version:    {
            let v = reader.read_u32()?;
            NonZeroU32::new(v)
                .map(Ok)
                .unwrap_or(Err(HeaderError::InvalidVersion))?
        },
        entry_size: reader.read_u32()?
    };
    Ok(ret)
}

fn read_entry<R: Read>(reader: &mut BufReader<R>) -> Result<DxvkStateCacheEntry, Error> {
    let header = DxvkStateCacheEntryHeader {
        stage_mask: reader.read_u8()?,
        entry_size: reader.read_u24()? as u32
    };
    let mut entry = DxvkStateCacheEntry::with_header(header);
    reader.read_exact(&mut entry.hash)?;
    reader.read_exact(&mut entry.data)?;

    Ok(entry)
}

fn read_entry_legacy<R: Read>(
    reader: &mut BufReader<R>,
    size: usize
) -> Result<DxvkStateCacheEntry, Error> {
    let mut entry = DxvkStateCacheEntry::with_length(size);
    reader.read_exact(&mut entry.data)?;
    reader.read_exact(&mut entry.hash)?;

    Ok(entry)
}

fn wrtie_header<W: Write>(
    writer: &mut BufWriter<W>,
    header: DxvkStateCacheHeader
) -> Result<(), Error> {
    writer.write_all(&MAGIC_STRING)?;
    writer.write_u32(header.version.get())?;
    writer.write_u32(header.entry_size as u32)?;

    Ok(())
}

fn write_entry<W: Write>(
    writer: &mut BufWriter<W>,
    entry: &DxvkStateCacheEntry
) -> Result<(), Error> {
    if let Some(h) = &entry.header {
        writer.write_u8(h.stage_mask)?;
        writer.write_u24(h.entry_size)?;
    }
    writer.write_all(&entry.hash)?;
    writer.write_all(&entry.data)?;

    Ok(())
}

fn write_entry_legacy<W: Write>(
    writer: &mut BufWriter<W>,
    entry: &DxvkStateCacheEntry
) -> Result<(), Error> {
    writer.write_all(&entry.data)?;
    writer.write_all(&entry.hash)?;

    Ok(())
}
