//! The library scripts: plain `.luau` files in a `scripts` folder beside the
//! preferences, so they can be kept in git and shared as files.
//!
//! A script's name is its file name less `.luau`. A name finds its own file
//! first, then the one file whose name differs only in case, as Windows and
//! macOS compare file names; where a case-sensitive folder holds several, the
//! name is refused rather than guessed. A file that is not UTF-8 text is never
//! listed, but its name stays taken, so nothing is written over it. Every write
//! is atomic (`crate::atomic`); a delete removes the file outright, since the
//! app has no helper for the OS trash.

use std::path::{Path, PathBuf};

/// The extension every library script carries.
pub const EXT: &str = "luau";

/// The folder's name inside the preferences folder.
const DIR_NAME: &str = "scripts";

/// One library script as read from its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryScript {
    pub name: String,
    pub path: PathBuf,
    pub source: String,
}

/// Why a library script could not be written, renamed or removed.
#[derive(Debug)]
pub enum ScriptError {
    BadName(&'static str),
    Taken(String),
    Missing(String),
    /// Several files carry the name in other cases, and none in this one.
    Ambiguous(String),
    /// The file is there but is not UTF-8 text, so it is left as it is.
    NotText(String),
    Io(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::BadName(why) => f.write_str(why),
            ScriptError::Taken(name) => write!(f, "a script called “{name}” already exists"),
            ScriptError::Missing(name) => write!(f, "no script is called “{name}”"),
            ScriptError::Ambiguous(name) => {
                write!(f, "more than one script is called “{name}” in another case")
            }
            ScriptError::NotText(name) => {
                write!(f, "“{name}” is not UTF-8 text, so it is left as it is")
            }
            ScriptError::Io(why) => f.write_str(why),
        }
    }
}

impl From<std::io::Error> for ScriptError {
    fn from(e: std::io::Error) -> Self {
        ScriptError::Io(crate::atomic::without_path(&e))
    }
}

/// The real library folder, beside the preferences; `None` where the platform
/// has no configuration folder.
pub fn folder() -> Option<PathBuf> {
    Some(crate::prefs::folder()?.join(DIR_NAME))
}

