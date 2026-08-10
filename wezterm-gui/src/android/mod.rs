//! The Android entry point.
//!
//! An Android app process is not a login session. It has no `HOME`, no
//! `TMPDIR`, and a `PATH` that points at system directories an app may not
//! execute from. wezterm assumes all three long before it reaches any GUI
//! code: `config/src/lib.rs` resolves `HOME_DIR` with
//! `dirs_next::home_dir().expect(...)` in a `lazy_static`, and
//! `compute_cache_dir`/`compute_data_dir`/`compute_runtime_dir` all fall back
//! to it.
//!
//! Because those are `lazy_static`s, setting the environment before anything
//! first touches `config` is sufficient and needs no change to `config`
//! itself. That makes the bootstrap an entry-point concern, which is what this
//! module is. Nothing in here may reference `config` before `bootstrap_env`
//! has run.

pub mod prefix;

use android_activity::AndroidApp;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The Activity calls into this once, on a dedicated native thread.
pub fn android_main(app: AndroidApp) {
    init_logging();

    // Must happen before the first touch of `config`.
    let dirs = match bootstrap_env(&app) {
        Ok(dirs) => dirs,
        Err(err) => {
            log::error!("failed to bootstrap the environment: {err:#}");
            return;
        }
    };
    log::info!("environment bootstrapped: {dirs:#?}");

    // Hand the app object to the window crate before anything constructs a
    // Connection.
    window::os::android::set_android_app(app);

    config::designate_this_as_the_main_thread();
    config::assign_error_callback(mux::connui::show_configuration_error_message);

    if let Err(err) = run(&dirs) {
        // Deliberately not terminate_with_error: that calls
        // std::process::exit, which on Android subverts the Activity
        // lifecycle and can take down other components sharing the process.
        log::error!("wezterm exited with an error: {err:#}");
    }

    mux::Mux::shutdown();
    crate::frontend::shutdown();
}

fn run(dirs: &AndroidDirs) -> anyhow::Result<()> {
    env_bootstrap::bootstrap();
    config::lua::add_context_setup_func(window_funcs::register);
    config::lua::add_context_setup_func(crate::scripting::register);
    config::lua::add_context_setup_func(crate::stats::register);

    crate::stats::Stats::init()?;

    // There is no argv on Android, so the desktop clap parsing is skipped
    // entirely and the equivalent of a bare `wezterm start` is run.
    config::common_init(None, &[], false)?;

    presize_initial_grid();

    let config = config::configuration();
    if let Some(value) = &config.default_ssh_auth_sock {
        std::env::set_var("SSH_AUTH_SOCK", value);
    }

    log::info!(
        "wezterm {} starting on android, config from {}",
        config::wezterm_version(),
        dirs.config.display()
    );

    let opts = start_command(&config);
    let default_domain = opts.domain.clone();
    let res = crate::run_terminal_gui(opts, default_domain);
    wezterm_blob_leases::clear_storage();
    res
}

/// Build the equivalent of the command line the desktop binary would have
/// parsed.
///
/// `always_new_process` is unconditional. On the desktop, a second `wezterm`
/// invocation looks for an already-running GUI over a unix socket and hands
/// the request to it. On Android there is only ever one process, and the
/// socket lives in the app's private runtime directory where nothing else can
/// reach it, so that discovery could only ever find itself.
///
/// When `default_domain` names a remote domain, this reproduces what
/// `wezterm connect <name>` does on the desktop. That matters because
/// `attach` is what makes the mux client adopt the panes that already exist on
/// the server, rather than opening an empty window beside them -- which is the
/// whole point of running a mux client on a phone.
fn start_command(config: &config::ConfigHandle) -> crate::StartCommand {
    let domain = config
        .default_domain
        .as_deref()
        .filter(|name| is_remote_domain(config, name))
        .map(|name| name.to_string());

    crate::StartCommand {
        always_new_process: true,
        attach: domain.is_some(),
        domain,
        ..Default::default()
    }
}

