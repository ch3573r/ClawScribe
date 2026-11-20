//! The recordings folders the user chose, and what may be done inside them.
//!
//! Trust comes from a dialog the core opened, not from where a path points:
//! a folder the user picked in the core's folder picker (or that was already
//! the configured recordings folder before this version took over) is a root,
//! and that includes a network location. Text that arrives from the webview is
//! never trusted on its own: a path that is not on a local drive is looked at
//! only when its own spelling puts it inside a trusted root.
//!
//! The trust state is core-owned: it is kept in `trusted_folders.json` and the
//! webview has no permission to write the plugin store (see the capability list
//! in `tauri.conf.json`).
use crate::audio::recording_preferences::{is_system_top_level, ProtectedFolders};
use crate::behavior_settings::Backing;
use std::path::{Path, PathBuf};

pub const STORE_FILE: &str = "trusted_folders.json";
const TRUSTED: &str = "trusted";
const TAKEOVER_DONE: &str = "takeover_done";
const KEPT: &str = "kept_recordings";
const REGISTERED_DONE: &str = "registered_takeover_done";
const CODEX_DONE: &str = "codex_home_marked";
const MAX_TRUSTED: usize = 100;
/// The most parent folders of saved meetings taken over at once.
pub(crate) const MAX_REGISTERED_PARENTS: usize = 50;
const MAX_KEPT: usize = 500;

/// What the core has recorded about trusted folders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Trust {
    pub trusted: Vec<PathBuf>,
    /// The configured and previous folders of an earlier version were taken over.
    pub takeover_done: bool,
    /// Interrupted recordings the user asked not to be asked about again.
    pub kept: Vec<String>,
    /// The folders that saved meetings already live in were taken over.
    pub registered_done: bool,
    /// The custom Codex profile folder of an earlier version was marked.
    pub codex_done: bool,
}

fn strings(backing: &dyn Backing, key: &str) -> Vec<String> {
    backing
        .read(key)
        .and_then(|value| value.as_array().cloned())
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

pub(crate) fn read(backing: &dyn Backing) -> Trust {
    Trust {
        trusted: strings(backing, TRUSTED).into_iter().map(PathBuf::from).collect(),
        takeover_done: backing.read(TAKEOVER_DONE).and_then(|value| value.as_bool()) == Some(true),
        kept: strings(backing, KEPT),
        registered_done: flag(backing, REGISTERED_DONE),
        codex_done: flag(backing, CODEX_DONE),
    }
}

fn flag(backing: &dyn Backing, key: &str) -> bool {
    backing.read(key).and_then(|value| value.as_bool()) == Some(true)
}

/// Writes the state and saves; when the save fails the previous values are put back.
fn write(backing: &dyn Backing, trust: &Trust) -> Result<(), String> {
    let keys = [TRUSTED, TAKEOVER_DONE, KEPT, REGISTERED_DONE, CODEX_DONE];
    let previous: Vec<_> = keys.iter().map(|key| backing.read(key)).collect();
    backing.write(
        TRUSTED,
        serde_json::json!(trust.trusted.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>()),
    );
    backing.write(TAKEOVER_DONE, serde_json::json!(trust.takeover_done));
    backing.write(KEPT, serde_json::json!(trust.kept));
    backing.write(REGISTERED_DONE, serde_json::json!(trust.registered_done));
    backing.write(CODEX_DONE, serde_json::json!(trust.codex_done));
    if let Err(error) = backing.commit() {
        for (key, previous) in keys.iter().zip(previous) {
            match previous {
                Some(value) => backing.write(key, value),
                None => backing.remove(key),
            }
        }
        return Err(error);
    }
    Ok(())
}

// ---- the rules a root must satisfy, whether local or on the network ----------

/// Why a folder may not be a root, or None: a drive or share root, a system,
/// profile or application-data folder (the rules a new recordings folder must
/// pass). `resolve` follows links on disk; without it the judgement is by the
/// text alone, which never touches a network host (used at start-up).
pub(crate) fn root_problem(folder: &Path, protected: &ProtectedFolders, resolve: bool) -> Option<&'static str> {
    let key = |path: &Path| crate::local_path::comparable_text(&path.to_string_lossy());
    let resolved = |path: &Path| -> PathBuf {
        if !resolve {
            return path.to_path_buf();
        }
        let mut existing = path.to_path_buf();
        let mut rest = Vec::new();
        while !existing.exists() {
            match (existing.file_name().map(ToOwned::to_owned), existing.parent().map(Path::to_path_buf)) {
                (Some(name), Some(parent)) => {
                    rest.push(name);
                    existing = parent;
                }
                _ => break,
            }
        }
        let mut base = existing
            .canonicalize()
            .map(|canonical| crate::database::meeting_files::plain_path(&canonical))
            .unwrap_or(existing);
        for name in rest.into_iter().rev() {
            base.push(name);
        }
        base
    };
    let folder = resolved(folder);
    let text = folder.to_string_lossy();
    if text.is_empty() || text.contains('\0') || folder.is_relative() || key(&folder).split('/').any(|part| part == "..") {
        return Some("is not an absolute path");
    }
    // A drive root or a share root has no parent.
    if folder.parent().is_none() || is_system_top_level(&folder) {
        return Some("is a drive, share or system folder");
    }
    let inside = |root: &PathBuf| {
        let (a, b) = (key(&folder), key(&resolved(root)));
        a == b || a.starts_with(&format!("{}/", b.trim_end_matches('/')))
    };
    let exact = protected.exact.iter().any(|root| key(&folder) == key(&resolved(root)));
    if exact || protected.trees.iter().any(inside) {
        return Some("is a system, profile or application-data folder");
    }
    None
}

