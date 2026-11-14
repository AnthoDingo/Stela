//! Terminal-only progress; redirected commands keep their ordinary output.
//! Once the setup wizard has drawn its timeline, each phase continues it: a
//! running step on the rail, then the finished step it leaves behind. A
//! program that embeds stela reads the running phase with [`running`].
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use std::{
    io::{self, IsTerminal, Read, Write},
    path::Path,
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// The running phase, as a program embedding stela shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// What it does, such as `Downloading macOS onto USB`.
    pub label: String,
    /// Bytes done, of `total`.
    pub done: u64,
    /// Bytes to do; `None` for a phase without a size.
    pub total: Option<u64>,
}

/// The newest phase that has not ended, with its number: phases nest, and
/// one that ends after a newer one began changes nothing.
#[derive(Default)]
struct Tracker(Option<(u64, Progress)>);

impl Tracker {
    fn begin(&mut self, number: u64, label: &str, total: Option<u64>) {
        let progress = Progress {
            label: label.to_owned(),
            done: 0,
            total,
        };
        self.0 = Some((number, progress));
    }

    fn update(&mut self, number: u64, change: impl FnOnce(&mut Progress)) {
        if let Some((running, progress)) = self.0.as_mut()
            && *running == number
        {
            change(progress);
        }
    }

    fn end(&mut self, number: u64) {
        if self
            .0
            .as_ref()
            .is_some_and(|(running, _)| *running == number)
        {
            self.0 = None;
        }
    }
}

static CURRENT: Mutex<Tracker> = Mutex::new(Tracker(None));
static NUMBER: AtomicU64 = AtomicU64::new(0);

fn current() -> MutexGuard<'static, Tracker> {
    CURRENT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The running phase, if any.
pub(crate) fn running() -> Option<Progress> {
    current().0.as_ref().map(|(_, progress)| progress.clone())
}

/// Start phase `label`, of `total` bytes when known; its number.
fn begin(label: &str, total: Option<u64>) -> u64 {
    let number = NUMBER.fetch_add(1, Ordering::Relaxed);
    current().begin(number, label, total);
    number
}

fn update(number: u64, change: impl FnOnce(&mut Progress)) {
    current().update(number, change);
}

fn end(number: u64) {
    current().end(number);
}

/// The wizard timeline's styled glyphs: a running, finished and stopped
/// step, and the rail between steps.
pub(crate) struct Rail {
    pub(crate) active: String,
    pub(crate) done: String,
    pub(crate) stopped: String,
    pub(crate) bar: String,
}

static RAIL: OnceLock<Rail> = OnceLock::new();

/// Draw every later phase on `rail`.
pub(crate) fn follow(rail: Rail) {
    let _ = RAIL.set(rail);
}

/// The start of a step's first line: its glyph on the rail, if any.
fn step(glyph: fn(&Rail) -> &str) -> String {
    RAIL.get()
        .map(|rail| format!("{}  ", glyph(rail)))
        .unwrap_or_default()
}

/// The start of a step's later lines: the rail, if any.
fn detail() -> String {
    RAIL.get()
        .map(|rail| format!("{}  ", rail.bar))
        .unwrap_or_default()
}

/// The columns left free at the right edge.
const MARGIN: &str = "  ";

pub(crate) struct Phase {
    bar: Option<ProgressBar>,
    label: String,
    finished: bool,
    /// Its number in [`CURRENT`].
    number: u64,
}

impl Phase {
    pub(crate) fn bytes(label: impl Into<String>, bytes: u64) -> Self {
        let label = label.into();
        let bar = io::stderr().is_terminal().then(|| {
            let template = format!(
                "{}{{msg}}\n{}{{wide_bar:.173/239}} {{percent:>3}}%  {{bytes}}/{{total_bytes}}  \
                 {{bytes_per_sec}}  ETA {{eta_precise}}{MARGIN}",
                step(|rail| &rail.active),
                detail(),
            );
            let bar = ProgressBar::new(bytes).with_style(
                ProgressStyle::with_template(&template)
                    .expect("a fixed progress template")
                    .progress_chars("█▌░"),
            );
            bar.set_message(label.clone());
            bar.enable_steady_tick(Duration::from_millis(200));
            bar
        });
        let number = begin(&label, Some(bytes));
        Self {
            bar,
            label,
            finished: false,
            number,
        }
    }