/// Spawn the first pane at the width the window is actually going to have.
///
/// The mux creates the first tab -- and with it the pty and the shell -- from
/// `initial_cols`/`initial_rows`, before any window exists. On the desktop the
/// window is then opened at a matching pixel size, so the pty it hands over is
/// already the right shape and is never resized. Android gives the activity
/// whatever size it likes and ignores what was asked for, so the first thing
/// that happens to a brand new shell is a resize.
///
/// That resize costs the user the first prompt. mksh, which is the shell on
/// every Android device, redraws its input line when the terminal's width
/// changes, and a prompt that has been printed but not yet typed into is
/// erased and not put back. The screen a launch opens on is then blank until
/// something is typed, which reads as a hung terminal.
///
/// Only the width is worth correcting. The redraw is driven by the column
/// count, so a row count that is merely close costs nothing visible, whereas
/// computing it here would mean duplicating the tab bar and key row geometry
/// that `apply_dimensions` owns, and quietly disagreeing with it later.
///
/// This overrides `initial_cols` even if the config sets it. On a platform
/// where the window cannot be opened at a requested size, that setting has
/// nothing to describe.
fn presize_initial_grid() {
    let Some(app) = window::os::android::try_android_app() else {
        return;
    };
    let Some(pixel_width) = wait_for_surface_width(app) else {
        log::warn!("no ANativeWindow arrived; the first pane may lose its prompt");
        return;
    };

    // Matches `Connection::default_dpi`; see the reasoning there.
    const DP_PER_POINT: f64 = 72. / 160.;
    let dpi = app.config().density().unwrap_or(160) as f64 * DP_PER_POINT;

    let config = config::configuration();
    let cell_width = match crate::cell_pixel_dims(&config, dpi) {
        Ok((width, _height)) if width > 0 => width,
        Ok(_) => return,
        Err(err) => {
            log::warn!("cannot measure the cell to presize the first pane: {err:#}");
            return;
        }
    };

    let context = config::DimensionContext {
        dpi: dpi as f32,
        pixel_max: pixel_width as f32,
        pixel_cell: cell_width as f32,
    };
    let padding_left = config.window_padding.left.evaluate_as_pixels(context) as usize;
    let padding_right = crate::termwindow::resize::effective_right_padding(&config, context);

    let cols = pixel_width.saturating_sub(padding_left + padding_right) / cell_width;
    if cols == 0 {
        return;
    }

    log::info!("presizing the first pane to {cols} columns for a {pixel_width}px window");
    if let Err(err) =
        config::set_config_overrides(&[("initial_cols".to_string(), cols.to_string())])
    {
        log::warn!("cannot presize the first pane: {err:#}");
        return;
    }
    config::reload();
}

/// Pump the event loop until Android hands over a surface, and report how wide
/// it is.
///
/// `AndroidApp` only learns about the window from inside `poll_events`, so
/// this cannot be a plain wait. Draining events here is safe because there is
/// no window yet for the connection to deliver them to: everything it does
/// with an event is done to each window it knows about, and it knows about
/// none until the GUI has started.
fn wait_for_surface_width(app: &AndroidApp) -> Option<usize> {
    // Long enough to cover a cold start on a slow device, short enough that a
    // surface that is never coming does not hold the terminal hostage.
    const DEADLINE: Duration = Duration::from_secs(5);

    let start = Instant::now();
    loop {
        match app.native_window() {
            Some(native_window) => {
                return match native_window.width() {
                    width if width > 0 => Some(width as usize),
                    _ => None,
                };
            }
            None => match DEADLINE.checked_sub(start.elapsed()) {
                Some(remaining) if !remaining.is_zero() => {
                    app.poll_events(Some(remaining.min(Duration::from_millis(50))), |_| {});
                }
                _ => return None,
            },
        }
    }
}

/// True when `name` refers to a domain served by another process, whether over
/// ssh, TLS or a unix socket.
fn is_remote_domain(config: &config::ConfigHandle, name: &str) -> bool {
    config
        .ssh_domains
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|d| d.name == name)
        || config.tls_clients.iter().any(|d| d.name == name)
        || config.unix_domains.iter().any(|d| d.name == name)
}

/// Where wezterm's per-user state lives on this device.
#[derive(Debug, Clone)]
pub struct AndroidDirs {
    /// `Context.getFilesDir()`; private to the app, and the value of `HOME`.
    pub home: PathBuf,
    /// `$HOME/.config/wezterm`, where `wezterm.lua` is read from.
    pub config: PathBuf,
    /// The app cache dir; `TMPDIR` and `XDG_RUNTIME_DIR`.
    pub cache: PathBuf,
    /// The APK's native library directory. This is the only directory an app
    /// may both read and execute from since API 29, so it is where a bundled
    /// shell has to live, and it goes on `PATH`.
    pub native_lib: Option<PathBuf>,
    /// Symlinks that give the bundled binaries their real names; see
    /// `prefix.rs`.
    pub prefix: prefix::Prefix,
}

