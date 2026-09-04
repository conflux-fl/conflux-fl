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
}

/// A command's answer, in both renderings, plus the exit code it earns.
pub struct Report {
    /// The text rendering, printed as is.
    pub text: String,
    /// The JSON rendering.
    pub json: serde_json::Value,
    /// `0` for a positive answer, [`crate::EXIT_NEGATIVE`] otherwise.
    pub exit_code: i32,
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
    }
}