    pub(crate) fn spinner(label: impl Into<String>) -> Self {
        let label = label.into();
        let bar = io::stderr().is_terminal().then(|| {
            let template = format!("{}{{msg}}  {{elapsed:.dim}}", step(|rail| &rail.active));
            let bar = ProgressBar::new_spinner().with_style(
                ProgressStyle::with_template(&template).expect("a fixed progress template"),
            );
            bar.set_message(label.clone());
            bar.enable_steady_tick(Duration::from_secs(1));
            bar
        });
        let number = begin(&label, None);
        Self {
            bar,
            label,
            finished: false,
            number,
        }
    }

    pub(crate) fn advance(&self, bytes: u64) {
        update(self.number, |progress| {
            progress.done = progress.done.saturating_add(bytes);
        });
        if let Some(bar) = &self.bar {
            bar.inc(bytes);
        }
    }

    pub(crate) fn position(&self, bytes: u64) {
        update(self.number, |progress| progress.done = bytes);
        if let Some(bar) = &self.bar {
            bar.set_position(bytes);
            bar.reset_eta();
        }
    }

    pub(crate) fn add_work(&self, bytes: u64) {
        update(self.number, |progress| {
            progress.total = Some(progress.total.unwrap_or(0).saturating_add(bytes));
        });
        if let Some(bar) = &self.bar {
            bar.set_length(bar.length().unwrap_or(0).saturating_add(bytes));
        }
    }

    pub(crate) fn finish(mut self) {
        end(self.number);
        if let Some(bar) = &self.bar {
            // A one-line summary replaces the finished timers. Off the
            // timeline, a step without a size leaves nothing behind.
            match (bar.length(), RAIL.get()) {
                (None, None) => bar.finish_and_clear(),
                (None, Some(_)) => {
                    bar.set_style(summary(|rail| &rail.done));
                    bar.finish_with_message(self.label.clone());
                }
                (Some(total), _) => {
                    bar.set_style(summary(|rail| &rail.done));
                    bar.finish_with_message(format!("{} · {}", self.label, HumanBytes(total)));
                }
            }
        }
        self.finished = true;
    }
}

/// A finished or stopped step: its glyph and message, then the rail.
fn summary(glyph: fn(&Rail) -> &str) -> ProgressStyle {
    let rail = RAIL
        .get()
        .map(|rail| format!("\n{}", rail.bar))
        .unwrap_or_default();
    ProgressStyle::with_template(&format!("{}{{msg}}{rail}", step(glyph)))
        .expect("a fixed progress template")
}

impl Drop for Phase {
    fn drop(&mut self) {
        end(self.number);
        if !self.finished
            && let Some(bar) = &self.bar
        {
            bar.set_style(summary(|rail| &rail.stopped));
            bar.abandon_with_message(format!("{} stopped", self.label));
        }
    }
}

pub(crate) fn copy(from: &Path, to: &Path, progress: &Phase) -> io::Result<()> {
    let mut input = std::fs::File::open(from)?;
    let mut output = std::fs::File::create(to)?;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = match input.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        progress.advance(count as u64);
    }
    output.sync_all()?;
    drop_cached(&output)
}

/// Drop `file`'s written pages from the cache after `sync_all`, so a check
/// that reads the file back reads what the disk holds rather than what was
/// written.
pub(crate) fn drop_cached(file: &std::fs::File) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: an advice call on a descriptor `file` keeps open.
        let error =
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        match error {
            0 => Ok(()),
            error => Err(io::Error::from_raw_os_error(error)),
        }
    }
    #[cfg(target_os = "macos")]
    {
        invalidate_mapped(file)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = file;
        Ok(())
    }
}