/// Makes a folder the user chose in the core's folder picker a root.
pub(crate) fn trust_folder(backing: &dyn Backing, folder: &Path, protected: &ProtectedFolders) -> Result<(), String> {
    if let Some(reason) = root_problem(folder, protected, true) {
        return Err(format!("This folder {reason} and cannot hold recordings."));
    }
    let mut trust = read(backing);
    let wanted = crate::local_path::comparable_text(&folder.to_string_lossy());
    trust.trusted.retain(|known| crate::local_path::comparable_text(&known.to_string_lossy()) != wanted);
    trust.trusted.insert(0, folder.to_path_buf());
    trust.trusted.truncate(MAX_TRUSTED);
    write(backing, &trust)
}

/// What the one-time takeover decided.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Takeover {
    pub trusted: Vec<PathBuf>,
    pub rejected: Vec<PathBuf>,
}

/// The first start of this version: the configured recordings folder and the
/// folders configured before are what the user already had, so they become
/// roots (network ones included) unless they break the folder rules. Done once;
/// recorded in the trust state. Judged by text only: nothing is looked up on
/// disk, so an unreachable share cannot stall the start.
pub(crate) fn take_over_once(
    backing: &dyn Backing,
    configured: Option<&Path>,
    previous: &[PathBuf],
    protected: &ProtectedFolders,
) -> Result<Takeover, String> {
    let mut trust = read(backing);
    let mut outcome = Takeover::default();
    if trust.takeover_done {
        return Ok(outcome);
    }
    for folder in configured.into_iter().chain(previous.iter().map(PathBuf::as_path)) {
        let wanted = crate::local_path::comparable_text(&folder.to_string_lossy());
        if folder.as_os_str().is_empty()
            || trust.trusted.iter().any(|known| crate::local_path::comparable_text(&known.to_string_lossy()) == wanted)
        {
            continue;
        }
        if root_problem(folder, protected, false).is_some() {
            outcome.rejected.push(folder.to_path_buf());
        } else if trust.trusted.len() < MAX_TRUSTED {
            trust.trusted.push(folder.to_path_buf());
            outcome.trusted.push(folder.to_path_buf());
        }
    }
    trust.takeover_done = true;
    write(backing, &trust)?;
    Ok(outcome)
}

