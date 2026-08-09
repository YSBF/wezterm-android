//! The executable prefix.
//!
//! Since API 29 an app may not execute a binary out of its own writable data
//! directory (W^X), and SELinux constrains most of what is left. The one
//! directory an app may both read and execute from is the APK's native library
//! directory, so any shell we ship has to live there.
//!
//! The installer only extracts files matching `lib*.so`, and only sets the
//! execute bit on those, so a bundled `bash` is shipped as `libbash.so`. That
//! keeps it runnable but leaves it under a name no script or `PATH` lookup
//! would ever find.
//!
//! The fix, which is what Termux does on modern Android, is a prefix of
//! symlinks: `$HOME/.local/bin/bash` points at `<nativeLibDir>/libbash.so`.
//! `execve` resolves the symlink and applies the W^X and SELinux checks to the
//! *target*, which lives in the executable directory, so this needs no root.
//!
//! Multi-call binaries get one link per applet, discovered by asking them.
//!
//! None of this is fatal if it fails. With no bundled binaries at all the
//! prefix is simply empty and `PATH` falls through to `/system/bin`, where
//! toybox provides a small but real set of utilities.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Libraries in the native library directory that are not shells or utilities.
const NOT_EXECUTABLES: &[&str] = &["libwezterm_gui.so", "libc++_shared.so", "libmain.so"];

/// Multi-call binaries we know how to enumerate, and the argument that makes
/// them list their applets.
const MULTICALL: &[(&str, &str)] = &[("busybox", "--list"), ("toybox", "--long")];

/// Shells to prefer, best first. bash is the expected default for anyone who
/// wants a real terminal; the rest are fallbacks in decreasing order of
/// capability.
const SHELL_PREFERENCE: &[&str] = &["bash", "zsh", "fish", "ash", "mksh", "sh"];

#[derive(Debug, Clone)]
pub struct Prefix {
    /// Where the symlinks live, and the first entry on `PATH`.
    pub bin: PathBuf,
    /// The names that resolved to something executable.
    pub commands: BTreeSet<String>,
}

impl Prefix {
    /// The best available shell, as an absolute path.
    ///
    /// Falls back to `/system/bin/sh`, which exists on every Android device;
    /// note that `/bin/sh`, which `portable-pty` would otherwise reach for,
    /// does not.
    pub fn shell(&self) -> PathBuf {
        for name in SHELL_PREFERENCE {
            if self.commands.contains(*name) {
                return self.bin.join(name);
            }
        }
        PathBuf::from("/system/bin/sh")
    }
}

/// Build the prefix from whatever was bundled into `native_lib`.
pub fn populate(home: &Path, native_lib: Option<&Path>) -> Prefix {
    let bin = home.join(".local").join("bin");
    let mut commands = BTreeSet::new();

    if let Err(err) = std::fs::create_dir_all(&bin) {
        log::warn!("could not create {}: {err:#}", bin.display());
        return Prefix { bin, commands };
    }

    let native_lib = match native_lib {
        Some(dir) => dir,
        None => {
            log::info!("no native library directory; the prefix will be empty");
            return Prefix { bin, commands };
        }
    };

    for (name, target) in bundled_executables(native_lib) {
        if link(&bin.join(&name), &target) {
            commands.insert(name);
        }
    }

    // Multi-call binaries are a single executable that behaves as dozens of
    // utilities depending on argv[0], so each applet needs its own link.
    for (name, list_arg) in MULTICALL {
        if !commands.contains(*name) {
            continue;
        }
        let applets = match list_applets(&bin.join(name), list_arg) {
            Ok(applets) => applets,
            Err(err) => {
                log::warn!("could not enumerate {name} applets: {err:#}");
                continue;
            }
        };
        let target = native_lib.join(format!("lib{name}.so"));
        for applet in applets {
            if commands.contains(&applet) {
                // A dedicated binary beats a busybox applet.
                continue;
            }
            if link(&bin.join(&applet), &target) {
                commands.insert(applet);
            }
        }
    }

    log::info!(
        "prefix {} has {} commands",
        bin.display(),
        commands.len()
    );
    Prefix { bin, commands }
}

