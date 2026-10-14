//! Automatic updates (v1.3), the blocking half: fetching the release feed,
//! downloading, verifying, extracting and swapping the bundle, and the
//! relaunch helper. Runs on a worker thread in the app and directly in the
//! `check-update` and `apply-update` commands. Every tool is called by its
//! absolute path with an argument vector; nothing goes through a shell
//! except the relaunch helper, whose script is a constant.

use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::update::{
    BUNDLE_NAME, CURL, CheckOutcome, FEED_ENV, FeedSource, MAX_SIG_BYTES, MAX_ZIP_BYTES,
    RELAUNCH_SHELL, UPDATE_PUBLIC_KEY, Version, check_bundle_info, check_extracted, check_newer,
    curl_download_args, curl_feed_args, evaluate, feed_source, is_staging_name, parse_release,
    previous_path, relaunch_args, staging_template, swap, verify_zip,
};

const DITTO: &str = "/usr/bin/ditto";
const CODESIGN: &str = "/usr/bin/codesign";
const PLUTIL: &str = "/usr/bin/plutil";

/// Debug builds only: a base64 public key that replaces the compiled-in
/// one, for the end-to-end tests on scratch copies.
#[cfg(debug_assertions)]
pub const TEST_KEY_ENV: &str = "ICE_RUST_UPDATE_TEST_KEY";

/// The key updates must be signed with. Release builds trust only the
/// compiled-in key.
pub fn trusted_key() -> [u8; 32] {
    #[cfg(debug_assertions)]
    if let Some(key) = std::env::var(TEST_KEY_ENV)
        .ok()
        .and_then(|text| crate::update::parse_key(&text))
    {
        return key;
    }
    UPDATE_PUBLIC_KEY
}

/// Where the feed comes from: `ICE_RUST_UPDATE_FEED` if set, else GitHub.
/// Release builds accept only `https://` overrides.
pub fn current_feed() -> Result<FeedSource, String> {
    let override_url = std::env::var(FEED_ENV).ok();
    feed_source(override_url.as_deref(), cfg!(debug_assertions))
}

fn run(program: &str, args: &[impl AsRef<OsStr>]) -> Result<Vec<u8>, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {program} ({e})"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        Err(if detail.is_empty() {
            format!("{program} failed ({})", output.status)
        } else {
            format!("{program} failed: {detail}")
        })
    }
}

/// Reads the release feed.
pub fn fetch_feed(source: &FeedSource) -> Result<String, String> {
    match source {
        FeedSource::Https(url) => {
            let body = run(CURL, &curl_feed_args(url))?;
            String::from_utf8(body).map_err(|_| "the release feed is not UTF-8".to_string())
        }
        FeedSource::File(path) => {
            fs::read_to_string(path).map_err(|e| format!("could not read {} ({e})", path.display()))
        }
    }
}

/// One check: reads the feed and decides what it means for `current`.
pub fn check(current: Version) -> Result<CheckOutcome, String> {
    let source = current_feed()?;
    let json = fetch_feed(&source)?;
    let release = parse_release(&json).map_err(|e| e.to_string())?;
    Ok(evaluate(current, &release))
}

