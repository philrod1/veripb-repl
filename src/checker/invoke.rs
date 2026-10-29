//! Subprocess plumbing: writes a candidate proof to a temp file, spawns
//! `veripb`, and returns its raw output.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;

use super::ProofText;

/// Returns the `veripb` binary to invoke: `$VERIPB_REPL_VERIPB_BIN` if
/// set, otherwise `"veripb"` resolved via `$PATH`.
fn veripb_binary() -> std::ffi::OsString {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").unwrap_or_else(|| "veripb".into())
}

thread_local! {
    static INVOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static DATABASE_DUMPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns how many times this thread has run `veripb`.
#[doc(hidden)]
pub fn invocation_count() -> usize {
    INVOCATIONS.with(std::cell::Cell::get)
}

/// Returns how many of [`invocation_count`]'s runs dumped the database.
#[doc(hidden)]
pub fn database_dump_count() -> usize {
    DATABASE_DUMPS.with(std::cell::Cell::get)
}

/// The raw output of one `veripb` invocation.
pub struct RawInvocation {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub code: Option<i32>,
    /// The temp file (or pipe) the proof was written to and passed to `veripb`
    /// as its `<DERIVATION>` argument — a rejection's error text names
    /// this path, so `checker::parse` anchors its line-number search to
    /// it.
    pub proof_file_path: PathBuf,
    /// The formula file passed as `<FORMULA>`; a formula syntax error names
    /// this path.
    pub formula_file_path: PathBuf,
}

/// Runs `veripb <formula_path> <proof> ...extra_args` and returns its
/// output. `proof` is written to a temp `.pbp` file first.
///
/// Returns `Err` only for an invocation failure — the binary couldn't be
/// spawned, or the temp file couldn't be written — never because the
/// proof was rejected; that's a normal outcome captured in
/// `RawInvocation` for `checker::parse` to interpret.
pub fn run<'a, I, S>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    extra_args: I,
) -> anyhow::Result<RawInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_streaming(formula_path, proof, extra_args, |_| false)
}

/// Like [`run`], passing each line of `veripb`'s stdout (without its
/// newline) to `consume` as `veripb` writes it. Lines `consume` returns
/// `true` for are left out of [`RawInvocation::stdout`]: a trace can run to
/// several times the proof's size, too much to hold.
pub fn run_streaming<'a, I, S>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    extra_args: I,
    consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<RawInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(run_watched(formula_path, proof, extra_args, consume, None)?.0)
}

/// A file `veripb` writes and the most bytes it may reach: past that,
/// `veripb` is stopped (see [`run_watched`]).
#[derive(Clone, Copy)]
struct Watch<'p> {
    path: &'p Path,
    limit: u64,
}

/// [`run_streaming`], stopping `veripb` if `watch`'s file grows past its
/// limit. Returns whether it was stopped.
fn run_watched<'a, I, S>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    extra_args: I,
    mut consume: impl FnMut(&str) -> bool,
    watch: Option<Watch<'_>>,
) -> anyhow::Result<(RawInvocation, bool)>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let proof = proof.into();
    let extra_args: Vec<OsString> = extra_args
        .into_iter()
        .map(|s| s.as_ref().to_os_string())
        .collect();

    #[cfg(unix)]
    if let Some(reporter) = super::progress::reporter()
        && let Ok(pipe) = NamedPipe::new()
    {
        return run_through_pipe(
            pipe,
            formula_path,
            proof,
            extra_args,
            &mut consume,
            reporter,
            watch,
        );
    }

    let mut proof_file = tempfile::Builder::new()
        .suffix(".pbp")
        .tempfile()
        .context("failed to create a temp file for the candidate proof")?;

    let mut writer = std::io::BufWriter::new(proof_file.as_file_mut());
    proof
        .write_to(&mut writer, || true)
        .and_then(|()| writer.flush())
        .context("failed to write the candidate proof to a temp file")?;
    drop(writer);

    let proof_file_path = proof_file.path().to_path_buf();

    let mut args: Vec<OsString> = vec![
        formula_path.as_os_str().to_os_string(),
        proof_file_path.as_os_str().to_os_string(),
    ];
    args.extend(extra_args);
    let output = run_veripb(&args, &mut consume, watch)?;

    let raw = RawInvocation {
        stdout: output.stdout,
        stderr: output.stderr,
        success: output.success,
        code: output.code,
        proof_file_path,
        formula_file_path: formula_path.to_path_buf(),
    };
    Ok((raw, output.stopped))
}