/// `(command name, absolute path)` for every `lib*.so` that looks like a
/// bundled executable rather than a library we linked against.
fn bundled_executables(native_lib: &Path) -> Vec<(String, PathBuf)> {
    let read_dir = match std::fs::read_dir(native_lib) {
        Ok(read_dir) => read_dir,
        Err(err) => {
            log::warn!("could not read {}: {err:#}", native_lib.display());
            return vec![];
        }
    };

    let mut found = vec![];
    for entry in read_dir.filter_map(Result::ok) {
        let path = entry.path();
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => continue,
        };

        if NOT_EXECUTABLES.contains(&file_name) {
            continue;
        }

        let name = match file_name
            .strip_prefix("lib")
            .and_then(|rest| rest.strip_suffix(".so"))
        {
            Some(name) if !name.is_empty() => name,
            _ => continue,
        };

        found.push((name.to_string(), path));
    }
    found
}

/// Point `link_path` at `target`, replacing whatever was there before.
///
/// Replacing unconditionally matters across app upgrades: the native library
/// directory's path contains a version-specific component on some Android
/// versions, so a link left over from the previous install would dangle.
fn link(link_path: &Path, target: &Path) -> bool {
    use std::os::unix::fs::symlink;

    match std::fs::read_link(link_path) {
        Ok(existing) if existing == target => return true,
        Ok(_) | Err(_) => {
            // Either it points somewhere stale or it is not a symlink at all.
            let _ = std::fs::remove_file(link_path);
        }
    }

    match symlink(target, link_path) {
        Ok(()) => true,
        Err(err) => {
            log::warn!(
                "could not link {} -> {}: {err:#}",
                link_path.display(),
                target.display()
            );
            false
        }
    }
}

/// Ask a multi-call binary which applets it provides.
fn list_applets(program: &Path, list_arg: &str) -> anyhow::Result<Vec<String>> {
    use std::process::Command;

    let output = Command::new(program).arg(list_arg).output()?;
    if !output.status.success() {
        anyhow::bail!("{} {list_arg} exited with {}", program.display(), output.status);
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter(|name| is_plausible_applet(name))
        .map(str::to_string)
        .collect())
}

/// Guard against a multi-call binary printing a banner or a usage line rather
/// than a bare list; only accept things that could be a filename.
fn is_plausible_applet(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '['))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn accepts_real_applet_names() {
        for name in ["ls", "sha256sum", "[", "[[", "run-parts", "unlzma", "."] {
            assert!(is_plausible_applet(name), "{name}");
        }
    }

    #[test]
    fn rejects_banner_noise() {
        assert!(!is_plausible_applet(""));
        assert!(!is_plausible_applet(&"x".repeat(33)));
        assert!(!is_plausible_applet("usage:"));
        assert!(!is_plausible_applet("/bin/ls"));
        assert!(!is_plausible_applet("(c)"));
        // A word like "multi-call" from a banner does get through; the guard
        // only has to keep obvious junk from becoming a symlink, and a
        // spurious link to a real binary is harmless.
        assert!(is_plausible_applet("multi-call"));
    }

    #[test]
    fn shell_preference_falls_back_to_system() {
        let prefix = Prefix {
            bin: PathBuf::from("/data/data/org.wezfurlong.wezterm/files/.local/bin"),
            commands: BTreeSet::new(),
        };
        assert_eq!(prefix.shell(), PathBuf::from("/system/bin/sh"));
    }

    #[test]
    fn shell_preference_prefers_bash() {
        let mut commands = BTreeSet::new();
        commands.insert("sh".to_string());
        commands.insert("bash".to_string());
        let prefix = Prefix {
            bin: PathBuf::from("/prefix/bin"),
            commands,
        };
        assert_eq!(prefix.shell(), PathBuf::from("/prefix/bin/bash"));
    }
}
