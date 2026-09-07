//! Profile files that were sitting in the profile directory and that
//! this run did not read.
//!
//! `cflux init hospital` writes two profiles and prints the line that
//! selects them. Skip that line and the server starts on the builtins —
//! correctly, and with a startup log whose provenance is entirely
//! accurate — while two hand-edited files sit unread beside it. Nothing
//! is wrong enough to fail, which is exactly why nothing said anything.
//!
//! # The rule, and where its edge is
//!
//! **Only an axis where nothing was selected at all is reported.**
//! Setting `CONFLUX_TOPOLOGY=cross_silo` is a choice, and a directory of
//! profiles beside a chosen one is a library, not a mistake — a
//! deployment that keeps six profiles and selects one must not be told
//! about the other five on every start. So the check fires on *unset*,
//! which is the case where nobody chose anything and the files are
//! evidence that somebody meant to.
//!
//! That covers the half-slip too: `init` writes two files, so exporting
//! one variable and forgetting the other is at least as likely as
//! forgetting both, and the axis that was left unset still speaks.
//!
//! # Why this warns instead of refusing
//!
//! Elsewhere the framework refuses rather than warning — a batch outside
//! its method's cited requirement halts the round rather than
//! aggregating anyway. That rule is about the configuration that *is*
//! running violating something it promised. This is the opposite shape:
//! nothing is violated, the builtins are a legal and deliberate default,
//! and a fresh checkout with a `profiles/` directory must still start.
//! Refusing here would make the presence of a file an error.

use std::path::Path;

use crate::profile::{ProfileError, load_mode_profile, load_topology_profile};

/// Which of the two configuration axes a profile file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileAxis {
    /// Its `inherits` chain terminates at a topology builtin.
    Topology,
    /// Its chain terminates at a mode builtin.
    Mode,
}

impl ProfileAxis {
    /// Both axes, so a caller can report them in a loop rather than
    /// writing the same block twice with two nouns swapped.
    pub const ALL: [ProfileAxis; 2] = [ProfileAxis::Topology, ProfileAxis::Mode];

    /// The axis's name, spelled as the profile loader's errors spell it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Topology => "topology",
            Self::Mode => "mode",
        }
    }

    /// The environment variable that selects a profile on this axis —
    /// the one thing the reader has to do about the warning, so it comes
    /// from here rather than being retyped at each call site.
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Topology => "CONFLUX_TOPOLOGY",
            Self::Mode => "CONFLUX_MODE",
        }
    }
}

/// A file that claims to be a profile and loads as neither axis.
///
/// Invisible until now: an unselected profile is never loaded, so a
/// misspelled key or a file shadowing a builtin waits silently until the
/// day someone finally selects it.
#[derive(Debug, Clone)]
pub struct UnusableProfile {
    /// The name that would select it — the file stem, without `.toml`.
    pub name: String,
    /// Why it does not load, in the profile loader's own words.
    pub problem: String,
}

/// What a profile directory held that this run did not read.
#[derive(Debug, Clone, Default)]
pub struct UnselectedProfiles {
    /// Topology profiles found when nothing selected a topology.
    /// Always empty when a topology was named, builtin or not.
    pub topology: Vec<String>,
    /// Mode profiles found when nothing selected a mode.
    pub mode: Vec<String>,
    /// Files that declare `inherits` but load as neither axis.
    pub unusable: Vec<UnusableProfile>,
}

impl UnselectedProfiles {
    /// Whether there is nothing to say.
    pub fn is_empty(&self) -> bool {
        self.topology.is_empty() && self.mode.is_empty() && self.unusable.is_empty()
    }

    /// How many files this describes, across both axes and the unusable.
    pub fn len(&self) -> usize {
        self.topology.len() + self.mode.len() + self.unusable.len()
    }

    /// The unselected profiles on one axis.
    pub fn on(&self, axis: ProfileAxis) -> &[String] {
        match axis {
            ProfileAxis::Topology => &self.topology,
            ProfileAxis::Mode => &self.mode,
        }
    }
}

