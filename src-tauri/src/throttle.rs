//! Optional throughput limit for the backup target volume.
//!
//! One process-wide token bucket is active only while a backup, verification or
//! restore runs, and only for files that live on the target volume (compared by
//! device number). Rust read/write loops pass through [`ThrottledReader`] and
//! [`copy_file`]; the archive writer `tar -c` is governed by pausing its process
//! group (SIGSTOP/SIGCONT) whenever the archive file grows faster than allowed.
//! A stopped child is always continued before it is terminated or abandoned.
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) const MIN_MB_PER_S: u32 = 1;
pub(crate) const MAX_MB_PER_S: u32 = 5000;
pub(crate) const DEFAULT_MB_PER_S: u32 = 80;
const BYTES_PER_MB: u64 = 1_000_000;
/// The bucket may bank at most this much idle time as a burst allowance.
const BURST: Duration = Duration::from_millis(500);
/// Sleep slices stay short so cancellation is noticed quickly.
const SLICE: Duration = Duration::from_millis(50);

pub(crate) fn validate_mb_per_s(mb_per_s: u32) -> Result<(), String> {
    if (MIN_MB_PER_S..=MAX_MB_PER_S).contains(&mb_per_s) {
        Ok(())
    } else {
        Err(format!(
            "Durchsatzbegrenzung muss zwischen {MIN_MB_PER_S} und {MAX_MB_PER_S} MB/s liegen"
        ))
    }
}

/// Deterministic token bucket. Tokens may go negative (debt); the caller waits
/// until the debt is repaid by elapsed time.
#[derive(Debug)]
pub(crate) struct Bucket {
    bytes_per_sec: u64,
    tokens: f64,
    last: Instant,
}
impl Bucket {
    pub fn new(bytes_per_sec: u64, now: Instant) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec.max(1),
            tokens: 0.0,
            last: now,
        }
    }
    fn refill(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        let cap = self.bytes_per_sec as f64 * BURST.as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.bytes_per_sec as f64).min(cap);
    }
    /// Consume `bytes` and return how long the caller has to wait before the
    /// bucket is balanced again (zero when the bytes were covered by tokens).
    pub fn debit(&mut self, bytes: u64, now: Instant) -> Duration {
        self.refill(now);
        self.tokens -= bytes as f64;
        self.wait()
    }
    /// Remaining wait without consuming anything.
    pub fn wait_at(&mut self, now: Instant) -> Duration {
        self.refill(now);
        self.wait()
    }
    fn wait(&self) -> Duration {
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.bytes_per_sec as f64)
        }
    }
}

struct Active {
    dev: u64,
    mb_per_s: u32,
    bucket: Arc<Mutex<Bucket>>,
}
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

fn cancelled() -> bool {
    crate::BACKUP_CANCELLED.load(Ordering::SeqCst) || crate::VERIFY_CANCELLED.load(Ordering::SeqCst)
}

/// Enables the limit for the volume containing `target` for the lifetime of the guard.
pub(crate) struct Guard(());
impl Drop for Guard {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
    }
}
pub(crate) fn activate(
    config: &crate::BackupConfig,
    target: &Path,
) -> Result<Option<Guard>, String> {
    if !config.throttle_enabled {
        return Ok(None);
    }
    validate_mb_per_s(config.throttle_mb_per_s)?;
    let dev = existing_dev(target)?;
    let mut active = ACTIVE.lock().unwrap();
    if active.is_some() {
        return Err("Durchsatzbegrenzung ist bereits aktiv".into());
    }
    *active = Some(Active {
        dev,
        mb_per_s: config.throttle_mb_per_s,
        bucket: Arc::new(Mutex::new(Bucket::new(
            u64::from(config.throttle_mb_per_s) * BYTES_PER_MB,
            Instant::now(),
        ))),
    });
    Ok(Some(Guard(())))
}
#[cfg(test)]
pub(crate) fn activate_for_tests(path: &Path, mb_per_s: u32) -> Option<Guard> {
    let mut config = crate::BackupConfig::default();
    config.throttle_enabled = true;
    config.throttle_mb_per_s = mb_per_s;
    activate(&config, path).unwrap()
}

fn existing_dev(path: &Path) -> Result<u64, String> {
    let existing = crate::restore::resolve_existing_ancestor(path)?;
    let mut probe = existing.as_path();
    while !probe.exists() {
        probe = probe.parent().ok_or("Speicherziel fehlt")?;
    }
    std::fs::metadata(probe)
        .map(|m| m.dev())
        .map_err(|e| format!("{}: {e}", probe.display()))
}

/// Human-readable description of the active limit, for logs.
pub(crate) fn describe() -> Option<String> {
    ACTIVE
        .lock()
        .unwrap()
        .as_ref()
        .map(|a| format!("Durchsatzbegrenzung aktiv: {} MB/s", a.mb_per_s))
}