/// Whether the current user can write `path` (`access(2)` with `W_OK`).
pub fn writable(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `c_path` is a valid NUL-terminated string for the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// A private directory made with `mkdtemp(3)` from `template`.
fn make_temp_dir(template: &Path) -> io::Result<PathBuf> {
    let c_template = CString::new(template.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("the staging path contains a NUL byte"))?;
    let mut bytes = c_template.into_bytes_with_nul();
    // SAFETY: `bytes` is a mutable NUL-terminated buffer that ends in
    // "XXXXXX", as mkdtemp requires; it rewrites those bytes in place.
    let result = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
    if result.is_null() {
        return Err(io::Error::last_os_error());
    }
    bytes.pop();
    Ok(PathBuf::from(OsStr::from_bytes(&bytes)))
}

/// Removes a staging directory when dropped, on every path.
struct Staging(PathBuf);

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Downloads `url` into `out` with curl's size and time limits.
pub fn download(url: &str, out: &Path) -> Result<(), String> {
    run(CURL, &curl_download_args(url, out)).map(|_| ())
}

fn read_limited(path: &Path, limit: u64, what: &str) -> Result<Vec<u8>, String> {
    let size = fs::metadata(path)
        .map_err(|e| format!("could not read the {what} ({e})"))?
        .len();
    if size > limit {
        return Err(format!("the {what} is larger than {limit} bytes"));
    }
    fs::read(path).map_err(|e| format!("could not read the {what} ({e})"))
}

/// A key of a bundle's Info.plist, through `plutil -extract`.
fn plist_value(bundle: &Path, key: &str) -> Option<String> {
    let plist = bundle.join("Contents/Info.plist");
    let args: [&OsStr; 6] = [
        OsStr::new("-extract"),
        OsStr::new(key),
        OsStr::new("raw"),
        OsStr::new("-o"),
        OsStr::new("-"),
        plist.as_os_str(),
    ];
    let out = run(PLUTIL, &args).ok()?;
    let text = String::from_utf8(out).ok()?;
    Some(text.trim_end_matches('\n').to_string())
}

/// The installed bundle's `CFBundleShortVersionString`.
pub fn bundle_version(bundle: &Path) -> Option<String> {
    plist_value(bundle, "CFBundleShortVersionString")
}

/// Installs the update in `zip` (signed by `sig_text`) into `bundle`, which
/// must hold an older version. Verifies the signature over the zip bytes
/// before anything reads the zip, extracts those same bytes, checks the
/// new bundle, then swaps with rollback. Leaves the old bundle at
/// `<name>.app.previous`. Does not relaunch.
pub fn install(
    bundle: &Path,
    zip: &Path,
    sig_text: &str,
    version: Version,
    key: &[u8; 32],
) -> Result<(), String> {
    let parent = bundle
        .parent()
        .ok_or_else(|| "the app has no parent folder".to_string())?;
    let zip_bytes = read_limited(zip, MAX_ZIP_BYTES, "update zip")?;
    verify_zip(key, &zip_bytes, sig_text).map_err(|e| e.to_string())?;
    check_newer(bundle_version(bundle).as_deref(), version)?;

    // Staging sits next to the bundle so the final move is a rename on the
    // same volume. The verified bytes are written there and extracted from
    // there, so nothing can swap the zip between check and use.
    let staging = Staging(
        make_temp_dir(&staging_template(parent))
            .map_err(|e| format!("could not create a staging folder ({e})"))?,
    );
    let staged_zip = staging.0.join("update.zip");
    fs::write(&staged_zip, &zip_bytes).map_err(|e| format!("could not write the update ({e})"))?;
    let extract = staging.0.join("extract");
    let ditto_args: [&OsStr; 4] = [
        OsStr::new("-x"),
        OsStr::new("-k"),
        staged_zip.as_os_str(),
        extract.as_os_str(),
    ];
    run(DITTO, &ditto_args)?;

    let mut names: Vec<String> = fs::read_dir(&extract)
        .map_err(|e| format!("could not read the extracted update ({e})"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    check_extracted(&names)?;
    let new_bundle = extract.join(BUNDLE_NAME);
    let meta = fs::symlink_metadata(&new_bundle)
        .map_err(|e| format!("could not read the new bundle ({e})"))?;
    if !meta.is_dir() {
        return Err(format!("{BUNDLE_NAME} in the update is not a folder"));
    }
    check_bundle_info(
        plist_value(&new_bundle, "CFBundleIdentifier").as_deref(),
        bundle_version(&new_bundle).as_deref(),
        version,
    )?;
    let codesign_args: [&OsStr; 3] = [
        OsStr::new("--verify"),
        OsStr::new("--strict"),
        new_bundle.as_os_str(),
    ];
    run(CODESIGN, &codesign_args)
        .map_err(|e| format!("the new bundle's signature is invalid: {e}"))?;

    let previous =
        previous_path(bundle).ok_or_else(|| "the app path has no file name".to_string())?;
    remove_previous(&previous)?;
    swap(bundle, &new_bundle, &previous, |from, to| {
        fs::rename(from, to)
    })
    .map_err(|e| e.to_string())
}

/// Removes a leftover `<name>.app.previous` folder (never a symlink target).
fn remove_previous(previous: &Path) -> Result<(), String> {
    match fs::symlink_metadata(previous) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(previous)
            .map_err(|e| format!("could not remove {} ({e})", previous.display())),
        Ok(_) => fs::remove_file(previous)
            .map_err(|e| format!("could not remove {} ({e})", previous.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not read {} ({e})", previous.display())),
    }
}

/// Downloads the offered zip and signature into a private temporary folder
/// and installs them into `bundle`.
pub fn download_and_install(
    bundle: &Path,
    zip_url: &str,
    sig_url: &str,
    version: Version,
    key: &[u8; 32],
) -> Result<(), String> {
    let downloads = Staging(
        make_temp_dir(&std::env::temp_dir().join("ice-rust-update.XXXXXX"))
            .map_err(|e| format!("could not create a download folder ({e})"))?,
    );
    let zip = downloads.0.join("update.zip");
    let sig = downloads.0.join("update.zip.sig");
    download(zip_url, &zip)?;
    download(sig_url, &sig)?;
    let sig_bytes = read_limited(&sig, MAX_SIG_BYTES, "signature")?;
    let sig_text =
        String::from_utf8(sig_bytes).map_err(|_| "the signature is not text".to_string())?;
    install(bundle, &zip, &sig_text, version, key)
}

/// At launch: removes the previous bundle and stale staging folders left
/// next to `bundle` by an update.
pub fn clean_up_after_update(bundle: &Path) {
    if let Some(previous) = previous_path(bundle)
        && let Err(err) = remove_previous(&previous)
    {
        eprintln!("ice-rust: {err}");
    }
    let Some(parent) = bundle.parent() else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if is_dir && is_staging_name(&name.to_string_lossy()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Starts the helper that reopens `bundle` once this process has exited.
pub fn spawn_relaunch_helper(bundle: &Path) -> Result<(), String> {
    Command::new(RELAUNCH_SHELL)
        .args(relaunch_args(std::process::id(), bundle))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start the relaunch helper ({e})"))
}