/// Meetings recorded in folders other than the one configured at the upgrade
/// keep working: the parent folder of every folder saved for a meeting becomes
/// a root, under the same rules, by text alone, once duplicates and parents
/// already inside a root are dropped, up to `MAX_REGISTERED_PARENTS`. `None`
/// means the database was not available: nothing is recorded as done, so the
/// next start tries again. Returns how many were added and how many skipped.
pub(crate) fn take_over_registered_parents(
    backing: &dyn Backing,
    saved_folders: Option<&[String]>,
    protected: &ProtectedFolders,
) -> Result<(usize, usize), String> {
    let Some(saved_folders) = saved_folders else { return Ok((0, 0)) };
    let mut trust = read(backing);
    if trust.registered_done {
        return Ok((0, 0));
    }
    let (mut added, mut skipped) = (0usize, 0usize);
    let mut seen: Vec<String> = Vec::new();
    for saved in saved_folders {
        let Some(parent) = Path::new(saved).parent().filter(|parent| !parent.as_os_str().is_empty()) else {
            skipped += 1;
            continue;
        };
        let key = crate::local_path::comparable_text(&parent.to_string_lossy());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        let covered = trust
            .trusted
            .iter()
            .any(|root| crate::local_path::lexically_inside(&parent.to_string_lossy(), root));
        if covered {
            continue;
        }
        if added >= MAX_REGISTERED_PARENTS || root_problem(parent, protected, false).is_some() || trust.trusted.len() >= MAX_TRUSTED {
            skipped += 1;
            continue;
        }
        trust.trusted.push(parent.to_path_buf());
        added += 1;
    }
    trust.registered_done = true;
    write(backing, &trust)?;
    Ok((added, skipped))
}

/// Records that the custom Codex profile folder was marked.
pub(crate) fn mark_codex_done(backing: &dyn Backing) -> Result<(), String> {
    let mut trust = read(backing);
    trust.codex_done = true;
    write(backing, &trust)
}

/// Whether `folder` is exactly one of the trusted roots or a default one.
pub(crate) fn is_trusted_or_default(trust: &Trust, folder: &Path, defaults: &[PathBuf]) -> bool {
    let wanted = crate::local_path::comparable_text(&folder.to_string_lossy());
    trust
        .trusted
        .iter()
        .map(PathBuf::as_path)
        .chain(defaults.iter().map(PathBuf::as_path))
        .any(|known| crate::local_path::comparable_text(&known.to_string_lossy()) == wanted)
}

/// Remembers (or forgets) that a recording folder is not to be asked about again.
pub(crate) fn set_kept(backing: &dyn Backing, folder: &Path, keep: bool) -> Result<(), String> {
    let mut trust = read(backing);
    let wanted = crate::local_path::comparable_text(&folder.to_string_lossy());
    trust.kept.retain(|known| crate::local_path::comparable_text(known) != wanted);
    if keep {
        trust.kept.insert(0, folder.to_string_lossy().into_owned());
        trust.kept.truncate(MAX_KEPT);
    }
    write(backing, &trust)
}

#[cfg(test)]
fn is_kept(trust: &Trust, folder: &Path) -> bool {
    let wanted = crate::local_path::comparable_text(&folder.to_string_lossy());
    trust.kept.iter().any(|known| crate::local_path::comparable_text(known) == wanted)
}

// ---- paths from the webview, judged against the roots ------------------------

/// Whether text from the webview may be looked at on disk: a plain local path
/// (checked as before), or one whose own spelling is inside a trusted root,
/// which may be on the network. Before any filesystem call.
pub(crate) fn text_allowed(text: &str, roots: &[PathBuf]) -> bool {
    crate::local_path::is_local_path_text(text)
        || roots.iter().any(|root| crate::local_path::lexically_inside(text, root))
}

/// Whether a resolved path lies below (or is) one of the roots, once both are resolved.
pub(crate) fn canonical_inside(canonical: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| canonical.starts_with(&root))
}

/// Resolves a folder or file named by the webview or read from the library:
/// allowed text, then `canonicalize`, then still local or inside a root.
pub(crate) fn resolve_allowed(text: &str, roots: &[PathBuf]) -> Result<PathBuf, String> {
    if !text_allowed(text, roots) {
        return Err(crate::local_path::LOCAL_ONLY.into());
    }
    let resolved = Path::new(text)
        .canonicalize()
        .map_err(|_| crate::local_path::LOCAL_ONLY.to_string())?;
    if crate::local_path::is_local_canonical(&resolved) || canonical_inside(&resolved, roots) {
        Ok(resolved)
    } else {
        Err(crate::local_path::LOCAL_ONLY.into())
    }
}

