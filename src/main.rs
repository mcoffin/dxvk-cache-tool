mod dxvk;
mod error;
mod sep;
pub mod read;
mod logging;

use std::{
    env,
    ffi::OsStr,
    fs::{
        self,
        File,
    },
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
    #[clap(about = "Print information about dxvk-cache files")]
    Inspect {
        #[clap(required = true, help = "Files to inspect")]
        files: Vec<PathBuf>,
    },
    #[clap(about = "read, and re-write a given state cache")]
    Jumble {
        input_file: PathBuf,
        output_file: PathBuf,
    },
    #[clap(about = "List SHA1 hashes of all entries in the given state caches")]
    ListEntries {
        #[clap(required = true, help = "dxvk-cache files")]
        files: Vec<PathBuf>,
    },
    #[clap(about = "List SHA1 hashes of all entries present in the first file but not the second")]
    Difference(DifferenceConfig),
}

#[derive(Debug, clap::Args)]
struct DifferenceConfig {
    first: PathBuf,
    second: PathBuf,
    #[clap(short, long = "output", help = "output filename - if set, the entries are written as a cache file here instead of printed")]
    output_file: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct MergeConfig {
    #[clap(short, long, default_value = "output.dxvk-cache", help = "Output file name")]
    output: PathBuf,
    #[clap(required = true, help = "Input files")]
    files: Vec<PathBuf>,
    #[clap(long, parse(from_flag))]
    dry_run: bool,
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

impl MergeConfig {
    fn run(self) -> Result<(), Error> {
        let dry_run = self.dry_run;
        let config: LegacyMergeConfig = self.into();

        info!("Merging files: {}", Separated::new(" ", || config.files().map(|p| p.display())));
        let mut entries = LinkedHashMap::new();
        for (i, path) in config.files.iter().enumerate() {
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

        if dry_run {
            info!("{} entries when merged", entries.len());
            return Ok(());
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
}

fn inspect<P: AsRef<Path>, Pfx: std::fmt::Display>(prefix: Option<&Pfx>, f: P) -> Result<(), ReadError> {
    let prefix = if let Some(prefix) = prefix {
        println!("{}:", prefix);
        "\t"
    } else {
        ""
    };
    let f = fs::OpenOptions::new()
        .read(true)
        .open(f)
        .map(BufReader::new)?;
    let cache = DxvkStateCache::from_reader(f)?;
    println!("{}version: {}", prefix, cache.header.version);
    println!("{}entries: {}", prefix, cache.entries.len());
    Ok(())
}

impl DifferenceConfig {
    fn run(self) -> Result<(), Box<dyn StdError + 'static>> {
        let mut fst = DxvkStateCache::from_file(self.first)?;
        let snd = DxvkStateCache::from_file(self.second)?;
        if fst.header.version != snd.header.version {
            return Err(Box::new(io::Error::new(io::ErrorKind::Other, format!("version mismatch: v{} != v{}", fst.header.version, snd.header.version))));
        }
        fst.entries = fst.entries.difference(&snd.entries)
            .map(Clone::clone)
            .collect();

        if let Some(output_file) = self.output_file {
            let f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .open(output_file)?;
            fst.write_to(f)?;
        } else {
            fst.iter().for_each(|entry| {
                println!("{}", entry.hash_display());
            });
        }
        Ok(())
    }
}

#[inline(always)]
fn run_main<F, E>(f: F)
where
    F: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    if let Err(e) = f() {
        error!("{}", e);
        process::exit(1);
    }
}

fn main() {
    logging::init();
    run_main(|| -> Result<(), Box<dyn StdError + 'static>> {
        use clap::Parser;
        let config = AppConfig::parse();
        match config.command {
            Command::Merge(cfg) => cfg.run().map_err(From::from),
            Command::Inspect { files } => {
                if files.len() == 1 {
                    inspect::<_, String>(None, files.iter().next().unwrap())?;
                } else {
                    for f in files.iter() {
                        inspect(Some(&f.display()), f)?;
                    }
                }
                Ok(())
            },
            Command::Jumble { input_file, output_file } => {
                let cache = DxvkStateCache::from_file(input_file)?;
                let f = fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .open(output_file)?;
                cache.write_to(f)?;
                Ok(())
            },
            Command::ListEntries { files } => {
                for f in files.iter() {
                    let cache = DxvkStateCache::from_file(f)?;
                    cache.iter().for_each(|entry| {
                        println!("{}", entry.hash_display());
                    });
                }
                Ok(())
            },
            Command::Difference(cfg) => cfg.run(),
        }
    })
}