/// Populate the environment an app process does not get for free.
///
/// This must run before anything touches `config`.
pub fn bootstrap_env(app: &AndroidApp) -> anyhow::Result<AndroidDirs> {
    let home = app
        .internal_data_path()
        .ok_or_else(|| anyhow::anyhow!("Context.getFilesDir() is unavailable"))?;

    // internal_data_path is .../files; use it directly as HOME so that dotfiles
    // written by shells land somewhere private and backed up with the app.
    std::fs::create_dir_all(&home)
        .map_err(|err| anyhow::anyhow!("creating {}: {err}", home.display()))?;

    let config_home = home.join(".config");
    let config = config_home.join("wezterm");
    std::fs::create_dir_all(&config)
        .map_err(|err| anyhow::anyhow!("creating {}: {err}", config.display()))?;

    // There is no getCacheDir() binding in android-activity, but the cache dir
    // is a documented sibling of the files dir, so derive it and fall back to
    // a subdirectory of HOME if the layout ever differs.
    let cache = match home.parent().map(|p| p.join("cache")) {
        Some(cache) if create_dir_ok(&cache) => cache,
        _ => {
            let cache = home.join(".cache");
            std::fs::create_dir_all(&cache)
                .map_err(|err| anyhow::anyhow!("creating {}: {err}", cache.display()))?;
            cache
        }
    };

    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::env::set_var("XDG_CACHE_HOME", &cache);
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    std::env::set_var("XDG_RUNTIME_DIR", &cache);
    std::env::set_var("TMPDIR", &cache);

    // A terminal with no TERM is not much of a terminal.
    if std::env::var_os("TERM").is_none() {
        std::env::set_var("TERM", "xterm-256color");
    }
    if std::env::var_os("LANG").is_none() {
        std::env::set_var("LANG", "en_US.UTF-8");
    }

    let native_lib = native_library_dir();

    // Give any bundled binaries usable names before PATH and SHELL are
    // derived from them.
    let prefix = prefix::populate(&home, native_lib.as_deref());

    set_path(&prefix.bin, native_lib.as_deref());

    // portable-pty falls back to /bin/sh, which does not exist on Android, so
    // SHELL has to be set explicitly. CommandBuilder consults the environment
    // before the passwd database, so setting it here is enough.
    std::env::set_var("SHELL", prefix.shell());

    Ok(AndroidDirs {
        home,
        config,
        cache,
        native_lib,
        prefix,
    })
}

fn create_dir_ok(path: &Path) -> bool {
    std::fs::create_dir_all(path).is_ok()
}

/// Locate the APK's native library directory.
///
/// The process was loaded from it, so `/proc/self/maps` names it; this avoids
/// a JNI round trip to `ApplicationInfo.nativeLibraryDir` during the earliest
/// part of startup.
fn native_library_dir() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    for line in maps.lines() {
        // Anonymous mappings have no pathname at all; skip them rather than
        // giving up on the scan.
        let path = match line.split_once('/') {
            Some((_, rest)) => format!("/{rest}"),
            None => continue,
        };
        if path.ends_with("/libwezterm_gui.so") || path.ends_with("/libmain.so") {
            return Path::new(&path).parent().map(Path::to_path_buf);
        }
    }
    None
}

/// Build a `PATH` that can actually be used to spawn something.
///
/// Since API 29 an app may not execute binaries out of its writable data
/// directory (W^X), and SELinux constrains most of the rest, so the native
/// library directory -- the one place an app may both read and execute from --
/// comes first. Anything bundled there is shipped named `lib*.so` so that the
/// installer extracts it with the execute bit set.
fn set_path(prefix_bin: &Path, native_lib: Option<&Path>) {
    let mut entries: Vec<String> = vec![prefix_bin.display().to_string()];

    if let Some(dir) = native_lib {
        entries.push(dir.display().to_string());
    }

    // The system directories are still worth having: toybox lives in
    // /system/bin on every modern Android and provides a usable, if minimal,
    // set of utilities even with nothing bundled.
    for dir in ["/system/bin", "/system/xbin", "/vendor/bin"] {
        if Path::new(dir).is_dir() {
            entries.push(dir.to_string());
        }
    }

    std::env::set_var("PATH", entries.join(":"));
}

/// Route `log` output to logcat.
///
/// `env_bootstrap::bootstrap()` installs wezterm's own logger later, which
/// writes to stderr and to a log file under the data dir; stderr goes nowhere
/// on Android, so this earlier logger is what makes the startup path
/// diagnosable with `adb logcat`.
fn init_logging() {
    use std::sync::OnceLock;
    static LOGGER: OnceLock<LogcatLogger> = OnceLock::new();

    let logger = LOGGER.get_or_init(|| LogcatLogger);
    let _ = log::set_logger(logger);
    log::set_max_level(log::LevelFilter::Info);
}

struct LogcatLogger;

impl log::Log for LogcatLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        use std::ffi::CString;

        let priority = match record.level() {
            log::Level::Error => 6, // ANDROID_LOG_ERROR
            log::Level::Warn => 5,
            log::Level::Info => 4,
            log::Level::Debug => 3,
            log::Level::Trace => 2,
        };

        let tag = CString::new(record.target())
            .unwrap_or_else(|_| CString::new("wezterm").expect("literal has no NUL"));
        let msg = match CString::new(format!("{}", record.args())) {
            Ok(msg) => msg,
            // The message contained an interior NUL; drop it rather than lose
            // the whole log line.
            Err(err) => {
                let mut bytes = err.into_vec();
                bytes.retain(|b| *b != 0);
                CString::new(bytes).expect("no NULs remain")
            }
        };

        // Safety: both pointers are valid NUL-terminated C strings for the
        // duration of the call.
        unsafe {
            ndk_sys::__android_log_write(priority, tag.as_ptr(), msg.as_ptr());
        }
    }

    fn flush(&self) {}
}