/// A named pipe in a temp directory of its own (removed on drop).
#[cfg(unix)]
struct NamedPipe {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

#[cfg(unix)]
impl NamedPipe {
    fn new() -> std::io::Result<Self> {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::Builder::new().prefix(".veripb-repl").tempdir()?;
        let path = dir.path().join("proof.pbp");
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: `c_path` is a NUL-terminated path.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(NamedPipe { _dir: dir, path })
    }
}

/// [`run_streaming`], handing `proof` to `veripb` through `pipe` and
/// reporting progress to `reporter` (see [`super::progress`]).
#[cfg(unix)]
fn run_through_pipe(
    pipe: NamedPipe,
    formula_path: &Path,
    proof: ProofText<'_>,
    extra_args: Vec<OsString>,
    consume: &mut dyn FnMut(&str) -> bool,
    reporter: super::progress::Reporter,
    watch: Option<Watch<'_>>,
) -> anyhow::Result<(RawInvocation, bool)> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::progress::{INTERVAL, Progress};

    let mut args: Vec<OsString> = vec![
        formula_path.as_os_str().to_os_string(),
        pipe.path.as_os_str().to_os_string(),
    ];
    args.extend(extra_args);
    let total = proof.line_count();
    let written = AtomicUsize::new(0);
    let done = AtomicBool::new(false);

    let output = std::thread::scope(|scope| {
        let (written, done, path) = (&written, &done, &pipe.path);
        let writer = scope.spawn(move || {
            // Blocks until `veripb` opens the pipe (or `done` is set and the
            // main thread opens it instead).
            let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) else {
                return;
            };
            // A write fails once `veripb` has exited, unless the main thread
            // has the pipe open by then: `done` stops it.
            let mut writer = std::io::BufWriter::new(file);
            let _ = proof
                .write_to(&mut writer, || {
                    written.fetch_add(1, Ordering::Relaxed);
                    !done.load(Ordering::Relaxed)
                })
                .and_then(|()| writer.flush());
        });
        scope.spawn(move || {
            let mut started = true;
            while !done.load(Ordering::Relaxed) {
                let line = written.load(Ordering::Relaxed).min(total);
                reporter(Progress {
                    started,
                    line,
                    total,
                });
                started = false;
                let next = std::time::Instant::now() + INTERVAL;
                while !done.load(Ordering::Relaxed) && std::time::Instant::now() < next {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        });

        let output = run_veripb(&args, consume, watch);
        done.store(true, Ordering::Relaxed);
        // `veripb` has exited, maybe without opening the pipe or reading
        // all of it: open it and read it out until the writer has stopped.
        let mut drain = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .ok();
        let mut buf = [0u8; 8192];
        while !writer.is_finished() {
            if let Some(drain) = &mut drain {
                let _ = drain.read(&mut buf);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        output
    })?;

    let raw = RawInvocation {
        stdout: output.stdout,
        stderr: output.stderr,
        success: output.success,
        code: output.code,
        proof_file_path: pipe.path.clone(),
        formula_file_path: formula_path.to_path_buf(),
    };
    Ok((raw, output.stopped))
}

/// What [`run_veripb`] returns: stdout without the lines its `consume`
/// took.
struct Output {
    stdout: String,
    stderr: String,
    success: bool,
    code: Option<i32>,
    /// Whether `veripb` was stopped for growing a watched file too large.
    stopped: bool,
}

/// How often a [`Watch`]ed file's size is checked.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// Runs `veripb` with `args`, reading its stdout a line at a time as it is
/// written (see [`keep_lines`]) and its stderr whole. Stops it if `watch`'s
/// file grows past its limit.
fn run_veripb(
    args: &[OsString],
    consume: &mut dyn FnMut(&str) -> bool,
    watch: Option<Watch<'_>>,
) -> anyhow::Result<Output> {
    INVOCATIONS.with(|count| count.set(count.get() + 1));
    let mut child = std::process::Command::new(veripb_binary())
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to run `{}` — is it installed and on PATH?",
                veripb_binary().to_string_lossy()
            )
        })?;
    // Drain stderr alongside, so a full stderr pipe cannot stall veripb.
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr_pipe.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stdout_pipe = child.stdout.take().expect("stdout is piped");
    let child = std::sync::Mutex::new(child);
    let done = std::sync::atomic::AtomicBool::new(false);
    let stopped = std::sync::atomic::AtomicBool::new(false);
    let (stdout, status) = std::thread::scope(|scope| {
        if let Some(watch) = watch {
            let (child, done, stopped) = (&child, &done, &stopped);
            scope.spawn(move || {
                use std::sync::atomic::Ordering;
                while !done.load(Ordering::Relaxed) {
                    let size = std::fs::metadata(watch.path).map_or(0, |m| m.len());
                    if size > watch.limit {
                        stopped.store(true, Ordering::Relaxed);
                        if let Ok(mut child) = child.lock() {
                            let _ = child.kill();
                        }
                        return;
                    }
                    std::thread::sleep(WATCH_INTERVAL);
                }
            });
        }
        let stdout = keep_lines(std::io::BufReader::new(stdout_pipe), consume)
            .context("failed to read veripb's stdout");
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let status = child
            .lock()
            .map_err(|_| anyhow::anyhow!("the veripb watcher panicked"))
            .and_then(|mut child| child.wait().context("failed to wait for veripb"));
        (stdout, status)
    });
    let (stdout, status) = (stdout?, status?);
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("the thread reading veripb's stderr panicked"))?
        .context("failed to read veripb's stderr")?;
    Ok(Output {
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        success: status.success(),
        code: status.code(),
        stopped: stopped.into_inner(),
    })
}

