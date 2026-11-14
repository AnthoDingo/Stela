//! `stela` as a library, for a program with its own window (`stela-gui`):
//! the same commands and checks, run in that program without a terminal.
//! What a command prints still goes to stdout and stderr.
use super::{Request, host, wifi};
use anyhow::{Result, anyhow};
use clap::Parser;
use host::Host;
use std::path::PathBuf;

pub use crate::progress::Progress;

/// The step the running command is at, while it has one: a download, a
/// copy or a check, with its bytes when it counts them.
pub fn progress() -> Option<Progress> {
    crate::progress::running()
}

/// A removable disk Stela may use.
pub struct Stick {
    /// Its whole disk as a command's STICK takes it.
    pub path: String,
    /// Its model.
    pub name: String,
    /// Its size as `stela` shows it, such as `64.0 GB`.
    pub size: String,
    /// It holds a Stela setup.
    pub stela: bool,
}

/// The removable disks plugged in, Stela sticks first.
pub fn sticks() -> Result<Vec<Stick>> {
    let mut sticks: Vec<Stick> = host::System
        .disks()?
        .into_iter()
        .filter(host::Disk::erasable)
        .map(|disk| Stick {
            path: format!("{}{}", host::DISK_PREFIX, disk.identifier),
            stela: super::setup_volume(&disk).is_some(),
            size: host::size_text(disk.bytes),
            name: disk.name,
        })
        .collect();
    sticks.sort_by_key(|stick| !stick.stela);
    Ok(sticks)
}

/// Where `stela` keeps the macOS images it downloads to this computer:
/// `create --download-dir` with it downloads there first, and every later
/// `create` finds the image there instead of downloading it again.
pub fn image_cache() -> PathBuf {
    super::download_dir()
}

/// The macOS restore images in [`image_cache`], with their sizes. A
/// command checks an image before it uses one.
pub fn cached_images() -> Vec<(PathBuf, u64)> {
    let Ok(entries) = std::fs::read_dir(image_cache()) else {
        return Vec::new();
    };
    let mut images: Vec<(PathBuf, u64)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_name().to_str().is_some_and(|name| {
                name.starts_with("UniversalMac_") && name.ends_with("_Restore.ipsw")
            })
        })
        .filter_map(|entry| Some((entry.path(), entry.metadata().ok()?.len())))
        .collect();
    images.sort();
    images
}

/// Whether this process may write disks: an elevated administrator on
/// Windows, root elsewhere.
pub fn is_administrator() -> bool {
    super::elevation::is_administrator()
}

/// Run the command `args` names, as `stela` takes them after its name,
/// with `--yes`. `passphrase` is `--wifi`'s, which a terminal would ask
/// for. A command that needs more rights than this process has stops.
pub fn run(args: &[String], passphrase: Option<String>) -> Result<()> {
    let words = std::iter::once("stela").chain(args.iter().map(String::as_str));
    let mut request: Request = crate::Cli::try_parse_from(words)?.request()?;
    if let Some(passphrase) = passphrase {
        wifi::check_passphrase(&passphrase).map_err(|error| anyhow!(error))?;
        request.wifi_passphrase = Some(passphrase);
    }
    request.embedded = true;
    request.yes = true;
    request
        .run()
        .map_err(|error| match error.is::<super::home::Due>() {
            true => anyhow!("something needs doing: the report above names the command"),
            false => error,
        })
}
