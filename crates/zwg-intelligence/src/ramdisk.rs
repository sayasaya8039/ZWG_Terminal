//! RAM Disk acceleration — use ImDisk to create a high-speed RAM drive
//! for caches, scrollback, KV cache, and model files.
//!
//! RAM is ~10x faster than NVMe SSD for random I/O:
//!
//! | Medium | Sequential | Random 4K  |
//! |--------|-----------|------------|
//! | HDD    | ~100 MB/s | ~0.5 MB/s  |
//! | NVMe   | ~3.5 GB/s | ~50 MB/s   |
//! | RAM    | ~40 GB/s  | ~40 GB/s   |
//!
//! # Requirements
//!
//! - ImDisk Virtual Disk Driver installed (<https://ltr-data.se/opencode.html/#ImDisk>)
//! - No admin rights needed after driver installation
//!
//! # Usage
//!
//! ```no_run
//! use zwg_intelligence::ramdisk::{RamDisk, RamDiskConfig};
//!
//! let config = RamDiskConfig::default();
//! if let Some(rd) = RamDisk::create(config).ok().flatten() {
//!     let cache_dir = rd.path().join("cache");
//!     // Use cache_dir for high-speed I/O...
//!     // rd is auto-unmounted on drop
//! }
//! ```

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default RAM disk size in megabytes.
const DEFAULT_SIZE_MB: u32 = 512;

/// Default drive letter to try.
const DEFAULT_DRIVE_LETTER: char = 'R';

/// Fallback drive letters if the default is taken.
const FALLBACK_LETTERS: &[char] = &['Q', 'P', 'T', 'V', 'W', 'X', 'Y'];

/// ImDisk executable name.
const IMDISK_EXE: &str = "imdisk.exe";

/// Subdirectories created on the RAM disk.
const SUBDIRS: &[&str] = &["cache", "scrollback", "kv-cache", "models", "tmp"];

/// Configuration for RAM disk creation.
#[derive(Debug, Clone)]
pub struct RamDiskConfig {
    /// Size in megabytes.
    pub size_mb: u32,
    /// Preferred drive letter (will try fallbacks if taken).
    pub drive_letter: char,
    /// Filesystem type.
    pub filesystem: RamDiskFs,
    /// App-specific subdirectory name (e.g., "smux" or "zwg").
    pub app_name: String,
}

/// Filesystem for the RAM disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RamDiskFs {
    /// NTFS — supports large files, compression, security.
    Ntfs,
    /// exFAT — lightweight, no journaling overhead.
    ExFat,
}

/// A managed RAM disk that auto-unmounts on drop.
pub struct RamDisk {
    drive_letter: char,
    base_path: PathBuf,
    app_path: PathBuf,
}

impl Default for RamDiskConfig {
    fn default() -> Self {
        Self {
            size_mb: DEFAULT_SIZE_MB,
            drive_letter: DEFAULT_DRIVE_LETTER,
            filesystem: RamDiskFs::Ntfs,
            app_name: "zwg".to_string(),
        }
    }
}

impl RamDiskConfig {
    /// Create config for ZWG Terminal.
    pub fn zwg() -> Self {
        Self {
            app_name: "zwg".to_string(),
            ..Default::default()
        }
    }

    /// Create config for smux.
    pub fn smux() -> Self {
        Self {
            app_name: "smux".to_string(),
            ..Default::default()
        }
    }
}

impl RamDisk {
    /// Check if ImDisk is installed and available.
    pub fn is_available() -> bool {
        find_imdisk().is_some()
    }