/// Reads `reader` a line at a time, returning the lines (newlines kept)
/// that `consume` does not take.
fn keep_lines(
    mut reader: impl BufRead,
    consume: &mut dyn FnMut(&str) -> bool,
) -> std::io::Result<String> {
    let mut kept = String::new();
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            return Ok(kept);
        }
        let line = String::from_utf8_lossy(&bytes);
        if !consume(line.trim_end_matches(['\n', '\r'])) {
            kept.push_str(&line);
        }
    }
}

/// The result of [`run_with_elaboration`].
pub struct ElaboratedInvocation {
    pub raw: RawInvocation,
    pub elaborated_proof: String,
}

/// Runs [`run`] with `--elaborate`, and returns the elaborated proof
/// text it wrote, or `None` if it passed `limit` bytes: `veripb` is then
/// stopped and its partial output deleted.
pub fn run_with_elaboration<'a>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    limit: u64,
) -> anyhow::Result<Option<ElaboratedInvocation>> {
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for elaboration output")?;
    let args: Vec<std::ffi::OsString> = vec!["--elaborate".into(), scratch.path().into()];

    let watch = Watch {
        path: scratch.path(),
        limit,
    };
    let (raw, stopped) = run_watched(formula_path, proof, args, |_| false, Some(watch))?;
    if stopped {
        return Ok(None);
    }
    let elaborated_proof = std::fs::read_to_string(scratch.path())
        .context("failed to read back the elaboration scratch file")?;

    Ok(Some(ElaboratedInvocation {
        raw,
        elaborated_proof,
    }))
}

/// The result of [`run_with_database_dump`].
pub struct DatabaseDumpInvocation {
    pub raw: RawInvocation,
    pub database_dump: String,
}

/// Runs [`run_streaming`] with `--dump-database` plus any `extra_args`, and
/// returns the database dump it wrote.
pub fn run_with_database_dump<'a, I, S>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    extra_args: I,
    consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<DatabaseDumpInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for the database dump")?;
    DATABASE_DUMPS.with(|dumps| dumps.set(dumps.get() + 1));
    let mut args: Vec<std::ffi::OsString> = vec!["--dump-database".into(), scratch.path().into()];
    args.extend(extra_args.into_iter().map(|s| s.as_ref().to_os_string()));

    let raw = run_streaming(formula_path, proof, args, consume)?;
    let database_dump = std::fs::read_to_string(scratch.path())
        .context("failed to read back the database dump scratch file")?;

    if database_dump.is_empty() {
        return Err(missing_dump_error(&raw, "--dump-database"));
    }

    Ok(DatabaseDumpInvocation { raw, database_dump })
}

/// The result of [`run_with_objective_dump`].
pub struct ObjectiveDumpInvocation {
    pub raw: RawInvocation,
    pub objective_dump: String,
}

/// Runs [`run`] with `--dump-objective` plus any `extra_args`, and returns
/// the objective-bounds dump it wrote.
pub fn run_with_objective_dump<'a, I, S>(
    formula_path: &Path,
    proof: impl Into<ProofText<'a>>,
    extra_args: I,
) -> anyhow::Result<ObjectiveDumpInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for the objective dump")?;
    let mut args: Vec<std::ffi::OsString> = vec!["--dump-objective".into(), scratch.path().into()];
    args.extend(extra_args.into_iter().map(|s| s.as_ref().to_os_string()));

    let raw = run(formula_path, proof, args)?;
    let objective_dump = std::fs::read_to_string(scratch.path())
        .context("failed to read back the objective dump scratch file")?;

    if objective_dump.is_empty() {
        return Err(missing_dump_error(&raw, "--dump-objective"));
    }

    Ok(ObjectiveDumpInvocation {
        raw,
        objective_dump,
    })
}

/// The error for a run that wrote no dump file (a real run of a supporting
/// `veripb` always writes at least the header): the formula's own error if
/// it has one; "unsupported flag" if `veripb` rejected `flag` as an
/// argument; otherwise `veripb`'s reason for failing.
fn missing_dump_error(raw: &RawInvocation, flag: &str) -> anyhow::Error {
    if let Some(message) = super::parse::formula_error(raw) {
        return anyhow::anyhow!(message);
    }
    if raw.stderr.contains(flag) {
        return anyhow::anyhow!(
            "the `veripb` in use doesn't support `{flag}` — see README.md for building an \
             up-to-date one"
        );
    }
    let combined = format!("{}{}", raw.stdout, raw.stderr);
    anyhow::anyhow!(
        "veripb wrote no `{flag}` output (exit code {:?}): {}",
        raw.code,
        super::parse::rejection_reason(&combined)
    )
}
