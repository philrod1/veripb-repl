//! Subprocess plumbing: writes a candidate proof to a temp file, spawns
//! `veripb`, and returns its raw output.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Returns the `veripb` binary to invoke: `$VERIPB_REPL_VERIPB_BIN` if
/// set, otherwise `"veripb"` resolved via `$PATH`.
fn veripb_binary() -> std::ffi::OsString {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").unwrap_or_else(|| "veripb".into())
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
}

/// Runs `veripb <formula_path> <proof_text> ...extra_args` and returns
/// its output. `proof_text` is written to a temp `.pbp` file first.
///
/// Returns `Err` only for an invocation failure — the binary couldn't be
/// spawned, or the temp file couldn't be written — never because the
/// proof was rejected; that's a normal outcome captured in
/// `RawInvocation` for `checker::parse` to interpret.
pub fn run<I, S>(formula_path: &Path, proof_text: &str, extra_args: I) -> anyhow::Result<RawInvocation>
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

    let output = std::process::Command::new(veripb_binary())
        .arg(formula_path)
        .arg(&proof_file_path)
        .args(extra_args)
        .output()
        .with_context(|| {
            format!(
                "failed to run `{}` — is it installed and on PATH?",
                veripb_binary().to_string_lossy()
            )
        })?;

    Ok(RawInvocation {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
        code: output.status.code(),
        proof_file_path,
    })
}

/// The result of [`run_with_elaboration`].
pub struct ElaboratedInvocation {
    pub raw: RawInvocation,
    pub elaborated_proof: String,
}

/// Runs [`run`] with `--elaborate`, and returns the elaborated proof
/// text it wrote.
pub fn run_with_elaboration(formula_path: &Path, proof_text: &str) -> anyhow::Result<ElaboratedInvocation> {
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

/// Runs [`run`] with `--dump-database` plus any `extra_args`, and returns
/// the database dump it wrote.
pub fn run_with_database_dump<I, S>(
    formula_path: &Path,
    proof_text: &str,
    extra_args: I,
) -> anyhow::Result<DatabaseDumpInvocation>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let scratch = tempfile::NamedTempFile::new()
        .context("failed to create a scratch file for the database dump")?;
    let mut args: Vec<std::ffi::OsString> = vec!["--dump-database".into(), scratch.path().into()];
    args.extend(extra_args.into_iter().map(|s| s.as_ref().to_os_string()));

    let raw = run(formula_path, proof_text, args)?;
    let database_dump = std::fs::read_to_string(scratch.path())
        .context("failed to read back the database dump scratch file")?;

    // A genuine `--dump-database` run always writes at least the version
    // header, whether it accepted the proof, hit a rejection, or ran out
    // of input — see `checker::parse`'s own docs. An empty dump here means
    // the invocation itself didn't behave like that: most likely the
    // resolved `veripb` binary doesn't support `--dump-database` at all
    // (an old build, or the wrong one on $PATH/$VERIPB_REPL_VERIPB_BIN).
    // Surface the raw invocation so that's diagnosable, instead of
    // `checker::parse::parse_database_dump`'s much later, context-free
    // "missing its version header line".
    anyhow::ensure!(
        !database_dump.is_empty(),
        "veripb wrote no `--dump-database` output (exit code {:?}) — the resolved `veripb` \
         binary may not support `--dump-database`, or may be an older build than this REPL \
         expects. Raw output:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        raw.code,
        raw.stdout,
        raw.stderr,
    );

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

    // Same reasoning as `run_with_database_dump`'s own check: a genuine
    // `--dump-objective` run always writes at least the version header.
    anyhow::ensure!(
        !objective_dump.is_empty(),
        "veripb wrote no `--dump-objective` output (exit code {:?}) — the resolved `veripb` \
         binary may not support `--dump-objective`, or may be an older build than this REPL \
         expects. Raw output:\n--- stdout ---\n{}\n--- stderr ---\n{}",
        raw.code,
        raw.stdout,
        raw.stderr,
    );

    Ok(ObjectiveDumpInvocation { raw, objective_dump })
}