    /// Create a RAM disk. Returns `Ok(None)` if ImDisk is not installed.
    /// Returns `Ok(Some(RamDisk))` on success.
    pub fn create(config: RamDiskConfig) -> Result<Option<Self>> {
        let imdisk = match find_imdisk() {
            Some(path) => path,
            None => {
                log::info!("ImDisk not found — RAM disk acceleration disabled");
                return Ok(None);
            }
        };

        // Clean up any stale ImDisk mounts that have no filesystem
        cleanup_stale_imdisk(&imdisk, config.drive_letter);

        // Find an available drive letter
        let letter = find_available_letter(config.drive_letter)?;

        let fs_type = match config.filesystem {
            RamDiskFs::Ntfs => "ntfs",
            RamDiskFs::ExFat => "exfat",
        };

        // Create + format via cmd.exe to avoid Rust's argument quoting
        // breaking imdisk's `-p` parameter (which passes args to format.com
        // with the necessary privileges — format.com alone requires elevation).
        let mount_arg = format!("{}:", letter);

        log::info!(
            "Creating {}MB RAM disk on {}: ({})",
            config.size_mb,
            mount_arg,
            fs_type.to_uppercase()
        );

        let cmd_line = format!(
            "{} -a -s {}M -m {}: -p \"/fs:{} /q /y\"",
            imdisk.display(),
            config.size_mb,
            letter,
            fs_type
        );
        log::debug!("imdisk command: {}", cmd_line);

        let output = Command::new("cmd.exe")
            .args(["/c", &cmd_line])
            .output()
            .map_err(|e| anyhow!("failed to run imdisk via cmd: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            log::error!("imdisk failed: stdout={}, stderr={}", stdout.trim(), stderr.trim());
            return Err(anyhow!("imdisk failed: {}", stderr.trim()));
        }

        // Wait for filesystem to be writable after imdisk creates + formats
        let base_path = PathBuf::from(format!("{}:\\", letter));
        if !wait_for_filesystem_ready(letter, 15, 300) {
            log::error!("RAM disk {}: created but filesystem never became writable — removing", letter);
            let _ = unmount_drive_force(&imdisk, letter);
            return Err(anyhow!(
                "RAM disk created on {}: but filesystem not writable. \
                 Try running 'imdisk -D -m {}:' manually and retry.",
                letter, letter
            ));
        }

        let app_path = base_path.join(&config.app_name);

        // Create subdirectories with retry (filesystem may need extra time)
        for sub in SUBDIRS {
            let dir = app_path.join(sub);
            if let Err(e) = create_dir_with_retry(&dir, 5, 200) {
                log::warn!("Failed to create {} after retries: {e}", dir.display());
            }
        }

        log::info!(
            "RAM disk ready: {} ({} MB, {})",
            app_path.display(),
            config.size_mb,
            SUBDIRS.join(", ")
        );

        Ok(Some(Self {
            drive_letter: letter,
            base_path,
            app_path,
        }))
    }

    /// Get the app-specific base path on the RAM disk.
    pub fn path(&self) -> &Path {
        &self.app_path
    }

    /// Get the raw drive root (e.g., `R:\`).
    pub fn drive_root(&self) -> &Path {
        &self.base_path
    }

    /// Get the drive letter.
    pub fn drive_letter(&self) -> char {
        self.drive_letter
    }

    /// Get path for cache files.
    pub fn cache_dir(&self) -> PathBuf {
        self.app_path.join("cache")
    }

    /// Get path for scrollback storage.
    pub fn scrollback_dir(&self) -> PathBuf {
        self.app_path.join("scrollback")
    }

    /// Get path for KV cache files.
    pub fn kv_cache_dir(&self) -> PathBuf {
        self.app_path.join("kv-cache")
    }

    /// Get path for model files.
    pub fn models_dir(&self) -> PathBuf {
        self.app_path.join("models")
    }

    /// Get path for temporary files.
    pub fn tmp_dir(&self) -> PathBuf {
        self.app_path.join("tmp")
    }

    /// Check how much space is available on the RAM disk.
    pub fn available_bytes(&self) -> u64 {
        fs_available_bytes(&self.base_path)
    }

    /// Check total size of the RAM disk.
    pub fn total_bytes(&self) -> u64 {
        fs_total_bytes(&self.base_path)
    }

    /// Unmount the RAM disk (also called automatically on drop).
    pub fn unmount(&self) -> Result<()> {
        unmount_drive(self.drive_letter)
    }
}

impl Drop for RamDisk {
    fn drop(&mut self) {
        if let Err(e) = self.unmount() {
            log::warn!("Failed to unmount RAM disk {}: {e}", self.drive_letter);
        }
    }
}

/// Check if a drive letter has a usable filesystem (can list root directory).
fn drive_has_filesystem(letter: char) -> bool {
    let root = format!("{}:\\", letter);
    std::fs::read_dir(&root).is_ok()
}

/// Wait for filesystem to become fully ready (not just mountable, but writable).
/// Returns true if ready within the timeout.
fn wait_for_filesystem_ready(letter: char, max_attempts: u32, interval_ms: u64) -> bool {
    let test_path = PathBuf::from(format!("{}:\\.fs_ready_test", letter));
    for attempt in 1..=max_attempts {
        // First check if we can list the directory
        if !drive_has_filesystem(letter) {
            log::debug!("Filesystem not ready on {}: (attempt {}/{})", letter, attempt, max_attempts);
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            continue;
        }
        // Then check if we can actually write a file
        match std::fs::write(&test_path, b"ok") {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_path);
                log::debug!("Filesystem ready on {}: after {} attempts", letter, attempt);
                return true;
            }
            Err(e) => {
                log::debug!("Filesystem write test failed on {}: {e} (attempt {}/{})", letter, attempt, max_attempts);
                std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            }
        }
    }
    false
}