/// Surveys `dir` for profiles this run did not read.
///
/// `topology_selected` and `mode_selected` are the names resolution was
/// given — `None` meaning nothing was selected on that axis, which is
/// the only case an axis is reported in. Both `Some` returns empty
/// without touching the filesystem.
///
/// Never fails. An unreadable directory is not a finding: the default
/// (`profiles`, relative to the working directory) usually does not
/// exist, and "you have no profile directory" is not news.
pub fn unselected_profiles(
    dir: &Path,
    topology_selected: Option<&str>,
    mode_selected: Option<&str>,
) -> UnselectedProfiles {
    let mut found = UnselectedProfiles::default();
    if topology_selected.is_some() && mode_selected.is_some() {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            (path.extension()? == "toml").then(|| path.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    // Sorted so two runs of the same deployment log the same line, which
    // is what makes the line diffable across restarts.
    names.sort();

    for name in names {
        // A `.toml` without `inherits` is not claiming to be a profile,
        // and the directory is ours only by convention — the default is
        // a relative path anyone could already be using. Staying quiet
        // about files that never said they were profiles is the price of
        // not lecturing people about their own directory; a real profile
        // missing `inherits` still gets the loader's full explanation the
        // moment it is selected.
        if !declares_inherits(dir, &name) {
            continue;
        }
        match classify(dir, &name) {
            Ok(ProfileAxis::Topology) if topology_selected.is_none() => found.topology.push(name),
            Ok(ProfileAxis::Mode) if mode_selected.is_none() => found.mode.push(name),
            // Its axis was chosen; the rest of that axis is a library.
            Ok(_) => {}
            Err(problem) => found.unusable.push(UnusableProfile { name, problem }),
        }
    }
    found
}

/// Whether `<name>.toml` declares `inherits` — the one key every profile
/// must have, and so the cheapest honest test of "this file means to be
/// a profile".
fn declares_inherits(dir: &Path, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(dir.join(format!("{name}.toml"))) else {
        return false;
    };
    match text.parse::<toml::Table>() {
        Ok(table) => table.get("inherits").is_some(),
        // Unparseable TOML is not a profile we can say anything useful
        // about, and it fails loudly with the parser's own message the
        // moment it is selected.
        Err(_) => false,
    }
}

/// Which axis `name` loads on, or why it loads on neither.
///
/// The discriminator is [`ProfileError::NotFound`], which is precisely
/// "this chain does not terminate at *this* axis's builtins" — which is
/// what a mode profile looks like from the topology side. Any other
/// error means the chain did reach this axis and something else is
/// wrong, so it is reported with that axis's own message rather than
/// being retried as the other axis and losing the explanation.
fn classify(dir: &Path, name: &str) -> Result<ProfileAxis, String> {
    match load_topology_profile(dir, name) {
        Ok(_) => return Ok(ProfileAxis::Topology),
        Err(ProfileError::NotFound { .. }) => {}
        Err(e) => return Err(e.to_string()),
    }
    match load_mode_profile(dir, name) {
        Ok(_) => Ok(ProfileAxis::Mode),
        // Not found on either axis: the chain leads somewhere that is
        // neither builtin set. Reporting either axis's message here
        // would name the wrong builtins with total confidence, so this
        // says only what is actually known.
        Err(ProfileError::NotFound { .. }) => Err(concat!(
            "declares `inherits`, but the chain reaches neither a topology ",
            "builtin nor a mode one — check what it inherits from"
        )
        .to_string()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// A fresh directory per test, under the OS temp dir — real files,
    /// because reading a real directory is the whole job.
    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "conflux-unselected-tests-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.toml")), body).unwrap();
    }

    /// The `cflux init` pair: one profile per axis, neither selected.
    fn init_pair(dir: &Path) {
        write(dir, "hospital", "inherits = \"cross_silo\"\n");
        write(dir, "hospital_mode", "inherits = \"production\"\n");
    }

    #[test]
    fn nothing_selected_names_both_axes() {
        // The whole point: `init` wrote two files, the export line was
        // skipped, and the builtins are silently in force.
        let d = dir();
        init_pair(&d);

        let found = unselected_profiles(&d, None, None);

        assert_eq!(found.topology, vec!["hospital".to_string()]);
        assert_eq!(found.mode, vec!["hospital_mode".to_string()]);
        assert!(found.unusable.is_empty());
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn only_the_axis_that_was_left_unset_speaks() {
        // The half-slip: `init` writes two files, so exporting one
        // variable and forgetting the other is at least as likely as
        // forgetting both.
        let d = dir();
        init_pair(&d);

        let found = unselected_profiles(&d, Some("hospital"), None);

        assert!(found.topology.is_empty());
        assert_eq!(found.mode, vec!["hospital_mode".to_string()]);
    }

    #[test]
    fn naming_a_builtin_is_a_choice_not_an_omission() {
        // This is the rule's edge. A deployment that keeps a library of
        // profiles and deliberately runs `cross_silo` must not be told
        // about the others on every start.
        let d = dir();
        init_pair(&d);
        write(&d, "clinic", "inherits = \"cross_device\"\n");

        let found = unselected_profiles(&d, Some("cross_silo"), Some("research"));

        assert!(found.is_empty());
    }

    #[test]
    fn a_toml_that_never_claimed_to_be_a_profile_is_left_alone() {
        // The directory is ours by convention only — the default is a
        // relative path anyone could already be using.
        let d = dir();
        write(&d, "notes", "title = \"scratch\"\n");
        write(&d, "malformed", "this is not = = toml\n");

        assert!(unselected_profiles(&d, None, None).is_empty());
    }

    #[test]
    fn a_profile_that_loads_on_neither_axis_is_named_with_why() {
        // Invisible before this: an unselected profile is never loaded,
        // so a wrong-axis key waits until the day someone selects it.
        let d = dir();
        write(
            &d,
            "confused",
            "inherits = \"cross_silo\"\nallow_stub_client = true\n",
        );

        let found = unselected_profiles(&d, None, None);

        assert!(found.topology.is_empty());
        assert_eq!(found.unusable.len(), 1);
        assert_eq!(found.unusable[0].name, "confused");
        // The topology loader's own message, not a summary of it: it
        // names the axis that owns the key, which is the thing to do.
        assert!(
            found.unusable[0].problem.contains("mode-axis parameter"),
            "problem was: {}",
            found.unusable[0].problem
        );
    }

    #[test]
    fn a_file_shadowing_a_builtin_is_reported_before_anyone_selects_it() {
        // Leaving a topology unset never touches the directory, so this
        // file could sit there indefinitely without a word.
        let d = dir();
        write(&d, "cross_device", "inherits = \"cross_silo\"\n");

        let found = unselected_profiles(&d, None, None);

        assert_eq!(found.unusable.len(), 1);
        assert!(
            found.unusable[0].problem.contains("shadows the builtin"),
            "problem was: {}",
            found.unusable[0].problem
        );
    }

    #[test]
    fn a_chain_reaching_neither_builtin_set_claims_neither() {
        // Naming one axis's builtins here would be confidently wrong
        // half the time, so the message says only what is known.
        let d = dir();
        write(&d, "orphan", "inherits = \"nowhere\"\n");

        let found = unselected_profiles(&d, None, None);

        assert_eq!(found.unusable.len(), 1);
        assert!(found.unusable[0].problem.contains("neither a topology"));
    }

    #[test]
    fn a_missing_directory_is_not_a_finding() {
        // The default `profiles` is relative and usually absent; "you
        // have no profile directory" is not news.
        let found = unselected_profiles(Path::new("/nonexistent-profile-dir"), None, None);
        assert!(found.is_empty());
    }

    #[test]
    fn both_axes_selected_never_touches_the_filesystem() {
        // Cheap, and it is what makes a configured deployment silent
        // rather than merely quiet.
        let d = dir();
        write(&d, "cross_device", "inherits = \"cross_silo\"\n");

        assert!(unselected_profiles(&d, Some("a"), Some("b")).is_empty());
    }

    #[test]
    fn each_axis_knows_the_variable_that_selects_it() {
        assert_eq!(ProfileAxis::Topology.env_var(), "CONFLUX_TOPOLOGY");
        assert_eq!(ProfileAxis::Mode.env_var(), "CONFLUX_MODE");
        let found = UnselectedProfiles {
            topology: vec!["a".to_string()],
            ..Default::default()
        };
        assert_eq!(found.on(ProfileAxis::Topology), ["a".to_string()]);
        assert!(found.on(ProfileAxis::Mode).is_empty());
    }
}