// ---- a snapshot for code that has no app handle ------------------------------

static SNAPSHOT: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());

/// The roots as last computed (`recording_roots`); for helpers that cannot ask the app.
pub(crate) fn snapshot() -> Vec<PathBuf> {
    SNAPSHOT.read().unwrap_or_else(|error| error.into_inner()).clone()
}

pub(crate) fn set_snapshot(roots: Vec<PathBuf>) {
    *SNAPSHOT.write().unwrap_or_else(|error| error.into_inner()) = roots;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior_settings::test_memory::Memory;

    fn protected(base: &Path) -> ProtectedFolders {
        ProtectedFolders {
            exact: vec![base.join("profile"), base.join("appdata")],
            trees: vec![base.join("system")],
        }
    }

    #[test]
    fn a_picked_folder_becomes_a_root_and_a_protected_one_does_not() {
        let base = tempfile::tempdir().unwrap();
        for dir in ["recordings", "profile", "system/sub"] {
            std::fs::create_dir_all(base.path().join(dir)).unwrap();
        }
        let protected = protected(&base.path().canonicalize().unwrap());
        let memory = Memory::default();
        trust_folder(&memory, &base.path().join("recordings"), &protected).unwrap();
        assert_eq!(read(&memory).trusted, vec![base.path().join("recordings")]);
        for refused in ["profile", "system", "system/sub"] {
            assert!(trust_folder(&memory, &base.path().join(refused), &protected).is_err(), "{refused}");
        }
        assert_eq!(read(&memory).trusted.len(), 1);
        // Never a drive root.
        let root = base.path().ancestors().last().unwrap().to_path_buf();
        assert!(trust_folder(&memory, &root, &ProtectedFolders::default()).is_err());
    }

    #[test]
    fn the_takeover_trusts_the_earlier_folders_once_network_ones_included() {
        let base = tempfile::tempdir().unwrap();
        let share = base.path().join("share").join("recordings");
        let local = base.path().join("local");
        let system = base.path().join("system").join("x");
        let profile = base.path().join("profile");
        crate::local_path::mark_remote_for_tests(&base.path().join("share"));
        let protected = protected(base.path());
        let memory = Memory::default();
        let outcome = take_over_once(&memory, Some(&share), &[local.clone(), system.clone(), profile.clone()], &protected).unwrap();
        assert_eq!(outcome.trusted, vec![share.clone(), local.clone()]);
        assert_eq!(outcome.rejected, vec![system, profile]);
        let trust = read(&memory);
        assert!(trust.takeover_done && trust.trusted == vec![share.clone(), local]);
        // A second start changes nothing, even with other folders configured.
        let again = take_over_once(&memory, Some(&base.path().join("new")), &[], &protected).unwrap();
        assert_eq!(again, Takeover::default());
        assert_eq!(read(&memory).trusted.len(), 2);
        assert!(is_trusted_or_default(&read(&memory), &share, &[]));
    }

    #[test]
    fn the_parents_of_saved_meetings_are_taken_over_once_and_protected_ones_are_skipped() {
        let base = tempfile::tempdir().unwrap();
        let protected = protected(base.path());
        let (a, b, share) = (base.path().join("old-a"), base.path().join("old-b"), base.path().join("share").join("rec"));
        crate::local_path::mark_remote_for_tests(&base.path().join("share"));
        let saved: Vec<String> = [
            a.join("m1"), a.join("m2"), b.join("m3"), share.join("m4"),
            base.path().join("profile").join("m5"), base.path().join("system").join("sub").join("m6"),
        ]
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
        let memory = Memory::default();
        let (added, skipped) = take_over_registered_parents(&memory, Some(&saved), &protected).unwrap();
        assert_eq!((added, skipped), (3, 2), "a, b and the share; profile and system skipped");
        let trust = read(&memory);
        assert_eq!(trust.trusted, vec![a.clone(), b.clone(), share.clone()]);
        assert!(trust.registered_done);
        // Once only.
        assert_eq!(take_over_registered_parents(&memory, Some(&saved), &protected).unwrap(), (0, 0));
        assert_eq!(read(&memory).trusted.len(), 3);
    }

    #[test]
    fn without_the_database_nothing_is_recorded_as_done_and_the_next_start_tries_again() {
        let base = tempfile::tempdir().unwrap();
        let memory = Memory::default();
        let protected = protected(base.path());
        assert_eq!(take_over_registered_parents(&memory, None, &protected).unwrap(), (0, 0));
        assert!(!read(&memory).registered_done);
        let saved = vec![base.path().join("old").join("m").to_string_lossy().into_owned()];
        assert_eq!(take_over_registered_parents(&memory, Some(&saved), &protected).unwrap().0, 1);
        assert!(read(&memory).registered_done);
    }

    #[test]
    fn the_registered_parents_are_bounded_deduplicated_and_skip_what_a_root_already_covers() {
        let base = tempfile::tempdir().unwrap();
        let protected = protected(base.path());
        let memory = Memory::default();
        let root = base.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        trust_folder(&memory, &root, &ProtectedFolders::default()).unwrap();
        let mut saved: Vec<String> = (0..80)
            .map(|index| base.path().join(format!("old-{index:02}")).join("m").to_string_lossy().into_owned())
            .collect();
        saved.push(root.join("inside").join("m").to_string_lossy().into_owned());
        saved.push(base.path().join("OLD-00").join("m2").to_string_lossy().into_owned());
        let (added, skipped) = take_over_registered_parents(&memory, Some(&saved), &protected).unwrap();
        assert_eq!(added, MAX_REGISTERED_PARENTS);
        assert!(skipped >= 30, "{skipped} skipped past the cap");
        assert_eq!(read(&memory).trusted.len(), MAX_REGISTERED_PARENTS + 1);
        assert!(!read(&memory).trusted.contains(&root.join("inside")), "inside an existing root: not added");
    }

    #[test]
    fn a_failed_save_keeps_the_previous_trust() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("a")).unwrap();
        let memory = Memory::default();
        trust_folder(&memory, &base.path().join("a"), &ProtectedFolders::default()).unwrap();
        *memory.fail_saves.borrow_mut() = true;
        std::fs::create_dir_all(base.path().join("b")).unwrap();
        assert!(trust_folder(&memory, &base.path().join("b"), &ProtectedFolders::default()).is_err());
        assert_eq!(read(&memory).trusted, vec![base.path().join("a")]);
        assert!(take_over_once(&memory, Some(&base.path().join("c")), &[], &ProtectedFolders::default()).is_err());
        assert!(!read(&memory).takeover_done);
    }

    #[test]
    fn network_text_is_looked_at_only_inside_a_trusted_root() {
        let base = tempfile::tempdir().unwrap();
        let share = base.path().join("share");
        let root = share.join("recordings");
        std::fs::create_dir_all(root.join("meeting")).unwrap();
        std::fs::create_dir_all(share.join("other")).unwrap();
        crate::local_path::mark_remote_for_tests(&share);
        let roots = [root.clone()];
        let inside = root.join("meeting").to_string_lossy().into_owned();
        let outside = share.join("other").to_string_lossy().into_owned();
        let sneaky = format!("{}/../other", root.to_string_lossy());
        assert!(text_allowed(&inside, &roots));
        assert!(!text_allowed(&outside, &roots));
        assert!(!text_allowed(&sneaky, &roots));
        assert!(!text_allowed(&inside, &[]));
        assert!(resolve_allowed(&inside, &roots).is_ok());
        assert!(resolve_allowed(&outside, &roots).is_err());
        // A local path is still accepted as before.
        assert!(text_allowed(&base.path().join("elsewhere").to_string_lossy(), &roots));
    }

    #[test]
    fn kept_recordings_are_remembered_per_folder() {
        let memory = Memory::default();
        let folder = Path::new("C:/Rec/one");
        assert!(!is_kept(&read(&memory), folder));
        set_kept(&memory, folder, true).unwrap();
        assert!(is_kept(&read(&memory), Path::new(r"c:\rec\ONE\")));
        set_kept(&memory, folder, false).unwrap();
        assert!(!is_kept(&read(&memory), folder));
    }
}