/// The library scripts in one folder. Tests point it at a scratch folder; the app
/// at [`folder`].
#[derive(Clone, Debug)]
pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The library in the real folder.
    #[cfg_attr(
        test,
        expect(
            dead_code,
            reason = "under test the editor keeps its library in a scratch folder"
        )
    )]
    pub fn open() -> Option<Self> {
        folder().map(Self::new)
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Show the folder in the OS file browser, creating it first so there is
    /// something to show.
    pub fn reveal(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
        crate::panels::show_in_file_browser(&self.dir);
    }

    /// Every script in the folder, sorted by name without case. A missing folder
    /// is an empty library; a file that cannot be read as text is left out.
    pub fn list(&self) -> Vec<LibraryScript> {
        let mut out: Vec<LibraryScript> = self
            .files()
            .into_iter()
            .filter_map(|(name, path)| {
                let source = std::fs::read_to_string(&path).ok()?;
                Some(LibraryScript { name, path, source })
            })
            .collect();
        out.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        });
        out
    }

    /// Whether a script file has exactly the name `name`, case and all.
    pub fn is_listed(&self, name: &str) -> bool {
        self.files().iter().any(|(n, _)| n == name)
    }

    /// The source of the script called `name`.
    pub fn read(&self, name: &str) -> Result<String, ScriptError> {
        let path = self.existing(name)?;
        Ok(std::fs::read_to_string(path)?)
    }

    /// Write the API's definitions ([`DEFINITIONS`]) when the folder's copy is
    /// missing or differs, so the file follows the app's version.
    pub fn write_definitions(&self, text: &str) -> Result<(), ScriptError> {
        let path = self.dir.join(DEFINITIONS);
        if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
            return Ok(());
        }
        crate::atomic::write(&path, text.as_bytes())?;
        Ok(())
    }

    /// Write `source` as the script called `name`, creating it or replacing its
    /// source. A script whose name differs only in case is the same script; a
    /// file that is not UTF-8 text is refused, never replaced. A name with no
    /// listed script is only ever created: when the system already resolves it
    /// to something (a symlink, a directory, a name it compares differently),
    /// that is refused as taken rather than written through.
    pub fn write(&self, name: &str, source: &str) -> Result<PathBuf, ScriptError> {
        check_name(name)?;
        let path = match self.find(name)? {
            Some(path) => path,
            None if self.on_disk(name) => return Err(ScriptError::Taken(name.to_owned())),
            None => self.path_of(name),
        };
        if let Ok(bytes) = std::fs::read(&path)
            && std::str::from_utf8(&bytes).is_err()
        {
            return Err(ScriptError::NotText(name.to_owned()));
        }
        crate::atomic::write(&path, source.as_bytes())?;
        Ok(path)
    }

    /// Rename the script `from` to `to`, refusing a name another script file
    /// has in any case. A change of case alone renames the file.
    pub fn rename(&self, from: &str, to: &str) -> Result<PathBuf, ScriptError> {
        check_name(to)?;
        let old = self.existing(from)?;
        let want = to.to_lowercase();
        if self
            .files()
            .iter()
            .any(|(n, p)| n.to_lowercase() == want && *p != old)
        {
            return Err(ScriptError::Taken(to.to_owned()));
        }
        // A change of case alone resolves `to` to `old` itself where names compare
        // without case; anything else already there was not listed.
        let recased = script_name(&old).is_some_and(|n| n.to_lowercase() == want);
        if !recased && self.on_disk(to) {
            return Err(ScriptError::Taken(to.to_owned()));
        }
        let new = self.path_of(to);
        std::fs::rename(&old, &new)?;
        Ok(new)
    }

    /// Copy the script `name` under the next free name — `Card grid` to
    /// `Card grid 2` — and answer that name.
    pub fn duplicate(&self, name: &str) -> Result<String, ScriptError> {
        let source = self.read(name)?;
        let copy = self.unique_name(name);
        self.write(&copy, &source)?;
        Ok(copy)
    }

    /// Remove the script `name`'s file.
    pub fn delete(&self, name: &str) -> Result<(), ScriptError> {
        let path = self.existing(name)?;
        std::fs::remove_file(&path)?;
        crate::atomic::forget(&path);
        Ok(())
    }

    /// `wanted` if no script file has that name, else its stem numbered past the
    /// ones that do: `Card grid` → `Card grid 2`, `Untitled 1` → `Untitled 2`.
    pub fn unique_name(&self, wanted: &str) -> String {
        let taken: std::collections::HashSet<String> = self
            .files()
            .into_iter()
            .map(|(name, _)| name.to_lowercase())
            .collect();
        if !taken.contains(&wanted.to_lowercase()) && !self.on_disk(wanted) {
            return wanted.to_owned();
        }
        let (stem, from) = match mastik_core::naming::numbered(wanted) {
            Some((stem, n)) => (stem, n.saturating_add(1)),
            None => (wanted, 2),
        };
        (from..=u32::MAX)
            .map(|n| format!("{stem} {n}"))
            .find(|c| !taken.contains(&c.to_lowercase()) && !self.on_disk(c))
            .unwrap_or_else(|| format!("{stem} {from}"))
    }

    /// The file a script called `name` would be written to.
    fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{EXT}"))
    }

    /// Whether the system resolves `name`'s file to anything, listed or not,
    /// without following a symlink.
    fn on_disk(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.path_of(name)).is_ok()
    }

    /// Every script file in the folder by its script name, whether or not its
    /// text reads as UTF-8, unsorted.
    fn files(&self) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let path = e.path();
                Some((script_name(&path)?, path))
            })
            .collect()
    }

    /// The file of the script called `name` ([`lookup`]).
    fn find(&self, name: &str) -> Result<Option<PathBuf>, ScriptError> {
        lookup(&self.files(), name)
    }

    fn existing(&self, name: &str) -> Result<PathBuf, ScriptError> {
        check_name(name)?;
        self.find(name)?
            .ok_or_else(|| ScriptError::Missing(name.to_owned()))
    }
}

/// The file among `files` the script `name` is: the one with exactly that name,
/// else the only one whose name differs in case alone. Several of those and
/// none exact is refused, so a write never lands on a file nobody named.
fn lookup(files: &[(String, PathBuf)], name: &str) -> Result<Option<PathBuf>, ScriptError> {
    if let Some((_, path)) = files.iter().find(|(n, _)| n == name) {
        return Ok(Some(path.clone()));
    }
    let want = name.to_lowercase();
    let mut folded = files.iter().filter(|(n, _)| n.to_lowercase() == want);
    match (folded.next(), folded.next()) {
        (None, _) => Ok(None),
        (Some((_, path)), None) => Ok(Some(path.clone())),
        (Some(_), Some(_)) => Err(ScriptError::Ambiguous(name.to_owned())),
    }
}