/// Shared handle to the active bucket; `None` when no limit applies to `path`.
#[derive(Clone)]
pub(crate) struct Limiter {
    bucket: Arc<Mutex<Bucket>>,
    mb_per_s: u32,
}
pub(crate) fn limiter_for(path: &Path) -> Option<Limiter> {
    let active = ACTIVE.lock().unwrap();
    let active = active.as_ref()?;
    let dev = std::fs::metadata(path).ok()?.dev();
    (dev == active.dev).then(|| Limiter {
        bucket: active.bucket.clone(),
        mb_per_s: active.mb_per_s,
    })
}
impl Limiter {
    pub fn mb_per_s(&self) -> u32 {
        self.mb_per_s
    }
    fn debit(&self, bytes: u64) -> Duration {
        self.bucket.lock().unwrap().debit(bytes, Instant::now())
    }
    fn pending(&self) -> Duration {
        self.bucket.lock().unwrap().wait_at(Instant::now())
    }
    /// Account for `bytes` and block until the limit allows continuing.
    pub fn acquire(&self, bytes: u64) -> io::Result<()> {
        let mut wait = self.debit(bytes);
        while !wait.is_zero() {
            if cancelled() {
                return Err(io::Error::other("Vorgang abgebrochen"));
            }
            std::thread::sleep(wait.min(SLICE));
            wait = self.pending();
        }
        Ok(())
    }
}

/// Reader that charges every byte to the target limit (pass-through when unlimited).
pub(crate) struct ThrottledReader<R> {
    inner: R,
    limiter: Option<Limiter>,
}
impl<R: Read> ThrottledReader<R> {
    pub fn new(inner: R, limiter: Option<Limiter>) -> Self {
        Self { inner, limiter }
    }
}
impl<R: Read> Read for ThrottledReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(limiter) = &self.limiter {
            if n > 0 {
                limiter.acquire(n as u64)?;
            }
        }
        Ok(n)
    }
}
pub(crate) fn open_throttled(path: &Path) -> io::Result<ThrottledReader<std::fs::File>> {
    let file = std::fs::File::open(path)?;
    Ok(ThrottledReader::new(file, limiter_for(path)))
}

/// `fs::copy` replacement for regular files: same permissions, limited when
/// either side lives on the target volume. Never follows special files.
pub(crate) fn copy_file(source: &Path, target: &Path) -> io::Result<u64> {
    use std::io::Write;
    let parent = target.parent().unwrap_or(Path::new("."));
    let limiter = limiter_for(source).or_else(|| limiter_for(parent));
    if limiter.is_none() {
        return std::fs::copy(source, target);
    }
    let metadata = std::fs::metadata(source)?;
    let mut reader = ThrottledReader::new(std::fs::File::open(source)?, limiter);
    let mut writer = std::fs::File::create(target)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buffer[..n])?;
        total += n as u64;
    }
    writer.set_permissions(metadata.permissions())?;
    Ok(total)
}

