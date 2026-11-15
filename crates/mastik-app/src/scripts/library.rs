//! The library scripts: plain `.luau` files in a `scripts` folder beside the
//! preferences, so they can be kept in git and shared as files.
//!
//! A script's name is its file name less `.luau`. Names are compared without
//! case, as Windows and macOS compare file names, so `Grid` and `grid` are one
//! name. Every write is atomic (`crate::atomic`); a delete removes the file
//! outright, since the app has no helper for the OS trash.

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
    Io(String),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptError::BadName(why) => f.write_str(why),
            ScriptError::Taken(name) => write!(f, "a script called “{name}” already exists"),
            ScriptError::Missing(name) => write!(f, "no script is called “{name}”"),
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
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<LibraryScript> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let path = e.path();
                let name = script_name(&path)?;
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
    /// source. A script whose name differs only in case is the same script.
    pub fn write(&self, name: &str, source: &str) -> Result<PathBuf, ScriptError> {
        check_name(name)?;
        let path = self.find(name).unwrap_or_else(|| self.path_of(name));
        crate::atomic::write(&path, source.as_bytes())?;
        Ok(path)
    }

    /// Rename the script `from` to `to`, refusing a name another script has.
    /// A change of case alone renames the file.
    pub fn rename(&self, from: &str, to: &str) -> Result<PathBuf, ScriptError> {
        check_name(to)?;
        let old = self.existing(from)?;
        if let Some(other) = self.find(to)
            && other != old
        {
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

    /// `wanted` if no script has that name, else its stem numbered past the ones
    /// that do: `Card grid` → `Card grid 2`, `Untitled 1` → `Untitled 2`.
    pub fn unique_name(&self, wanted: &str) -> String {
        let taken: std::collections::HashSet<String> =
            self.list().iter().map(|s| s.name.to_lowercase()).collect();
        if !taken.contains(&wanted.to_lowercase()) {
            return wanted.to_owned();
        }
        let (stem, from) = match mastik_core::naming::numbered(wanted) {
            Some((stem, n)) => (stem, n.saturating_add(1)),
            None => (wanted, 2),
        };
        (from..=u32::MAX)
            .map(|n| format!("{stem} {n}"))
            .find(|c| !taken.contains(&c.to_lowercase()))
            .unwrap_or_else(|| format!("{stem} {from}"))
    }

    /// The file a script called `name` would be written to.
    fn path_of(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.{EXT}"))
    }

    /// The file of the script called `name`, compared without case.
    fn find(&self, name: &str) -> Option<PathBuf> {
        let want = name.to_lowercase();
        self.list()
            .into_iter()
            .find(|s| s.name.to_lowercase() == want)
            .map(|s| s.path)
    }

    fn existing(&self, name: &str) -> Result<PathBuf, ScriptError> {
        check_name(name)?;
        self.find(name)
            .ok_or_else(|| ScriptError::Missing(name.to_owned()))
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
    check_name(name).ok()?;
    // A definitions file (`name.d.luau`) types the API for an outside editor; it
    // is no script.
    if name.to_ascii_lowercase().ends_with(".d") {
        return None;
    }
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
}