/// Create a directory with retry logic for slow filesystem initialization.
fn create_dir_with_retry(path: &Path, max_attempts: u32, interval_ms: u64) -> std::io::Result<()> {
    let mut last_err = None;
    for _ in 0..max_attempts {
        match std::fs::create_dir_all(path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "create_dir_all failed")))
}

/// Clean up stale ImDisk mounts that exist but have no filesystem.
/// This handles the case where a previous process crashed and left
/// an unformatted RAM disk behind.
fn cleanup_stale_imdisk(imdisk: &Path, preferred: char) {
    let letters_to_check: Vec<char> = std::iter::once(preferred)
        .chain(FALLBACK_LETTERS.iter().copied())
        .collect();

    for letter in letters_to_check {
        if drive_exists(letter) && !drive_has_filesystem(letter) {
            log::warn!(
                "Found stale ImDisk mount on {}: with no filesystem — removing",
                letter
            );
            let _ = unmount_drive_force(imdisk, letter);
        }
    }
}

/// Force-unmount an ImDisk drive (tries -D first, then -R emergency removal).
fn unmount_drive_force(imdisk: &Path, letter: char) -> Result<()> {
    let mount_arg = format!("{}:", letter);

    // Try graceful removal first
    let output = Command::new(imdisk)
        .args(["-D", "-m", &mount_arg])
        .output();

    if let Ok(ref out) = output {
        if out.status.success() {
            log::info!("Removed stale RAM disk {}:", letter);
            std::thread::sleep(std::time::Duration::from_millis(300));
            return Ok(());
        }
    }

    // Try emergency removal by enumerating units
    for unit in 0..16u32 {
        let unit_str = unit.to_string();
        let probe = Command::new(imdisk)
            .args(["-l", "-u", &unit_str])
            .output();

        if let Ok(ref out) = probe {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if stdout.contains(&mount_arg) || stdout.contains(&format!("{}:\\", letter)) {
                log::warn!("Emergency removing ImDisk unit {} ({}:)", unit, letter);
                let _ = Command::new(imdisk)
                    .args(["-R", "-u", &unit_str])
                    .output();
                std::thread::sleep(std::time::Duration::from_millis(300));
                return Ok(());
            }
        }
    }

    Err(anyhow!("could not remove stale ImDisk mount {}:", letter))
}