/// Pauses a child's process group while its output file outruns the limit.
/// Progress is reported as written bytes; paused time is returned so callers
/// can extend wall-clock deadlines accordingly.
pub(crate) struct ChildGovernor {
    output: PathBuf,
    limiter: Limiter,
    pgid: i32,
    seen: u64,
    paused_since: Option<Instant>,
    paused_total: Duration,
    reported: Instant,
}
impl ChildGovernor {
    /// `None` when no limit applies to the output's volume.
    pub fn new(output: &Path) -> Option<Self> {
        let parent = output.parent().unwrap_or(Path::new("."));
        let limiter = limiter_for(parent)?;
        Some(Self {
            output: output.to_path_buf(),
            limiter,
            pgid: 0,
            seen: 0,
            paused_since: None,
            paused_total: Duration::ZERO,
            reported: Instant::now(),
        })
    }
    pub fn attach(&mut self, pid: u32) {
        self.pgid = pid as i32;
    }
    pub fn paused_total(&self) -> Duration {
        self.paused_total
            + self
                .paused_since
                .map(|since| since.elapsed())
                .unwrap_or_default()
    }
    fn signal(&self, signal: libc::c_int) {
        if self.pgid > 0 {
            unsafe {
                libc::kill(-self.pgid, signal);
            }
        }
    }
    /// Call periodically while the child runs.
    pub fn tick(&mut self) {
        let size = std::fs::metadata(&self.output).map(|m| m.len()).unwrap_or(self.seen);
        let grown = size.saturating_sub(self.seen);
        self.seen = size;
        let wait = if grown > 0 {
            self.limiter.debit(grown)
        } else {
            self.limiter.pending()
        };
        if wait.is_zero() {
            self.resume();
        } else if self.paused_since.is_none() {
            self.signal(libc::SIGSTOP);
            self.paused_since = Some(Instant::now());
        }
        if self.reported.elapsed() >= Duration::from_millis(500) {
            self.reported = Instant::now();
            crate::work_progress::detail(
                format!(
                    "{:.1} MiB geschrieben · gedrosselt auf {} MB/s",
                    self.seen as f64 / (1024.0 * 1024.0),
                    self.limiter.mb_per_s()
                ),
                false,
            );
        }
    }
    /// Continue the group if it is paused. Must precede SIGTERM: a stopped
    /// process does not handle termination signals until it runs again.
    pub fn resume(&mut self) {
        if let Some(since) = self.paused_since.take() {
            self.paused_total += since.elapsed();
            self.signal(libc::SIGCONT);
        }
    }
}
impl Drop for ChildGovernor {
    fn drop(&mut self) {
        self.resume();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_cancel() {
        crate::BACKUP_CANCELLED.store(false, Ordering::SeqCst);
        crate::VERIFY_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn bucket_charges_debt_and_repays_it_with_time() {
        let start = Instant::now();
        let mut bucket = Bucket::new(1_000_000, start);
        assert_eq!(bucket.debit(0, start), Duration::ZERO);
        // 2 MB at 1 MB/s with an empty bucket: two seconds of debt.
        let wait = bucket.debit(2_000_000, start);
        assert!((wait.as_secs_f64() - 2.0).abs() < 1e-6, "{wait:?}");
        let later = start + Duration::from_millis(1500);
        let wait = bucket.wait_at(later);
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-6, "{wait:?}");
        assert_eq!(bucket.wait_at(start + Duration::from_secs(2)), Duration::ZERO);
    }

    #[test]
    fn idle_time_is_banked_only_up_to_the_burst_allowance() {
        let start = Instant::now();
        let mut bucket = Bucket::new(1_000_000, start);
        let later = start + Duration::from_secs(60);
        // Half a second of burst (500 kB) is free; the remaining 500 kB cost 0.5 s.
        let wait = bucket.debit(1_000_000, later);
        assert!((wait.as_secs_f64() - 0.5).abs() < 1e-6, "{wait:?}");
    }

    #[test]
    fn validation_bounds_are_enforced() {
        assert!(validate_mb_per_s(0).is_err());
        assert!(validate_mb_per_s(MIN_MB_PER_S).is_ok());
        assert!(validate_mb_per_s(MAX_MB_PER_S).is_ok());
        assert!(validate_mb_per_s(MAX_MB_PER_S + 1).is_err());
    }

    #[test]
    fn inactive_limit_never_waits_and_reader_passes_bytes_unchanged() {
        reset_cancel();
        assert!(ACTIVE.lock().unwrap().is_none());
        assert!(limiter_for(Path::new("/")).is_none());
        let data: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let mut reader = ThrottledReader::new(&data[..], None);
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn active_limit_applies_only_to_the_target_device_and_slows_reads() {
        reset_cancel();
        let dir = std::env::temp_dir().join(format!("throttle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.bin");
        std::fs::write(&file, vec![7u8; 3_000_000]).unwrap();
        {
            let _guard = activate_for_tests(&dir, 4).unwrap();
            assert_eq!(describe().unwrap(), "Durchsatzbegrenzung aktiv: 4 MB/s");
            assert!(limiter_for(&file).is_some());
            // /dev lives on a different (devfs) device than the temporary directory.
            assert!(limiter_for(Path::new("/dev/null")).is_none());
            let started = Instant::now();
            let mut out = Vec::new();
            open_throttled(&file).unwrap().read_to_end(&mut out).unwrap();
            assert_eq!(out.len(), 3_000_000);
            // 3 MB at 4 MB/s with a 2 MB burst allowance: at least ~0.25 s.
            assert!(started.elapsed() >= Duration::from_millis(200), "{:?}", started.elapsed());
        }
        assert!(limiter_for(&file).is_none(), "guard must clear the limit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancellation_interrupts_a_waiting_reader() {
        reset_cancel();
        let dir = std::env::temp_dir().join(format!("throttle-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.bin");
        std::fs::write(&file, vec![1u8; 4_000_000]).unwrap();
        let _guard = activate_for_tests(&dir, 1).unwrap();
        let handle = std::thread::spawn({
            let file = file.clone();
            move || {
                let mut out = Vec::new();
                open_throttled(&file).unwrap().read_to_end(&mut out)
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        crate::VERIFY_CANCELLED.store(true, Ordering::SeqCst);
        let result = handle.join().unwrap();
        reset_cancel();
        assert!(result.is_err(), "reader must stop instead of finishing 4 s later");
        drop(_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_file_keeps_contents_and_permissions_under_a_limit() {
        use std::os::unix::fs::PermissionsExt;
        reset_cancel();
        let dir = std::env::temp_dir().join(format!("throttle-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.bin");
        let target = dir.join("target.bin");
        let data: Vec<u8> = (0..1_500_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &data).unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        let _guard = activate_for_tests(&dir, 50).unwrap();
        assert_eq!(copy_file(&source, &target).unwrap(), data.len() as u64);
        assert_eq!(std::fs::read(&target).unwrap(), data);
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o640);
        drop(_guard);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