/// The script name a file in the folder carries: its stem, when its extension is
/// [`EXT`] and the stem is a name [`check_name`] accepts. An atomic write's temp
/// file ends otherwise and is never listed.
fn script_name(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    if !ext.eq_ignore_ascii_case(EXT) {
        return None;
    }
    let name = path.file_stem()?.to_str()?;
    // A definitions file (`name.d.luau`) types the API for an outside editor;
    // `check_name` refuses its name, so it is no script.
    check_name(name).ok()?;
    Some(name.to_owned())
}

/// The file in the scripts folder that gives an outside editor the API's types
/// through luau-lsp, written from `mastik_vm::definitions`.
pub const DEFINITIONS: &str = "mastik.d.luau";

/// Refuse a name that is not one plain file name on every platform: empty,
/// padded, a path separator, a character Windows refuses, a trailing dot, or a
/// Windows device name.
pub fn check_name(name: &str) -> Result<(), ScriptError> {
    if name.trim().is_empty() {
        return Err(ScriptError::BadName("a script needs a name"));
    }
    if name.trim() != name {
        return Err(ScriptError::BadName(
            "a script's name cannot start or end with a space",
        ));
    }
    if name.contains(['/', '\\']) {
        return Err(ScriptError::BadName(
            "a script's name cannot hold a / or a \\",
        ));
    }
    if name.contains([':', '*', '?', '"', '<', '>', '|']) || name.chars().any(char::is_control) {
        return Err(ScriptError::BadName(
            "a script's name cannot hold : * ? \" < > |",
        ));
    }
    if name.ends_with('.') {
        return Err(ScriptError::BadName(
            "a script's name cannot end with a dot",
        ));
    }
    // `name.d.luau` is a definitions file, which the library never lists.
    if name.to_ascii_lowercase().ends_with(".d") {
        return Err(ScriptError::BadName("a script's name cannot end with .d"));
    }
    let base = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit());
    if device {
        return Err(ScriptError::BadName("that name is reserved by Windows"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> Library {
        let dir = std::env::temp_dir().join(format!("mastik-scripts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Library::new(dir)
    }

    fn names(lib: &Library) -> Vec<String> {
        lib.list().into_iter().map(|s| s.name).collect()
    }

    /// The real folder sits beside the preferences file.
    #[test]
    fn the_folder_is_beside_the_preferences() {
        if let (Some(scripts), Some(prefs)) = (folder(), crate::prefs::folder()) {
            assert_eq!(scripts.parent(), Some(prefs.as_path()));
        }
    }

    /// A missing folder lists nothing; written scripts list sorted by name
    /// without case, each with its path and source, and other files are not
    /// scripts.
    /// flip: `script_name` without its extension check → fails at the listing.
    #[test]
    fn scripts_list_by_name() {
        let lib = scratch("list");
        assert!(lib.list().is_empty(), "no folder, no scripts");
        lib.write("card grid", "print(1)").unwrap();
        lib.write("Align", "print(2)").unwrap();
        lib.write("Zeta", "").unwrap();
        std::fs::write(lib.dir().join("notes.txt"), "not a script").unwrap();
        std::fs::write(lib.dir().join(" padded.luau"), "").unwrap();
        assert_eq!(names(&lib), ["Align", "card grid", "Zeta"]);
        let first = &lib.list()[0];
        assert_eq!(first.path, lib.dir().join("Align.luau"));
        assert_eq!(first.source, "print(2)");
        assert_eq!(
            lib.read("ALIGN").unwrap(),
            "print(2)",
            "by name without case"
        );
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// Writing an existing name, in any case, replaces that script's source and
    /// keeps its file.
    #[test]
    fn writing_a_name_again_replaces_the_source() {
        let lib = scratch("rewrite");
        let path = lib.write("Grid", "a").unwrap();
        assert_eq!(lib.write("grid", "b").unwrap(), path);
        assert_eq!(names(&lib), ["Grid"]);
        assert_eq!(lib.read("Grid").unwrap(), "b");
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// A name with a path separator, a Windows-refused character, padding, a
    /// trailing dot or a device name is refused before anything is written.
    /// flip: `check_name` without its separator test → fails at `"../escape"`.
    #[test]
    fn a_name_that_is_not_one_file_is_refused() {
        let lib = scratch("names");
        for bad in [
            "",
            "  ",
            "../escape",
            "a/b",
            r"a\b",
            "a:b",
            "what?",
            " pad",
            "dot.",
            "CON",
            "com1",
            "nul.txt",
            "mastik.d",
            "Types.D",
        ] {
            assert!(
                matches!(lib.write(bad, "x"), Err(ScriptError::BadName(_))),
                "{bad:?} is refused"
            );
        }
        assert!(!lib.dir().exists(), "nothing was written");
        assert!(check_name("Card grid 2").is_ok());
        assert!(check_name("v1.2 tidy").is_ok());
        assert!(check_name("Console").is_ok());
    }

    /// A rename moves the file, refuses another script's name, and may change the
    /// case alone; a missing script is said to be missing.
    #[test]
    fn a_rename_moves_the_file_and_refuses_a_taken_name() {
        let lib = scratch("rename");
        lib.write("Grid", "g").unwrap();
        lib.write("Tidy", "t").unwrap();
        assert!(matches!(
            lib.rename("Grid", "tidy"),
            Err(ScriptError::Taken(_))
        ));
        let path = lib.rename("Grid", "Card grid").unwrap();
        assert_eq!(path, lib.dir().join("Card grid.luau"));
        assert_eq!(names(&lib), ["Card grid", "Tidy"]);
        lib.rename("Tidy", "TIDY").unwrap();
        assert_eq!(names(&lib), ["Card grid", "TIDY"]);
        assert_eq!(lib.read("TIDY").unwrap(), "t");
        assert!(matches!(
            lib.rename("Nope", "X"),
            Err(ScriptError::Missing(_))
        ));
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// A duplicate takes the next free number and the same source; a delete
    /// removes the file.
    #[test]
    fn duplicate_numbers_and_delete_removes() {
        let lib = scratch("dup");
        lib.write("Card grid", "src").unwrap();
        assert_eq!(lib.duplicate("Card grid").unwrap(), "Card grid 2");
        assert_eq!(lib.duplicate("card grid").unwrap(), "card grid 3");
        assert_eq!(lib.read("Card grid 2").unwrap(), "src");
        lib.delete("Card grid 2").unwrap();
        assert!(!lib.dir().join("Card grid 2.luau").exists());
        assert!(matches!(
            lib.delete("Card grid 2"),
            Err(ScriptError::Missing(_))
        ));
        assert_eq!(names(&lib), ["Card grid", "card grid 3"]);
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// A taken name is numbered past itself, compared without case.
    /// flip: `unique_name`'s candidates compared with case → fails at `Untitled 3`.
    #[test]
    fn a_taken_name_is_numbered_without_case() {
        let lib = scratch("untitled");
        assert_eq!(lib.unique_name("Untitled 1"), "Untitled 1");
        lib.write("untitled 1", "").unwrap();
        assert_eq!(lib.unique_name("Untitled 1"), "Untitled 2");
        lib.write("UNTITLED 2", "").unwrap();
        assert_eq!(lib.unique_name("Untitled 1"), "Untitled 3");
        assert_eq!(lib.unique_name("Fresh"), "Fresh");
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// A name finds its own file before one in another case, the only one in
    /// another case otherwise, and refuses to choose among several.
    /// flip: `lookup` taking the first match without case → fails at "its own".
    #[test]
    fn a_name_finds_its_own_file_first() {
        let files: Vec<(String, PathBuf)> = ["Grid", "grid", "Tidy"]
            .iter()
            .map(|n| (n.to_string(), PathBuf::from(format!("{n}.luau"))))
            .collect();
        let found = |name| match lookup(&files, name) {
            Ok(p) => Ok(p.map(|p| p.display().to_string())),
            Err(e) => Err(e.to_string()),
        };
        assert_eq!(found("grid"), Ok(Some("grid.luau".into())), "its own");
        assert_eq!(found("Grid"), Ok(Some("Grid.luau".into())));
        assert_eq!(found("TIDY"), Ok(Some("Tidy.luau".into())), "one in a case");
        assert_eq!(found("Nope"), Ok(None));
        assert!(found("GRID").is_err(), "two in other cases");
    }

    /// A name the system resolves to something the library does not list (here
    /// a directory) is taken: a write and a rename onto it are refused, and a
    /// new name is numbered past it.
    /// flip: `Library::write` writing to `path_of` when `find` misses → fails at
    /// "a write is refused".
    #[test]
    fn an_unlisted_name_on_disk_is_never_written_through() {
        let lib = scratch("unlisted");
        std::fs::create_dir_all(lib.dir().join("Grid.luau")).unwrap();
        lib.write("Tidy", "t").unwrap();
        assert_eq!(names(&lib), ["Tidy"], "fixture: it is not listed");
        assert!(
            matches!(lib.write("Grid", "x"), Err(ScriptError::Taken(_))),
            "a write is refused"
        );
        assert!(matches!(
            lib.rename("Tidy", "Grid"),
            Err(ScriptError::Taken(_))
        ));
        assert_eq!(lib.unique_name("Grid"), "Grid 2");
        assert!(lib.dir().join("Grid.luau").is_dir());
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// A script linked into the folder is never written through: the file the
    /// link points at keeps its text.
    /// flip: `Library::write` writing to `path_of` when `find` misses → fails at
    /// "the linked file keeps its text".
    #[cfg(unix)]
    #[test]
    fn a_linked_script_is_never_written_through() {
        let lib = scratch("link");
        std::fs::create_dir_all(lib.dir()).unwrap();
        let outside = lib.dir().with_extension("outside.luau");
        std::fs::write(&outside, "a").unwrap();
        std::os::unix::fs::symlink(&outside, lib.dir().join("Grid.luau")).unwrap();
        assert!(matches!(lib.write("Grid", "b"), Err(ScriptError::Taken(_))));
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "a",
            "the linked file keeps its text"
        );
        let _ = std::fs::remove_dir_all(lib.dir());
        let _ = std::fs::remove_file(&outside);
    }

    /// A script file that is not UTF-8 text is not listed, but its name stays
    /// taken: a write to it is refused, a rename onto it too, and a new name
    /// is numbered past it.
    /// flip: `unique_name` reading taken names from `list` → fails at "numbered
    /// past it".
    #[test]
    fn a_file_that_is_not_text_is_never_written_over() {
        let lib = scratch("latin1");
        std::fs::create_dir_all(lib.dir()).unwrap();
        let path = lib.dir().join("Café.luau");
        let bytes = b"-- caf\xe9\n".to_vec();
        std::fs::write(&path, &bytes).unwrap();
        lib.write("Tidy", "t").unwrap();
        assert_eq!(names(&lib), ["Tidy"], "fixture: it is not listed");
        assert_eq!(lib.unique_name("Café"), "Café 2", "numbered past it");
        assert!(matches!(
            lib.write("Café", "x"),
            Err(ScriptError::NotText(_))
        ));
        assert!(matches!(
            lib.rename("Tidy", "café"),
            Err(ScriptError::Taken(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "its bytes are intact");
        let _ = std::fs::remove_dir_all(lib.dir());
    }

    /// In a case-sensitive folder, `grid` and `Grid` are two files and each
    /// name writes, renames and deletes its own.
    /// flip: `lookup` taking the first match without case → fails at "Grid
    /// keeps its text".
    #[cfg(target_os = "linux")]
    #[test]
    fn two_names_in_two_cases_keep_their_own_files() {
        let lib = scratch("cases");
        lib.write("Grid", "a").unwrap();
        std::fs::write(lib.dir().join("grid.luau"), "b").unwrap();
        assert_eq!(names(&lib), ["Grid", "grid"], "fixture: both are listed");
        lib.write("grid", "c").unwrap();
        assert_eq!(lib.read("Grid").unwrap(), "a", "Grid keeps its text");
        assert_eq!(lib.read("grid").unwrap(), "c");
        assert!(matches!(
            lib.write("GRID", "d"),
            Err(ScriptError::Ambiguous(_))
        ));
        assert!(matches!(
            lib.rename("grid", "GRID"),
            Err(ScriptError::Taken(_))
        ));
        lib.delete("grid").unwrap();
        assert_eq!(names(&lib), ["Grid"], "the delete took its own file");
        let _ = std::fs::remove_dir_all(lib.dir());
    }
}
