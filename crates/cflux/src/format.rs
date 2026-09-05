//! The `--format` renderer split. A command computes one [`Report`]
//! carrying both renderings; only this module decides which one reaches
//! stdout, so structured output is never an afterthought bolted onto a
//! command that only knew how to print text.

use clap::ValueEnum;

/// Which rendering the caller wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable text.
    Pretty,
    /// One JSON document, for scripts and CI.
    Json,
    /// Text, preceded by GitHub Actions workflow commands so findings
    /// appear as annotations on a pull request rather than only in the
    /// job log.
    GithubActions,
}

/// One GitHub Actions annotation. A command with nothing to annotate
/// produces none, and this format then degrades to plain text.
pub struct Annotation {
    /// `error`, `warning`, or `notice`.
    pub level: &'static str,
    /// The message. Newlines are escaped at print time — a workflow
    /// command ends at the first one.
    pub message: String,
}

/// A command's answer, in both renderings, plus the exit code it earns.
pub struct Report {
    /// The text rendering, printed as is.
    pub text: String,
    /// The JSON rendering.
    pub json: serde_json::Value,
    /// `0` for a positive answer, [`crate::EXIT_NEGATIVE`] otherwise.
    pub exit_code: i32,
    /// What to annotate in CI, when the caller asked for that format.
    pub annotations: Vec<Annotation>,
}

impl Report {
    /// A report with nothing to annotate — most commands.
    pub fn plain(text: String, json: serde_json::Value, exit_code: i32) -> Self {
        Self {
            text,
            json,
            exit_code,
            annotations: Vec::new(),
        }
    }
}

/// Prints the requested rendering to stdout.
pub fn print(report: &Report, format: Format) {
    match format {
        Format::Pretty => {
            print!("{}", report.text);
            if !report.text.ends_with('\n') {
                println!();
            }
        }
        Format::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report.json).expect("serializable")
        ),
        Format::GithubActions => {
            for a in &report.annotations {
                println!("::{}::{}", a.level, a.message.replace('\n', "%0A"));
            }
            print!("{}", report.text);
        }
    }
}