/// macOS keeps a synced file's written pages cached and has no advice that
/// drops them; `msync` with `MS_INVALIDATE` on a shared mapping of the file
/// does. The writer's descriptor is write-only, so the file is mapped
/// through its path, opened again for reading.
#[cfg(target_os = "macos")]
fn invalidate_mapped(file: &std::fs::File) -> io::Result<()> {
    use std::os::unix::{ffi::OsStrExt, io::AsRawFd};
    let length = usize::try_from(file.metadata()?.len())
        .map_err(|_| io::Error::from(io::ErrorKind::FileTooLarge))?;
    if length == 0 {
        return Ok(());
    }
    let mut path = [0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL-terminated, into
    // `path`, through a descriptor `file` keeps open.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful F_GETPATH NUL-terminated `path`.
    let path = unsafe { std::ffi::CStr::from_ptr(path.as_ptr()) };
    let reader = std::fs::File::open(std::ffi::OsStr::from_bytes(path.to_bytes()))?;
    // SAFETY: a new read-only shared mapping of `length` bytes of a file
    // `reader` keeps open; nothing else refers to the mapping.
    let map = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ,
            libc::MAP_SHARED,
            reader.as_raw_fd(),
            0,
        )
    };
    if map == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `map` is the mapping of `length` bytes made above, unmapped
    // only after the call.
    let invalidated = unsafe { libc::msync(map, length, libc::MS_INVALIDATE) };
    let error = io::Error::last_os_error();
    // SAFETY: unmaps the mapping made above, which nothing uses after this.
    unsafe { libc::munmap(map, length) };
    match invalidated {
        0 => Ok(()),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tracker_tests {
    use super::Tracker;

    fn seen(tracker: &Tracker) -> Option<(&str, u64, Option<u64>)> {
        let (_, progress) = tracker.0.as_ref()?;
        Some((progress.label.as_str(), progress.done, progress.total))
    }

    #[test]
    fn the_newest_phase_runs_until_it_ends() {
        let mut tracker = Tracker::default();
        tracker.begin(1, "Downloading", Some(100));
        tracker.update(1, |p| p.done += 30);
        tracker.update(1, |p| p.total = p.total.map(|t| t + 50));
        assert_eq!(seen(&tracker), Some(("Downloading", 30, Some(150))));
        tracker.begin(2, "Preparing", None);
        tracker.update(1, |p| p.done = 90);
        assert_eq!(seen(&tracker), Some(("Preparing", 0, None)));
        tracker.end(1);
        assert_eq!(seen(&tracker), Some(("Preparing", 0, None)));
        tracker.end(2);
        assert_eq!(seen(&tracker), None);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::{io::Write, os::unix::io::AsRawFd};

    /// The share of `path`'s pages the cache holds, in percent.
    fn resident(path: &std::path::Path) -> usize {
        let file = std::fs::File::open(path).unwrap();
        let length = file.metadata().unwrap().len() as usize;
        // SAFETY: sysconf takes no pointers.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let mut pages = vec![0 as libc::c_char; length.div_ceil(page)];
        // SAFETY: a read-only mapping of the open file, unmapped below;
        // mincore writes one byte for each of its `pages.len()` pages.
        unsafe {
            let map = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            );
            assert_ne!(map, libc::MAP_FAILED);
            assert_eq!(libc::mincore(map, length, pages.as_mut_ptr()), 0);
            libc::munmap(map, length);
        }
        let held = pages
            .iter()
            .filter(|&&state| i32::from(state) & libc::MINCORE_INCORE != 0)
            .count();
        held * 100 / pages.len()
    }

    /// A written, synced file is left cached by macOS; dropping it leaves
    /// none of it cached, on whatever file system holds `TMPDIR`.
    #[test]
    fn a_synced_file_s_pages_leave_the_cache() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("piece");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(&vec![0x5a; 8 << 20]).unwrap();
        file.sync_all().unwrap();
        assert!(resident(&path) > 50, "{}", resident(&path));
        super::drop_cached(&file).unwrap();
        assert_eq!(resident(&path), 0);
        // An empty file has nothing to drop.
        let empty = std::fs::File::create(temp.path().join("empty")).unwrap();
        super::drop_cached(&empty).unwrap();
    }
}
