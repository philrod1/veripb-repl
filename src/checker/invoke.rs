//! The actual subprocess plumbing: write a candidate proof to a temp
//! file, spawn `veripb` against it and the (already on-disk) formula,
//! and hand back whatever it produced.

use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;

use anyhow::Context;

/// The name (or path) of the `veripb` binary to invoke — an environment
/// variable so a specific build can be pointed at during development or
/// testing without touching `$PATH`; falls back to bare `"veripb"`,
/// resolved via `$PATH` the normal way, when unset.
fn veripb_binary() -> std::ffi::OsString {
    std::env::var_os("VERIPB_REPL_VERIPB_BIN").unwrap_or_else(|| "veripb".into())
}

/// Everything one `veripb` invocation produced, as-is.
pub struct RawInvocation {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
    pub code: Option<i32>,
}

/// Run `veripb <formula_path> <temp file containing proof_text> ...
/// extra_args`, and capture everything it produced. The `proof_text` is
/// written to a fresh temp file with a `.pbp` suffix.
///
/// Fails (via the outer `Result`) only for a genuine invocation problem
/// — the binary couldn't be spawned at all, or the temp file couldn't be
/// written — never because the proof itself was rejected; that's a
/// normal, expected shape of `RawInvocation`'s own contents for
/// `checker::parse` to recognise, not a REPL error as such.
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

    let output = std::process::Command::new(veripb_binary())
        .arg(formula_path)
        .arg(proof_file.path())
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
    })
}

/// [`run`], plus reading back a `--elaborate` scratch file afterward.
pub struct ElaboratedInvocation {
    pub raw: RawInvocation,
    pub elaborated_proof: String,
}

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
