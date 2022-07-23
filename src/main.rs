mod dxvk;
mod error;
mod sep;
pub mod read;
mod logging;

use std::{
    env,
    ffi::OsStr,
    fs::File,
    io::{self, BufReader, BufWriter},
    path::{Path, PathBuf},
    num::NonZeroU32,
    cell::Cell,
    error::{
        Error as StdError,
    },
    process,
};
use clap::{
    crate_version,
    crate_authors,
    crate_description,
};

use dxvk::*;
use error::Error;
use linked_hash_map::LinkedHashMap;
use sep::Separated;
use read::FromReader;
use log::*;

#[derive(Debug, clap::Parser)]
#[clap(version = crate_version!(), author = crate_authors!(), about = crate_description!())]
struct AppConfig {
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    #[clap(about = "Merge multiple state-cache files together")]
    Merge(MergeConfig),
}

#[derive(Debug, clap::Args)]
struct MergeConfig {
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

struct LegacyMergeConfig {
    files:      Vec<PathBuf>,
    output:     PathBuf,
    header_info: Cell<Option<HeaderInfo>>,
}

impl From<MergeConfig> for LegacyMergeConfig {
    fn from(cfg: MergeConfig) -> Self {
        LegacyMergeConfig {
            output: cfg.output,
            files: cfg.files,
            header_info: Cell::new(None),
        }
    }
}

impl LegacyMergeConfig {
    pub fn check_header(&self, header: &DxvkStateCacheHeader) -> Result<(), Error> {
        match self.header_info.get() {
            None => {
                self.header_info.set(Some(HeaderInfo::from(header)));
                info!("Detected state cache version v{}", header.version);
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

fn merge_main(config: MergeConfig) -> Result<(), Error> {
    let config: LegacyMergeConfig = config.into();

    info!("Merging files: {}", Separated::new(" ", || config.files().map(|p| p.display())));
    let mut entries = LinkedHashMap::new();
    for (i, path) in config.files.iter().enumerate() {
        let ext = path.extension().and_then(OsStr::to_str);
        if ext != Some("dxvk-cache") {
            return Err(Error::invalid_input_extension(ext.map(String::from)));
        }

        let file = File::open(path)?;
        let mut reader = BufReader::new(file);

        let header = DxvkStateCacheHeader::from_reader(&mut reader)?;
        config.check_header(&header)?;

        let mut omitted = 0;
        let entries_len = entries.len();
        info!(
            "Merging {} ({}/{})... ",
            path.file_name().and_then(OsStr::to_str).unwrap(),
            i + 1,
            config.files.len()
        );
        loop {
            let res = DxvkStateCacheEntry::from_reader(&mut reader, &header);
            match res {
                Ok(e) => {
                    entries.insert(e.hash, e);
                },
                Err(EntryError::HashMismatch) => {
                    omitted += 1;
                },
                Err(EntryError::Io(ref e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(EntryError::Io(e)) => return Err(e.into()),
            }
        }
        info!("\t{} new entries", entries.len() - entries_len);
        if omitted > 0 {
            warn!("\t{} entries are omitted as invalid", omitted);
        }
    }

    if entries.is_empty() {
        return Err(Error::NoEntriesFound);
    }

    info!(
        "Writing {} entries to file {}",
        entries.len(),
        config.output.file_name().and_then(OsStr::to_str).unwrap()
    );

    let header: DxvkStateCacheHeader = config.header_info.get().unwrap().into();

    let file = File::create(&config.output)?;
    let mut writer = BufWriter::new(file);
    header.write_to(&mut writer)?;
    let write_edition = header.edition();
    for (_, entry) in &entries {
        entry.write_to(&mut writer, write_edition)?;
    }

    debug!("Finished");

    Ok(())
}

fn main_e() -> Result<(), Box<dyn StdError + 'static>> {
    use clap::Parser;
    logging::init();
    let config = AppConfig::parse();
    match config.command {
        Command::Merge(cfg) => merge_main(cfg).map_err(From::from)
    }
}

fn main() {
    if let Err(e) = main_e() {
        error!("{}", e);
        process::exit(1);
    }
}