/// Find the ImDisk executable.
fn find_imdisk() -> Option<PathBuf> {
    // Check PATH first
    if Command::new(IMDISK_EXE)
        .arg("--version")
        .output()
        .is_ok()
    {
        return Some(PathBuf::from(IMDISK_EXE));
    }

    // Check common locations
    let candidates = [
        r"C:\Windows\System32\imdisk.exe",
        r"C:\Program Files\ImDisk\imdisk.exe",
        r"C:\Program Files (x86)\ImDisk\imdisk.exe",
    ];

    for path in &candidates {
        let p = Path::new(path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    None
}

/// Find an available drive letter.
fn find_available_letter(preferred: char) -> Result<char> {
    if !drive_exists(preferred) {
        return Ok(preferred);
    }

    for &letter in FALLBACK_LETTERS {
        if !drive_exists(letter) {
            return Ok(letter);
        }
    }

    Err(anyhow!("no available drive letter found"))
}

/// Check if a drive letter is already in use.
fn drive_exists(letter: char) -> bool {
    let path = format!("{}:\\", letter);
    Path::new(&path).exists()
}

/// Unmount an ImDisk drive.
fn unmount_drive(letter: char) -> Result<()> {
    let imdisk = find_imdisk()
        .ok_or_else(|| anyhow!("imdisk not found for unmount"))?;

    let mount_arg = format!("{}:", letter);
    log::info!("Unmounting RAM disk {}:", letter);

    let output = Command::new(&imdisk)
        .args(["-D", "-m", &mount_arg])
        .output()
        .map_err(|e| anyhow!("failed to run imdisk -D: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("imdisk unmount failed: {stderr}"));
    }

    Ok(())
}

/// Get available bytes on a filesystem path.
fn fs_available_bytes(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free_bytes: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        unsafe {
            windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                windows::core::PCWSTR(wide.as_ptr()),
                Some(&mut free_bytes as *mut u64 as *mut _),
                Some(&mut total_bytes as *mut u64 as *mut _),
                Some(&mut total_free as *mut u64 as *mut _),
            ).ok();
        }
        free_bytes
    }
    #[cfg(not(windows))]
    { 0 }
}

/// Get total bytes on a filesystem path.
fn fs_total_bytes(path: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free_bytes: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_free: u64 = 0;
        unsafe {
            windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                windows::core::PCWSTR(wide.as_ptr()),
                Some(&mut free_bytes as *mut u64 as *mut _),
                Some(&mut total_bytes as *mut u64 as *mut _),
                Some(&mut total_free as *mut u64 as *mut _),
            ).ok();
        }
        total_bytes
    }
    #[cfg(not(windows))]
    { 0 }
}

impl std::fmt::Display for RamDisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total = self.total_bytes() / (1024 * 1024);
        let avail = self.available_bytes() / (1024 * 1024);
        write!(
            f,
            "RAM Disk {}:\\ ({} MB total, {} MB free) → {}",
            self.drive_letter,
            total,
            avail,
            self.app_path.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = RamDiskConfig::default();
        assert_eq!(cfg.size_mb, 512);
        assert_eq!(cfg.drive_letter, 'R');
        assert_eq!(cfg.app_name, "zwg");
    }

    #[test]
    fn zwg_config() {
        let cfg = RamDiskConfig::zwg();
        assert_eq!(cfg.app_name, "zwg");
    }

    #[test]
    fn drive_exists_c() {
        assert!(drive_exists('C'));
    }

    #[test]
    fn drive_not_exists_z() {
        if !drive_exists('Z') {
            assert!(!drive_exists('Z'));
        }
    }

    #[test]
    fn find_available_letter_works() {
        let letter = find_available_letter('C');
        assert!(letter.is_ok());
        let l = letter.unwrap();
        assert_ne!(l, 'C');
    }

    #[test]
    fn is_available_returns_bool() {
        let _ = RamDisk::is_available();
    }
}
