//! Command-line tools for updates (v1.3): `check-update`, `apply-update`,
//! `update-keygen`, `update-sign` and `update-verify`.

use std::path::PathBuf;

#[derive(Debug, PartialEq)]
pub enum UpdateCommand {
    Check {
        json: bool,
    },
    Apply {
        app: PathBuf,
        zip: PathBuf,
        sig: PathBuf,
        version: Option<String>,
    },
    Keygen {
        out: PathBuf,
    },
    Sign {
        key: PathBuf,
        zip: PathBuf,
    },
    Verify {
        zip: PathBuf,
    },
}

/// Parses the arguments after one of the update command names, or `None`
/// when `name` is not one of them.
pub fn parse(name: &str, rest: &[String]) -> Option<Result<UpdateCommand, String>> {
    let parsed = match name {
        "check-update" => parse_check(rest),
        "apply-update" => parse_apply(rest),
        "update-keygen" => parse_keygen(rest),
        "update-sign" => parse_sign(rest),
        "update-verify" => match rest {
            [zip] if !zip.is_empty() => Ok(UpdateCommand::Verify {
                zip: PathBuf::from(zip),
            }),
            _ => Err("update-verify needs <zip>".into()),
        },
        _ => return None,
    };
    Some(parsed)
}

fn value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    match it.next() {
        Some(v) if !v.is_empty() => Ok(v.clone()),
        _ => Err(format!("{flag} needs a value")),
    }
}

fn parse_check(rest: &[String]) -> Result<UpdateCommand, String> {
    let mut json = false;
    for arg in rest {
        match arg.as_str() {
            "--json" => json = true,
            other => return Err(format!("unknown check-update argument: {other}")),
        }
    }
    Ok(UpdateCommand::Check { json })
}

fn parse_apply(rest: &[String]) -> Result<UpdateCommand, String> {
    let (mut app, mut zip, mut sig, mut version) = (None, None, None, None);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--app" => app = Some(PathBuf::from(value(&mut it, "--app")?)),
            "--zip" => zip = Some(PathBuf::from(value(&mut it, "--zip")?)),
            "--sig" => sig = Some(PathBuf::from(value(&mut it, "--sig")?)),
            "--version" => version = Some(value(&mut it, "--version")?),
            other => return Err(format!("unknown apply-update argument: {other}")),
        }
    }
    Ok(UpdateCommand::Apply {
        app: app.ok_or("apply-update needs --app")?,
        zip: zip.ok_or("apply-update needs --zip")?,
        sig: sig.ok_or("apply-update needs --sig")?,
        version,
    })
}

fn parse_keygen(rest: &[String]) -> Result<UpdateCommand, String> {
    match rest {
        [flag, out] if flag == "--out" && !out.is_empty() => Ok(UpdateCommand::Keygen {
            out: PathBuf::from(out),
        }),
        _ => Err("update-keygen needs --out <path>".into()),
    }
}

fn parse_sign(rest: &[String]) -> Result<UpdateCommand, String> {
    match rest {
        [flag, key, zip] if flag == "--key" && !key.is_empty() && !zip.is_empty() => {
            Ok(UpdateCommand::Sign {
                key: PathBuf::from(key),
                zip: PathBuf::from(zip),
            })
        }
        _ => Err("update-sign needs --key <path> <zip>".into()),
    }
}

#[cfg(target_os = "macos")]
pub mod run {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::process::ExitCode;

    use ice_rust::platform::installer;
    use ice_rust::update::{
        CheckOutcome, Version, encode_key, is_placeholder_key, parse_key, precheck, public_key_of,
        sign_zip, verify_zip, version_from_zip_name,
    };

    use super::UpdateCommand;

    fn fail(msg: impl std::fmt::Display) -> ExitCode {
        eprintln!("ice-rust: {msg}");
        ExitCode::FAILURE
    }

    fn json_text(s: &str) -> String {
        serde_json::Value::String(s.to_string()).to_string()
    }

    pub fn run(command: &UpdateCommand) -> ExitCode {
        match command {
            UpdateCommand::Check { json } => check(*json),
            UpdateCommand::Apply {
                app,
                zip,
                sig,
                version,
            } => apply(app, zip, sig, version.as_deref()),
            UpdateCommand::Keygen { out } => keygen(out),
            UpdateCommand::Sign { key, zip } => sign(key, zip),
            UpdateCommand::Verify { zip } => verify(zip),
        }
    }

    fn check(json: bool) -> ExitCode {
        let current = Version::current();
        let outcome = match installer::check(current) {
            Ok(outcome) => outcome,
            Err(err) => return fail(format!("could not check for updates: {err}")),
        };
        let (state, latest, detail) = match &outcome {
            CheckOutcome::UpToDate { latest } => {
                ("up-to-date", latest.map(|v| v.to_string()), String::new())
            }
            CheckOutcome::Available(update) => (
                "available",
                Some(update.version.to_string()),
                format!("{} {}", update.zip_url, update.sig_url),
            ),
            CheckOutcome::NotInstallable { version, reason } => {
                ("not-installable", Some(version.to_string()), reason.clone())
            }
        };
        if json {
            println!(
                "{{\"current\":{},\"state\":{},\"latest\":{},\"detail\":{}}}",
                json_text(&current.to_string()),
                json_text(state),
                latest.as_deref().map_or("null".to_string(), json_text),
                json_text(&detail),
            );
        } else {
            println!("current  {current}");
            println!("latest   {}", latest.as_deref().unwrap_or("unknown"));
            println!("state    {state}");
            if !detail.is_empty() {
                println!("detail   {detail}");
            }
        }
        ExitCode::SUCCESS
    }

