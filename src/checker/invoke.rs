//! Subprocess plumbing: writes a candidate proof to a temp file, spawns
//! `veripb`, and returns its raw output.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Returns the `veripb` binary to invoke: `$VERIPB_REPL_VERIPB_BIN` if
/// set, otherwise `"veripb"` resolved via `$PATH`.
fn veripb_binary() -> std::ffi::OsString {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").unwrap_or_else(|| "veripb".into())
}

thread_local! {
    static INVOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Returns how many times this thread has run `veripb`.
#[doc(hidden)]
pub fn invocation_count() -> usize {
    INVOCATIONS.with(std::cell::Cell::get)
}

/// The raw output of one `veripb` invocation.
pub struct RawInvocation {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub code: Option<i32>,
    /// The temp file `proof_text` was written to and passed to `veripb`
    /// as its `<DERIVATION>` argument — a rejection's error text names
    /// this path, so `checker::parse` anchors its line-number search to
    /// it.
    pub proof_file_path: PathBuf,
    /// The formula file passed as `<FORMULA>`; a formula syntax error names
    /// this path.
    pub formula_file_path: PathBuf,
}

/// Runs `veripb <formula_path> <proof_text> ...extra_args` and returns
/// its output. `proof_text` is written to a temp `.pbp` file first.
///
/// Returns `Err` only for an invocation failure — the binary couldn't be
/// spawned, or the temp file couldn't be written — never because the
/// proof was rejected; that's a normal outcome captured in
/// `RawInvocation` for `checker::parse` to interpret.
pub fn run<I, S>(
    formula_path: &Path,
    proof_text: &str,
    extra_args: I,
) -> anyhow::Result<RawInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_streaming(formula_path, proof_text, extra_args, |_| false)
}

/// Like [`run`], passing each line of `veripb`'s stdout (without its
/// newline) to `consume` as `veripb` writes it. Lines `consume` returns
/// `true` for are left out of [`RawInvocation::stdout`]: a trace can run to
/// several times the proof's size, too much to hold.
pub fn run_streaming<I, S>(
    formula_path: &Path,
    proof_text: &str,
    extra_args: I,
    mut consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<RawInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut proof_file = tempfile::Builder::new()
        .suffix(".pbp")
        .tempfile()
        .context("failed to create a temp file for the candidate proof")?;

    proof_file
        .write_all(proof_text.as_bytes())
        .context("failed to write the candidate proof to a temp file")?;

    let proof_file_path = proof_file.path().to_path_buf();

    let mut args: Vec<OsString> = vec![
        formula_path.as_os_str().to_os_string(),
        proof_file_path.as_os_str().to_os_string(),
    ];
    args.extend(extra_args.into_iter().map(|s| s.as_ref().to_os_string()));
    let output = run_veripb(&args, &mut consume)?;

    Ok(RawInvocation {
        stdout: output.stdout,
        stderr: output.stderr,
        success: output.success,
        code: output.code,
        proof_file_path,
        formula_file_path: formula_path.to_path_buf(),
    })
}

/// What [`run_veripb`] returns: stdout without the lines its `consume`
/// took.
struct Output {
    stdout: String,
    stderr: String,
    success: bool,
    code: Option<i32>,
}

/// Runs `veripb` with `args`, reading its stdout a line at a time as it is
/// written (see [`keep_lines`]) and its stderr whole.
fn run_veripb(args: &[OsString], consume: &mut dyn FnMut(&str) -> bool) -> anyhow::Result<Output> {
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
    let stdout = keep_lines(std::io::BufReader::new(stdout_pipe), consume)
        .context("failed to read veripb's stdout")?;
    let status = child.wait().context("failed to wait for veripb")?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("the thread reading veripb's stderr panicked"))?
        .context("failed to read veripb's stderr")?;
    Ok(Output {
        stdout,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        success: status.success(),
        code: status.code(),
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
/// text it wrote.
pub fn run_with_elaboration(
    formula_path: &Path,
    proof_text: &str,
) -> anyhow::Result<ElaboratedInvocation> {
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for elaboration output")?;
    let args: Vec<std::ffi::OsString> = vec!["--elaborate".into(), scratch.path().into()];

    let raw = run(formula_path, proof_text, args)?;
    let elaborated_proof = std::fs::read_to_string(scratch.path())
        .context("failed to read back the elaboration scratch file")?;

    Ok(ElaboratedInvocation {
        raw,
        elaborated_proof,
    })
}

/// The result of [`run_with_database_dump`].
pub struct DatabaseDumpInvocation {
    pub raw: RawInvocation,
    pub database_dump: String,
}

/// Runs [`run_streaming`] with `--dump-database` plus any `extra_args`, and
/// returns the database dump it wrote.
pub fn run_with_database_dump<I, S>(
    formula_path: &Path,
    proof_text: &str,
    extra_args: I,
    consume: impl FnMut(&str) -> bool,
) -> anyhow::Result<DatabaseDumpInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for the database dump")?;
    let mut args: Vec<std::ffi::OsString> = vec!["--dump-database".into(), scratch.path().into()];
    args.extend(extra_args.into_iter().map(|s| s.as_ref().to_os_string()));

    let raw = run_streaming(formula_path, proof_text, args, consume)?;
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
pub fn run_with_objective_dump<I, S>(
    formula_path: &Path,
    proof_text: &str,
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

    let raw = run(formula_path, proof_text, args)?;
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