    fn apply(app: &Path, zip: &Path, sig: &Path, version: Option<&str>) -> ExitCode {
        let from_name = zip
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(version_from_zip_name);
        let Some(version) = version.and_then(Version::parse).or(from_name) else {
            return fail(
                "cannot tell the update's version; name the zip IceRust-<version>-arm64.zip or pass --version",
            );
        };
        let key = installer::trusted_key();
        let bundle = match precheck(Some(app), &key, version, installer::writable) {
            Ok(bundle) => bundle,
            Err(err) => return fail(err),
        };
        let sig_text = match fs::read_to_string(sig) {
            Ok(text) => text,
            Err(err) => return fail(format!("could not read {} ({err})", sig.display())),
        };
        match installer::install(&bundle, zip, &sig_text, version, &key) {
            Ok(()) => {
                println!("installed ice-rust {version} into {}", bundle.display());
                ExitCode::SUCCESS
            }
            Err(err) => fail(format!("the update failed: {err}")),
        }
    }

    fn keygen(out: &Path) -> ExitCode {
        let mut secret = [0u8; 32];
        // SAFETY: getentropy fills at most 256 bytes into the given buffer;
        // `secret` is 32 bytes long and writable.
        if unsafe { libc::getentropy(secret.as_mut_ptr().cast(), secret.len()) } != 0 {
            return fail("could not read random bytes for the key");
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(out);
        let mut file = match file {
            Ok(file) => file,
            Err(err) => return fail(format!("could not create {} ({err})", out.display())),
        };
        if let Err(err) = writeln!(file, "{}", encode_key(&secret)) {
            return fail(format!("could not write {} ({err})", out.display()));
        }
        println!("{}", encode_key(&public_key_of(&secret)));
        ExitCode::SUCCESS
    }

    /// Verifies `<zip>.sig` with the key built into this binary, as an
    /// installed app would.
    fn verify(zip: &Path) -> ExitCode {
        let mut sig_path = zip.as_os_str().to_os_string();
        sig_path.push(".sig");
        let (bytes, sig) = match (fs::read(zip), fs::read_to_string(&sig_path)) {
            (Ok(bytes), Ok(sig)) => (bytes, sig),
            (Err(err), _) => return fail(format!("could not read {} ({err})", zip.display())),
            (_, Err(err)) => return fail(format!("could not read the .sig file ({err})")),
        };
        match verify_zip(&installer::trusted_key(), &bytes, &sig) {
            Ok(()) => {
                println!("signature ok for {}", zip.display());
                ExitCode::SUCCESS
            }
            Err(err) => fail(err),
        }
    }

    fn sign(key_path: &Path, zip: &Path) -> ExitCode {
        let secret = match fs::read_to_string(key_path)
            .ok()
            .and_then(|t| parse_key(&t))
        {
            Some(secret) => secret,
            None => return fail(format!("{} is not a key file", key_path.display())),
        };
        let public = public_key_of(&secret);
        if is_placeholder_key(&public) {
            return fail("refusing a key whose public half is the placeholder");
        }
        let bytes = match fs::read(zip) {
            Ok(bytes) => bytes,
            Err(err) => return fail(format!("could not read {} ({err})", zip.display())),
        };
        let signature = sign_zip(&secret, &bytes);
        if let Err(err) = verify_zip(&public, &bytes, &signature) {
            return fail(format!("the new signature does not verify: {err}"));
        }
        let mut sig_path = zip.as_os_str().to_os_string();
        sig_path.push(".sig");
        if let Err(err) = fs::write(&sig_path, signature) {
            return fail(format!("could not write the signature ({err})"));
        }
        println!("wrote {}", Path::new(&sig_path).display());
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn other_names_are_not_update_commands() {
        assert!(parse("probe", &[]).is_none());
    }

    #[test]
    fn check_update_takes_json() {
        assert_eq!(
            parse("check-update", &args(&["--json"])),
            Some(Ok(UpdateCommand::Check { json: true }))
        );
        assert!(parse("check-update", &args(&["--x"])).unwrap().is_err());
    }

    #[test]
    fn apply_update_needs_app_zip_and_sig() {
        let ok = parse(
            "apply-update",
            &args(&[
                "--app",
                "/a/IceRust.app",
                "--zip",
                "z.zip",
                "--sig",
                "z.zip.sig",
            ]),
        );
        assert_eq!(
            ok,
            Some(Ok(UpdateCommand::Apply {
                app: "/a/IceRust.app".into(),
                zip: "z.zip".into(),
                sig: "z.zip.sig".into(),
                version: None,
            }))
        );
        assert!(
            parse("apply-update", &args(&["--app", "/a"]))
                .unwrap()
                .is_err()
        );
        assert!(parse("apply-update", &args(&["--zip"])).unwrap().is_err());
    }

    #[test]
    fn keygen_and_sign_shapes() {
        assert_eq!(
            parse("update-keygen", &args(&["--out", "k"])),
            Some(Ok(UpdateCommand::Keygen { out: "k".into() }))
        );
        assert!(parse("update-keygen", &args(&[])).unwrap().is_err());
        assert_eq!(
            parse("update-sign", &args(&["--key", "k", "a.zip"])),
            Some(Ok(UpdateCommand::Sign {
                key: "k".into(),
                zip: "a.zip".into(),
            }))
        );
        assert!(parse("update-sign", &args(&["a.zip"])).unwrap().is_err());
        assert_eq!(
            parse("update-verify", &args(&["a.zip"])),
            Some(Ok(UpdateCommand::Verify {
                zip: "a.zip".into()
            }))
        );
        assert!(parse("update-verify", &args(&[])).unwrap().is_err());
    }
}
