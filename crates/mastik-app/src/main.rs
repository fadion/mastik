//! mastik — the `mastik` binary (§9).
//!
//! Subcommands — fenced, because rustdoc reads a bare usage line as markup
//! (`[gui]` as an intra-doc link, `<file>` as an unclosed HTML tag):
//!
//! ```text
//! mastik [gui]                    launch the GUI shell (default)
//! mastik --help | help <command> | --version
//!                                the commands, what one takes, the version
//! mastik export <file> [--svg|--pdf|--png|--jpeg|--webp|--json|--html|--react|--vue
//!            |--penpot|--sketch] [-o out]
//!            [--outlines] [--frames] [--scale <n> | --width <px> | --height <px>]
//!            [--quality <1-100>] [--page <name|n>] [--layer <name|id>]...
//!                                render a page, or some of its layers, with no
//!                                GPU and no window, by default the first page
//!                                that is not a draft; the three sizes are the
//!                                rasters' only, and one at a time;
//!                                --outlines sets an SVG's or a PDF's text as
//!                                paths; --pdf --frames writes a page per
//!                                top-level frame;
//!                                --html writes a zip of HTML and CSS,
//!                                --react and --vue a zip of components;
//!                                --json describes every page;
//!                                --penpot and --sketch write every page as a
//!                                Penpot or Sketch file
//! mastik export <file> --all [--scale-override <2x|512w|512h>] [--folders]
//!            [--page <name|n>] [-o dir]
//!                                run the document's own export settings (§7) on
//!                                every page that is not a draft, or on one page,
//!                                optionally forcing every row to one size
//! mastik render <file> --component <name> --data <rows.csv> [--name-by <column>]
//!            [--png|--svg] [--scale <n> | --width <px> | --height <px>] [-o dir]
//!                                the component once per CSV row, each column
//!                                feeding the property of its name (render_cli)
//! mastik render <file> --all-modes [--page <name|n>] [--png|--svg] [size] [-o dir]
//!                                every visible top-level frame once per mode
//! mastik diff <a> <b> [--json]    what changed from document a to b, by layer;
//!                                exit 0 when nothing did, 1 when something did,
//!                                2 when either cannot be read
//! mastik check contrast|code|review|health|layout <file>... [--page <name|n>] [--json]
//!                                contrast: every text layer (and small shape,
//!                                --non-text) against what is drawn behind it,
//!                                --aaa or --apca; code: the page's HTML export
//!                                held to the canvas in a headless browser;
//!                                review: the MCP's `review`; health: System
//!                                health, --min <percent>; layout: the MCP's
//!                                `check_layout`. Each document is reported in
//!                                turn; exit 0 all pass, 1 one fails, 2 one is
//!                                unreadable (or no browser)
//! mastik merge <base> <ours> <theirs> [-o out]
//!                                three-way merge, written over ours unless -o;
//!                                exit 0 clean, 1 with conflicts (ours kept at
//!                                each), 2 when a file cannot be read; git's
//!                                merge driver: `mastik merge %O %A %B`
//! mastik tokens export <file> [--mode <name>] [-o out.json]
//!                                variables and styles as DTCG tokens.json,
//!                                the default mode's values unless --mode
//! mastik tokens import <file> <tokens.json> [--collection <name>] [--removals] [-o out]
//!                                apply a tokens.json once, written over the
//!                                document unless -o
//! mastik serve                    headless: documents behind the MCP endpoint (§8.2)
//! mastik mcp-proxy [--client <key>] [--document <path>]
//!                                the MCP server an agent harness configures
//!                                (§8.1); a key (else MASTIK_MCP_CLIENT) keeps
//!                                the client's document and revisions across
//!                                its processes, a document is used from the start
//! ```
//!
//! `mastik reader` is left out above: it is the app's own child, reading another
//! tool's file off standard input (`reader`).
//!
//! `export` runs on `mastik-core` and the CPU renderer alone, so it works with no
//! display or graphics driver, and so does `serve`.
//!
//! The rustdoc gate and the lint it allows are explained in `mastik_core`'s crate doc.
//!
//! A GUI-subsystem binary on Windows in a release build, so a launch from the
//! Start menu or Explorer opens no console. `mastik export …` typed at a prompt is
//! reached through the console twin `mastik-cli` (shipped as `mastik.com`, which
//! `PATHEXT` prefers over the `.exe`), which runs this binary with its own
//! standard handles. A debug build keeps the console, where `eprintln!` is read.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]
// `Duration - Duration` panics on underflow in release too. Production only: the
// tests back-date an `Instant` by subtraction on purpose.
#![cfg_attr(not(test), deny(clippy::unchecked_time_subtraction))]

mod again;
mod app;
mod assets;
mod assistant;
mod atomic;
mod batch_rename;
mod blend;
mod canvas;
mod chart_data;
mod chart_import;
mod cheatsheet;
mod check_cli;
mod chrome;
mod clean_up_box;
mod cleanup;
mod colour_review;
mod colour_text;
mod command;
mod compare;
mod contrast;
#[cfg(test)]
mod control_audit;
mod cursor;
mod distort;
mod expr;
mod eyedropper;
#[cfg(windows)]
mod file_type;
mod fill_box;
mod find;
mod fonts;
mod gradient;
mod grid;
mod import;
mod input;
mod inspect_cli;
mod instances;
mod job;
mod keychain;
mod kit_cli;
mod kits;
mod knife;
mod lasso;
mod list_data;
mod loose_spacing;
mod shape_handles;
mod system_health;
// No blanket `dead_code` allow here: the plain build is the gate that catches an
// unwired item. `store::versions` (test-only callers) carries its own allow.
mod library;
mod logging;
mod machine_dir;
mod mcp;
mod measure;
mod menu;
mod meter;
mod minimap;
mod mirror;
mod navigate;
mod one_window;
#[cfg(target_os = "macos")]
mod open_event;
mod opens;
mod pages;
mod palette;
mod panels;
mod path_bar;
mod pencil;
mod phone;
mod pictures;
mod pixel_fit;
mod player;
mod prefs;
mod present;
mod preview;
mod private;
mod prototype_export;
#[cfg(target_os = "macos")]
mod quit;
mod rail;
mod reader;
mod render_cli;
mod repeat;
mod rulers;
mod run_cli;
mod scripts;
mod session;
mod settings;
mod shape_builder;
mod size_preview;
mod slice;
mod snap;
mod spacing;
mod spell;
mod tabs;
mod theme;
mod thumbs;
mod tokens_import;
mod tools;
mod trace;
#[cfg(target_os = "macos")]
mod traffic_lights;
mod ui;
mod update;
mod version_diffs;
mod versions;
mod view;
mod vision;
mod warp;

use mastik_core::{ExportScale, ExportText};

#[derive(Debug, PartialEq)]
enum Command {
    /// The window, opening `open` when the command line named a document
    /// (`mastik file.mastik`, which is also what a double-click runs).
    Gui { open: Option<std::path::PathBuf> },
    /// Host documents with no window, behind the MCP endpoint (§8.2).
    Serve,
    /// `--help`, `help <command>`, a command's `--help` or `--version`: the text
    /// to print on stdout, exiting 0.
    Help(String),
    /// A one-off export of a page, or of some of its layers, in one format.
    Export(OneOff),
    /// Run the document's own export settings (§7) — every layer that carries any,
    /// at the formats and sizes the file says, into a folder.
    ///
    /// Goes through the same `mastik_export::plan` as the panel's button, so a build
    /// script gets the bytes a designer would get by clicking.
    ExportAll {
        file: String,
        out: Option<String>,
        folders: bool,
        /// `--scale-override`: force every spec in the file to this size. The
        /// document still decides which layers export and at what formats.
        scale_override: Option<ExportScale>,
        /// `--page`: run only this page's settings, draft or not.
        page: Option<String>,
    },
    /// `mastik render`: a component once per CSV row, or every frame once per mode.
    Render(render_cli::Render),
    /// Compare two documents (`mastik_core::diff`), `a` the earlier.
    Diff { a: String, b: String, json: bool },
    /// `mastik check`: contrast, code, review, health or layout, over one
    /// document or several.
    Check(check_cli::Check),
    /// Three-way merge of a document (`mastik_core::merge`), the result written to
    /// `out`, by default over `ours` as git's merge driver expects.
    Merge {
        base: String,
        ours: String,
        theirs: String,
        out: Option<String>,
    },
    /// `mastik run`: a Luau script or a JSON batch, as the MCP runs them, applied
    /// to a document or a new one and written to `-o`.
    Run(run_cli::Run),
    /// `mastik inspect`: one of the MCP's reads of a document, as JSON.
    Inspect(inspect_cli::Inspect),
    /// `mastik kit update`: the kits' waiting updates taken into each document.
    KitUpdate(kit_cli::Update),
    /// Write the document's variables and styles as DTCG `tokens.json`
    /// (`mastik_export::dtcg`), `mode` naming the mode whose values are `$value`.
    TokensExport {
        file: String,
        mode: Option<String>,
        out: Option<String>,
    },
    /// Apply a DTCG `tokens.json` to the document once, every row *Import
    /// tokens* starts ticked, written over `file` unless `out` names another.
    TokensImport {
        file: String,
        tokens: String,
        /// Where the file's tokens that name no collection land; the first else.
        collection: Option<String>,
        /// Also remove what the document has and the file does not.
        removals: bool,
        out: Option<String>,
    },
    /// `mastik mcp-proxy`, with the client key the host keeps its state under,
    /// a document to use from the first call, and whether to list every tool
    /// rather than the core set.
    McpProxy {
        client: Option<String>,
        document: Option<String>,
        all_tools: bool,
    },
}

/// `mastik export <file> [format] …`: one file of one format.
#[derive(Debug, PartialEq)]
struct OneOff {
    file: String,
    format: ExportFormat,
    out: Option<String>,
    /// How big the raster comes out — `--scale 2`, `--width 512`, `--height 512`.
    ///
    /// Not an `Option`: the parser refuses all three for the formats that cannot
    /// use one, so `Times(1.0)` here means "unscaled", never "unasked".
    scale: ExportScale,
    /// `--page`: a page's number from 1, or its exact name.
    page: Option<String>,
    /// `--outlines`: an SVG's or a PDF's text as glyph outlines.
    text: ExportText,
    /// `--sizes 16,32,48`: an icon file's sizes, empty for its format's set.
    sizes: Vec<u32>,
    /// `--layer`, each a layer's id or exact name; empty for the whole page.
    layers: Vec<String>,
    /// `--quality`: a JPEG's, 1 to 100; the export row's default without one.
    quality: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFormat {
    Svg,
    Pdf,
    /// `--pdf --frames`: one PDF, a page per visible top-level frame.
    PdfFrames,
    Png,
    Jpeg,
    /// Lossless, as the export row's WebP is.
    Webp,
    /// A Windows icon of the page, one PNG entry per `--sizes` size.
    Ico,
    /// A macOS icon of the page, as `Ico` is.
    Icns,
    Json,
    /// Code export's plain HTML/CSS, zipped.
    Html,
    /// Code export's React components, zipped.
    React,
    /// Code export's Vue components, zipped.
    Vue,
    /// The whole document as a `.penpot` file.
    Penpot,
    /// The whole document as a `.sketch` file.
    Sketch,
    /// The prototype as one HTML file, the pages its export settings' flows
    /// reach (`mastik_export::prototype`).
    Prototype,
    /// Every flow's frames with their connectors and interactions in words, as
    /// one PDF for review (`mastik_export::flow_map`).
    FlowMap,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            ExportFormat::Svg => "svg",
            ExportFormat::Pdf | ExportFormat::PdfFrames | ExportFormat::FlowMap => "pdf",
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Webp => "webp",
            ExportFormat::Ico => "ico",
            ExportFormat::Icns => "icns",
            ExportFormat::Json => "json",
            ExportFormat::Html | ExportFormat::React | ExportFormat::Vue => "zip",
            ExportFormat::Penpot => "penpot",
            ExportFormat::Sketch => "sketch",
            ExportFormat::Prototype => "html",
        }
    }

    /// Whether the format writes every page, so `--page` has nothing to choose.
    fn whole_document(self) -> bool {
        matches!(
            self,
            ExportFormat::Json
                | ExportFormat::Penpot
                | ExportFormat::Sketch
                | ExportFormat::Prototype
                | ExportFormat::FlowMap
        )
    }

    /// Whether the format is pixels, so a size flag applies.
    fn raster(self) -> bool {
        matches!(
            self,
            ExportFormat::Png | ExportFormat::Jpeg | ExportFormat::Webp
        )
    }

    /// The flag that asks for the format, for refusals to name.
    fn flag(self) -> &'static str {
        match self {
            ExportFormat::Svg => "--svg",
            ExportFormat::Pdf => "--pdf",
            ExportFormat::PdfFrames => "--pdf --frames",
            ExportFormat::Png => "--png",
            ExportFormat::Jpeg => "--jpeg",
            ExportFormat::Webp => "--webp",
            ExportFormat::Ico => "--ico",
            ExportFormat::Icns => "--icns",
            ExportFormat::Json => "--json",
            ExportFormat::Html => "--html",
            ExportFormat::React => "--react",
            ExportFormat::Vue => "--vue",
            ExportFormat::Penpot => "--penpot",
            ExportFormat::Sketch => "--sketch",
            ExportFormat::Prototype => "--prototype",
            ExportFormat::FlowMap => "--flow-map",
        }
    }
}

/// Whether `args` are a launch Velopack's startup hook must see: the GUI's
/// (nothing, `gui`, or a document to open) and Velopack's own `--veloapp-*`
/// re-invocations — an allowlist on the first argument, since the re-invocations
/// carry arguments too.
///
/// It lists what reaches the hook, not what avoids it, so `mastik --version` never
/// reaches it, nor does a mistyped `mastik exprot …`, which `parse` refuses with
/// exit 2.
fn hook_runs(args: &[String]) -> bool {
    match args.first() {
        None => true,
        Some(first) => {
            first == "gui"
                || first.to_ascii_lowercase().starts_with("--veloapp-")
                || names_a_document(first)
        }
    }
}

/// Whether a first argument is a document to open rather than a subcommand: a
/// `.mastik` name, or any file that exists. Anything else stays "unknown command",
/// so a typo of `export` is refused, not opened.
fn names_a_document(arg: &str) -> bool {
    let path = std::path::Path::new(arg);
    !arg.starts_with('-')
        && (path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("mastik"))
            || path.is_file())
}

/// The command line as strings, or the refusal for one that is not Unicode.
///
/// Not `std::env::args`, which panics on such an argument (a Latin-1-named file
/// on Linux): it is refused with `parse`'s exit 2 and a sentence instead. Taking
/// the path through would mean `OsString` all the way down the export path.
fn unicode_args(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Vec<String>, String> {
    args.map(|a| {
        a.into_string().map_err(|bad| {
            format!(
                "mastik: an argument is not valid Unicode: {}",
                bad.to_string_lossy()
            )
        })
    })
    .collect()
}

const DIFF_USAGE: &str = "usage: mastik diff <a.mastik> <b.mastik> [--json]";
const MERGE_USAGE: &str =
    "usage: mastik merge <base.mastik> <ours.mastik> <theirs.mastik> [-o <out.mastik>]";
const MCP_PROXY_USAGE: &str =
    "usage: mastik mcp-proxy [--client <key>] [--document <path>] [--tools core|all]";
const SERVE_USAGE: &str = "usage: mastik serve (an agent picks documents with use_document)";
const GUI_USAGE: &str = "usage: mastik [gui] [<file.mastik>]";

/// `mastik --help`: what the binary is and every command it takes, in the order
/// a build script is likeliest to want them.
const OVERVIEW: &str =
    "Mastik, a 2D design tool. With no command, or a .mastik file, it opens the window.

usage: mastik [<file.mastik>]
       mastik <command> [arguments]

commands:
  export     a page or some layers to a file, or the document's own export settings
  render     a component once per CSV row, or every frame once per mode
  check      contrast, code, review, health or layout checks, for CI
  inspect    a document's outline, context, components, variables or styles, as JSON
  diff       what changed from one document to another
  merge      a three-way merge, as git's merge driver
  tokens     DTCG design tokens out of a document, or into one
  kit        the kits' waiting updates, taken into documents
  run        a Luau script or a JSON batch, applied to a document
  serve      documents behind the MCP endpoint, with no window
  mcp-proxy  the MCP server an agent harness configures
  gui        the window

`mastik help <command>` shows what a command takes; `mastik --version` the version.";

/// The commands `OVERVIEW` lists, each with what it takes.
const COMMANDS: [(&str, &str); 12] = [
    ("export", EXPORT_USAGE),
    ("render", render_cli::USAGE),
    ("check", check_cli::USAGE),
    ("inspect", inspect_cli::USAGE),
    ("diff", DIFF_USAGE),
    ("merge", MERGE_USAGE),
    ("tokens", TOKENS_USAGE),
    ("kit", kit_cli::USAGE),
    ("run", run_cli::USAGE),
    ("serve", SERVE_USAGE),
    ("mcp-proxy", MCP_PROXY_USAGE),
    ("gui", GUI_USAGE),
];

/// What a command takes, `None` for one that does not exist.
fn usage_of(command: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .find(|(name, _)| *name == command)
        .map(|(_, usage)| *usage)
}

fn parse(args: &[String]) -> Result<Command, String> {
    let help = |a: &String| a == "--help" || a == "-h";
    // `mastik <command> … --help` anywhere after a command shows that command.
    if let Some(usage) = args.first().and_then(|c| usage_of(c))
        && args[1..].iter().any(help)
    {
        return Ok(Command::Help(usage.to_string()));
    }
    match args.first().map(String::as_str) {
        // `--help export` is `help export`.
        Some("help" | "--help" | "-h") => match args.get(1..) {
            Some([]) | None => Ok(Command::Help(OVERVIEW.to_string())),
            Some([command]) => usage_of(command)
                .map(|u| Command::Help(u.to_string()))
                .ok_or_else(|| format!("no command {command}; `mastik --help` lists them")),
            Some(_) => Err("usage: mastik help <command>".to_string()),
        },
        Some("--version" | "-V" | "version") if args.len() == 1 => Ok(Command::Help(format!(
            "mastik {}",
            env!("CARGO_PKG_VERSION")
        ))),
        None => Ok(Command::Gui { open: None }),
        Some("gui") => match &args[1..] {
            [] => Ok(Command::Gui { open: None }),
            [file] if names_a_document(file) => open_in_window(file),
            _ => Err(GUI_USAGE.to_string()),
        },
        Some("serve") if args.len() == 1 => Ok(Command::Serve),
        Some("serve") => Err(SERVE_USAGE.to_string()),
        Some("export") => parse_export(&args[1..]),
        Some("render") => render_cli::parse(&args[1..]).map(Command::Render),
        Some("diff") => parse_diff(&args[1..]),
        Some("tokens") => parse_tokens(&args[1..]),
        Some("kit") => kit_cli::parse(&args[1..]).map(Command::KitUpdate),
        Some("inspect") => inspect_cli::parse(&args[1..]).map(Command::Inspect),
        Some("merge") => parse_merge(&args[1..]),
        Some("check") => check_cli::parse(&args[1..]).map(Command::Check),
        Some("run") => run_cli::parse(&args[1..]).map(Command::Run),
        Some("mcp-proxy") => parse_mcp_proxy(&args[1..]),
        Some(file) if names_a_document(file) => match args.get(1) {
            Some(extra) if help(extra) => Ok(Command::Help(GUI_USAGE.to_string())),
            Some(extra) => Err(format!("mastik {file}: unexpected argument {extra}")),
            None => open_in_window(file),
        },
        Some(other) => Err(format!(
            "unknown command: {other}; `mastik --help` lists them"
        )),
    }
}

/// The window, opening `file`. Absolute, so the session's save path and
/// `prefs.last_document` survive a different working directory next launch.
fn open_in_window(file: &str) -> Result<Command, String> {
    Ok(Command::Gui {
        open: Some(std::path::absolute(file).map_err(|e| format!("mastik {file}: {e}"))?),
    })
}

fn parse_diff(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = DIFF_USAGE;
    let mut files = Vec::new();
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => files.push(other.to_string()),
        }
    }
    match <[String; 2]>::try_from(files) {
        Ok([a, b]) => Ok(Command::Diff { a, b, json }),
        Err(files) => Err(format!(
            "diff takes two documents; got {}\n{USAGE}",
            files.len()
        )),
    }
}

/// `mastik mcp-proxy [--client <key>] [--document <path>]`; the key falls back to
/// `MASTIK_MCP_CLIENT`.
fn parse_mcp_proxy(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = MCP_PROXY_USAGE;
    let (mut client, mut document, mut tools) = (None, None, None);
    let mut i = 0;
    while i < args.len() {
        let slot = match args[i].as_str() {
            "--client" => &mut client,
            "--document" => &mut document,
            "--tools" => &mut tools,
            other => return Err(format!("unknown option {other}\n{USAGE}")),
        };
        let value = args
            .get(i + 1)
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("{} wants a value\n{USAGE}", args[i]))?;
        *slot = Some(value.clone());
        i += 2;
    }
    let client = client.or_else(|| {
        std::env::var("MASTIK_MCP_CLIENT")
            .ok()
            .filter(|k| !k.trim().is_empty())
    });
    let all_tools = match tools.as_deref() {
        None | Some("core") => false,
        Some("all") => true,
        Some(other) => return Err(format!("--tools is core or all, not {other}\n{USAGE}")),
    };
    Ok(Command::McpProxy {
        client,
        document,
        all_tools,
    })
}

fn parse_merge(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = MERGE_USAGE;
    let (mut files, mut out) = (Vec::new(), None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                let value = args.get(i).filter(|v| !v.starts_with('-'));
                out = Some(
                    value
                        .ok_or_else(|| format!("-o wants a filename\n{USAGE}"))?
                        .clone(),
                );
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other => files.push(other.to_string()),
        }
        i += 1;
    }
    match <[String; 3]>::try_from(files) {
        Ok([base, ours, theirs]) => Ok(Command::Merge {
            base,
            ours,
            theirs,
            out,
        }),
        Err(files) => Err(format!(
            "merge takes three documents; got {}\n{USAGE}",
            files.len()
        )),
    }
}

/// `mastik merge`: whether it merged cleanly, after writing the result and listing
/// each conflict on stderr.
fn run_merge(base: &str, ours: &str, theirs: &str, out: Option<&str>) -> Result<bool, String> {
    let read = |file: &str| std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"));
    let merged = mastik_core::merge::merge(&read(base)?, &read(ours)?, &read(theirs)?)
        .map_err(|e| format!("cannot merge: {e}"))?;
    let out = out.unwrap_or(ours);
    // Atomic: the default target is `ours` itself, the only copy of that side.
    atomic::write(std::path::Path::new(out), &merged.bytes)
        .map_err(|e| format!("cannot write {out}: {e}"))?;
    for c in &merged.conflicts {
        eprintln!("conflict: {c}");
    }
    Ok(merged.conflicts.is_empty())
}

/// What `mastik tokens` takes, for its refusals and `help tokens`.
const TOKENS_USAGE: &str = "usage: mastik tokens export <file.mastik> [--mode <name>] [-o <tokens.json>]\n       \
     mastik tokens import <file.mastik> <tokens.json> [--collection <name>] [--removals] \
     [-o <out.mastik>]";

fn parse_tokens(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = TOKENS_USAGE;
    match args.first().map(String::as_str) {
        Some("export") => {}
        Some("import") => return parse_tokens_import(&args[1..], USAGE),
        _ => return Err(USAGE.to_string()),
    }
    let (mut file, mut mode, mut out) = (None, None, None);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            flag @ ("--mode" | "-o" | "--out") => {
                i += 1;
                let value = args.get(i).filter(|v| !v.starts_with('-'));
                let value = value.ok_or_else(|| format!("{flag} wants a value\n{USAGE}"))?;
                match flag {
                    "--mode" => mode = Some(value.clone()),
                    _ => out = Some(value.clone()),
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            other if file.is_some() => {
                return Err(format!(
                    "tokens export takes one document; got a second, {other}\n{USAGE}"
                ));
            }
            other => file = Some(other.to_string()),
        }
        i += 1;
    }
    Ok(Command::TokensExport {
        file: file.ok_or_else(|| USAGE.to_string())?,
        mode,
        out,
    })
}

/// `mastik tokens import`'s arguments, after `import`.
fn parse_tokens_import(args: &[String], usage: &str) -> Result<Command, String> {
    let (mut files, mut collection, mut removals, mut out) = (Vec::new(), None, false, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--removals" => removals = true,
            flag @ ("--collection" | "-o" | "--out") => {
                i += 1;
                let value = args.get(i).filter(|v| !v.starts_with('-'));
                let value = value.ok_or_else(|| format!("{flag} wants a value\n{usage}"))?;
                match flag {
                    "--collection" => collection = Some(value.clone()),
                    _ => out = Some(value.clone()),
                }
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{usage}"));
            }
            other => files.push(other.to_string()),
        }
        i += 1;
    }
    match <[String; 2]>::try_from(files) {
        Ok([file, tokens]) => Ok(Command::TokensImport {
            file,
            tokens,
            collection,
            removals,
            out,
        }),
        Err(files) => Err(format!(
            "tokens import takes a document and a tokens file; got {} of them\n{usage}",
            files.len()
        )),
    }
}

/// Whether `out` is the existing file `input`, by canonical path: the check a
/// command makes before its `-o` replaces one of the files it reads.
pub(crate) fn same_file(out: &std::path::Path, input: &std::path::Path) -> bool {
    std::fs::canonicalize(out).ok() == std::fs::canonicalize(input).ok() && out.exists()
}

/// `mastik tokens import`: the rows applied through an editor session, so they
/// settle as the review's *Import* does, and the document written atomically,
/// over itself unless `out` names another file. Nothing is written over the
/// document when nothing changes; another `out` still gets the document as it
/// stands. An `out` naming the tokens file is refused. Answers the line to
/// print; what is left out goes to stderr.
fn run_tokens_import(
    file: &str,
    tokens: &str,
    collection: Option<&str>,
    removals: bool,
    out: Option<&str>,
) -> Result<String, String> {
    let path = std::path::Path::new;
    let target = out.unwrap_or(file);
    if same_file(path(target), path(tokens)) {
        return Err(format!(
            "refusing to write the document over {tokens}, the tokens file it reads"
        ));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let text = std::fs::read(tokens)
        .map_err(|e| e.to_string())
        .and_then(tokens_import::tokens_text)
        .map_err(|e| format!("cannot read {tokens}: {e}"))?;
    let mut s = session::EditorSession::new();
    s.adopt_document(doc, None);
    let one = tokens_import::one_shot(&s.doc, &mut s.ids, &text, collection, removals)?;
    for l in &one.left_out {
        eprintln!("left out: {} ({})", l.path, l.ty);
    }
    if one.kit > 0 {
        eprintln!(
            "{} {} to a kit, left as the kit has it",
            one.kit,
            if one.kit == 1 {
                "token belongs"
            } else {
                "tokens belong"
            }
        );
    }
    let counts = format!(
        "{} added, {} changed, {} renamed, {} removed",
        one.added, one.changed, one.renamed, one.removed
    );
    if one.ops.is_empty() {
        // The document already holds the result; another `-o` is still this
        // command's output, so it must not be left missing or stale.
        if out.is_none() || same_file(path(target), path(file)) {
            return Ok(format!("nothing to change in {file}"));
        }
        atomic::write(path(target), &bytes).map_err(|e| format!("cannot write {target}: {e}"))?;
        return Ok(format!("nothing to change in {file}; wrote {target}"));
    }
    s.try_commit(mastik_core::Transaction(one.ops))
        .map_err(|e| format!("cannot apply {tokens}: {e:?}"))?;
    let saved = mastik_core::io::save(&s.doc).map_err(|e| format!("cannot save: {e}"))?;
    // Atomic: the default target is the document itself.
    atomic::write(path(target), &saved).map_err(|e| format!("cannot write {target}: {e}"))?;
    Ok(format!("{counts}; wrote {target}"))
}

/// What `mastik export` takes, for its refusals and `help export`.
const EXPORT_USAGE: &str = "usage: mastik export <file.mastik> [--svg|--pdf|--html|--react|--vue] [--outlines] \
         [--page <name|n>] [--layer <name|id>]... [-o <out>]\n       \
         mastik export <file.mastik> --png|--jpeg|--webp [--scale <n>|--width <px>|--height <px>] \
         [--quality <1-100>] [--page <name|n>] [--layer <name|id>]... [-o <out>]\n       \
         mastik export <file.mastik> --json|--penpot|--sketch|--prototype|--flow-map [-o <out>]\n       \
         mastik export <file.mastik> --ico|--icns [--sizes 16,32,48] [--page <name|n>] \
         [--layer <name|id>] [-o <out>]\n       \
         mastik export <file.mastik> --pdf --frames [--outlines] [--page <name|n>] [-o <out>]\n       \
         mastik export <file.mastik> --all [--scale-override <2x|512w|512h>] [--folders] \
         [--page <name|n>] [-o <dir>]";

fn parse_export(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = EXPORT_USAGE;
    let mut file = None;
    let mut format = None;
    let mut out = None;
    let mut page = None;
    let mut all = false;
    let mut folders = false;
    let mut outlines = false;
    let mut frames = false;
    let mut scale: Option<(&str, ExportScale)> = None;
    let mut over = None;
    let mut sizes: Option<Vec<u32>> = None;
    let mut layers = Vec::new();
    let mut quality = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--svg" => format = Some(ExportFormat::Svg),
            "--pdf" => format = Some(ExportFormat::Pdf),
            "--png" => format = Some(ExportFormat::Png),
            "--jpeg" | "--jpg" => format = Some(ExportFormat::Jpeg),
            "--webp" => format = Some(ExportFormat::Webp),
            "--layer" => {
                i += 1;
                let value = args.get(i).filter(|v| !v.starts_with('-'));
                layers.push(
                    value
                        .ok_or_else(|| format!("--layer wants a layer's name or id\n{USAGE}"))?
                        .clone(),
                );
            }
            "--quality" => {
                i += 1;
                quality = Some(
                    args.get(i)
                        .and_then(|v| v.parse::<u8>().ok())
                        .filter(|q| (1..=100).contains(q))
                        .ok_or_else(|| format!("--quality wants a number, 1 to 100\n{USAGE}"))?,
                );
            }
            "--ico" => format = Some(ExportFormat::Ico),
            "--icns" => format = Some(ExportFormat::Icns),
            "--sizes" => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                let parsed: Option<Vec<u32>> =
                    text.split(',').map(|s| s.trim().parse().ok()).collect();
                sizes = Some(parsed.filter(|v| !v.is_empty()).ok_or_else(|| {
                    format!("--sizes wants pixel sizes such as 16,32,48, not {text:?}\n{USAGE}")
                })?);
            }
            "--json" => format = Some(ExportFormat::Json),
            "--html" => format = Some(ExportFormat::Html),
            "--react" => format = Some(ExportFormat::React),
            "--vue" => format = Some(ExportFormat::Vue),
            "--penpot" => format = Some(ExportFormat::Penpot),
            "--sketch" => format = Some(ExportFormat::Sketch),
            "--prototype" => format = Some(ExportFormat::Prototype),
            "--flow-map" => format = Some(ExportFormat::FlowMap),
            "--all" => all = true,
            "--folders" => folders = true,
            "--outlines" => outlines = true,
            "--frames" => frames = true,
            // One value each and mutually exclusive: three ways of answering one
            // question. The typed flag is carried so the refusals can name it.
            flag @ ("--scale" | "--width" | "--height") => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                let asked = size_flag(flag, text, USAGE)?;
                if let Some((had, _)) = scale {
                    return Err(format!(
                        "{had} and {flag} are two answers to one question — pick one\n{USAGE}"
                    ));
                }
                scale = Some((flag, asked));
            }
            "--scale-override" => {
                i += 1;
                let text = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // The export row's own vocabulary, so a script types what the row shows.
                over = Some(ExportScale::from_label(text).ok_or_else(|| {
                    format!("--scale-override wants 2x, 512w or 512h, not {text:?}\n{USAGE}")
                })?);
            }
            "--page" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| USAGE.to_string())?;
                page = Some(name.clone());
            }
            "-o" | "--out" => {
                i += 1;
                let name = args.get(i).ok_or_else(|| USAGE.to_string())?;
                // A flag is not a filename: taking the next token unconditionally
                // would make `-o --png` write SVG bytes into a file called `--png`,
                // since the `other if other.starts_with('-')` arm never sees it.
                if name.starts_with('-') {
                    return Err(format!(
                        "-o wants a filename, not the option {name}\n{USAGE}"
                    ));
                }
                out = Some(name.clone());
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n{USAGE}"));
            }
            // A second document is refused, not overwritten: a build step whose glob
            // matched twice would otherwise export the wrong document unnoticed.
            other if file.is_some() => {
                return Err(format!(
                    "export takes one document; got {} and {other}\n{USAGE}",
                    file.as_deref().unwrap_or_default()
                ));
            }
            other => file = Some(other.to_string()),
        }
        i += 1;
    }
    if frames && (all || format != Some(ExportFormat::Pdf)) {
        return Err(format!("--frames applies only to a one-off --pdf\n{USAGE}"));
    }
    if frames {
        format = Some(ExportFormat::PdfFrames);
    }
    // Checked against what the file can hold, so a typo is refused, not dropped.
    let icon = match format {
        Some(ExportFormat::Ico) => Some(mastik_core::ExportFormat::Ico),
        Some(ExportFormat::Icns) => Some(mastik_core::ExportFormat::Icns),
        _ => None,
    };
    let sizes = match (sizes, icon) {
        (None, _) => Vec::new(),
        (Some(_), None) => {
            return Err(format!("--sizes applies only to --ico and --icns\n{USAGE}"));
        }
        (Some(sizes), Some(f)) => {
            let held = f.icon_sizes();
            if let Some(bad) = sizes.iter().find(|s| !held.contains(s)) {
                let list: Vec<String> = held.iter().map(u32::to_string).collect();
                return Err(format!(
                    "--{} holds {}, not {bad}\n{USAGE}",
                    f.extension(),
                    list.join(", ")
                ));
            }
            sizes
        }
    };
    if all {
        // A different command, not a fourth format: `--all` writes many files at
        // the document's formats, so `-o` is a folder and `--png` has nowhere to apply.
        if format.is_some() {
            return Err(format!(
                "--all runs the document's own export settings, so it takes no format\n{USAGE}"
            ));
        }
        // The size flags are refused too, and the message points at
        // `--scale-override`, which overrules a size the file already carries.
        if let Some((had, _)) = scale {
            return Err(format!(
                "--all takes its sizes from the document; use --scale-override to force them all\n\
                 ({had} sets the size of a one-off export)\n{USAGE}"
            ));
        }
        if outlines {
            return Err(format!(
                "--all takes each file's text setting from the document\n{USAGE}"
            ));
        }
        if !layers.is_empty() || quality.is_some() {
            return Err(format!(
                "--all runs every layer's own settings, so it takes no --layer or --quality\n{USAGE}"
            ));
        }
        return Ok(Command::ExportAll {
            file: file.ok_or_else(|| USAGE.to_string())?,
            out,
            folders,
            scale_override: over,
            page,
        });
    }
    if folders {
        return Err(format!("--folders applies only to --all\n{USAGE}"));
    }
    if over.is_some() {
        return Err(format!(
            "--scale-override applies only to --all; a one-off export takes --scale\n{USAGE}"
        ));
    }
    // SVG is the default: the semantic format, needing no viewport arguments.
    let format = format.unwrap_or(ExportFormat::Svg);
    // Refused rather than ignored, as everywhere in this parser. The message names
    // the rule, since the commonest way to hit it is typing no format at all.
    if let Some((had, _)) = scale
        && !format.raster()
    {
        return Err(format!(
            "{had} is a raster size, so it applies only to --png, --jpeg and --webp\n{USAGE}"
        ));
    }
    if quality.is_some() && format != ExportFormat::Jpeg {
        return Err(format!("--quality applies only to --jpeg\n{USAGE}"));
    }
    if page.is_some() && format.whole_document() {
        return Err(format!(
            "--{} writes every page, so it takes no --page\n{USAGE}",
            format.extension()
        ));
    }
    // A document format writes every layer, and a frames PDF a page per frame.
    if !layers.is_empty() && (format.whole_document() || format == ExportFormat::PdfFrames) {
        return Err(format!(
            "{} writes whole pages, so it takes no --layer\n{USAGE}",
            format.flag()
        ));
    }
    // An icon file is one layer drawn at several sizes.
    if layers.len() > 1 && icon.is_some() {
        return Err(format!(
            "{} draws one layer; got {}\n{USAGE}",
            format.flag(),
            layers.len()
        ));
    }
    if outlines
        && !matches!(
            format,
            ExportFormat::Svg | ExportFormat::Pdf | ExportFormat::PdfFrames
        )
    {
        return Err(format!(
            "--outlines applies only to --svg and --pdf\n{USAGE}"
        ));
    }
    Ok(Command::Export(OneOff {
        file: file.ok_or_else(|| USAGE.to_string())?,
        format,
        out,
        scale: scale.map_or(ExportScale::Times(1.0), |(_, s)| s),
        page,
        text: match outlines {
            true => ExportText::Outlines,
            false => ExportText::Live,
        },
        sizes,
        layers,
        quality,
    }))
}

/// The value of `--scale`, `--width` or `--height`, shared by `export` and
/// `render`. Zero and negative are refused here, not left to `factor`'s floor,
/// which would turn `--scale 0` into a one-pixel file.
fn size_flag(flag: &str, text: &str, usage: &str) -> Result<ExportScale, String> {
    match flag {
        "--width" => text
            .parse()
            .ok()
            .filter(|px| *px > 0)
            .map(ExportScale::Width),
        "--height" => text
            .parse()
            .ok()
            .filter(|px| *px > 0)
            .map(ExportScale::Height),
        _ => text
            .parse()
            .ok()
            .filter(|n: &f32| n.is_finite() && *n > 0.0)
            .map(ExportScale::Times),
    }
    .ok_or_else(|| format!("{flag} wants a positive number, not {text:?}\n{usage}"))
}

fn main() {
    let args = match unicode_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    // The app's own reading child: no hook, no instance count, no logging.
    if args.first().map(String::as_str) == Some(reader::SUBCOMMAND) {
        std::process::exit(reader::serve());
    }
    // Velopack's hook, before anything else in a GUI launch. The installer and
    // updater re-run this binary with `--veloapp-*`; `run` services the flag and
    // ends the process, so it must precede `parse`, which would refuse the flag.
    // With no such flag it returns at once.
    //
    // It applies nothing at startup: Velopack's default would apply a staged
    // update and stop every running Mastik, killing a dirty window with no
    // question. An update applies only through the chip, once nothing else is
    // running (`update::Updater::on_exit`). Other command lines skip it, see
    // `hook_runs`.
    //
    // On Windows the install and update hooks also register the `.mastik` file type
    // (the update re-registers installs that predate it), and uninstall removes it.
    if hook_runs(&args) {
        logging::init();
        let mut hooks = velopack::VelopackApp::build().set_auto_apply_on_startup(false);
        #[cfg(windows)]
        {
            hooks = hooks
                .on_after_install_fast_callback(|_| file_type::register())
                .on_after_update_fast_callback(|_| file_type::register())
                .on_before_uninstall_fast_callback(|_| file_type::unregister());
        }
        hooks.run();
    }
    // Counted by any other Mastik's updater before it applies — the GUI and a
    // headless export alike, since the updater stops both.
    instances::register();
    let parsed = parse(&args);
    // An assistant's harness runs marked; its MCP server is `mcp-proxy`, which
    // reaches only the window, and nothing else is its to run. A speed bump, not
    // a boundary: the window's grants hold the level.
    if std::env::var_os(mastik_ai::AGENT_CHILD_ENV).is_some()
        && !matches!(parsed, Ok(Command::McpProxy { .. }))
    {
        eprintln!("mastik: refused inside an assistant's agent; only `mcp-proxy` runs there");
        std::process::exit(2);
    }
    let result = match parsed {
        Ok(Command::Gui { open }) => match one_window::claim(open.as_deref()) {
            one_window::Claim::Window => {
                run_gui(open).map_err(|e| format!("mastik gui failed: {e}"))
            }
            one_window::Claim::Forwarded => {
                log::info!(target: "mastik", "handed to the running window: {open:?}");
                Ok(())
            }
            one_window::Claim::Unanswered => {
                let why = "Mastik is already running but did not answer. Close it and try again.";
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("Mastik")
                    .set_description(why)
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                Err(why.to_owned())
            }
        },
        Ok(Command::Export(e)) => run_export(&e),
        Ok(Command::ExportAll {
            file,
            out,
            folders,
            scale_override,
            page,
        }) => run_export_all(
            &file,
            out.as_deref(),
            folders,
            scale_override,
            page.as_deref(),
        ),
        Ok(Command::Render(r)) => render_cli::run(&r),
        // `diff`'s own exit codes: 1 is "they differ", so a failure is 2.
        Ok(Command::Diff { a, b, json }) => match diff_files(&a, &b, json) {
            Ok((report, differs)) => {
                print!("{report}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(i32::from(differs));
            }
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        },
        // `diff`'s codes: 1 is "a document fails", so an unreadable one is 2.
        Ok(Command::Check(check)) => std::process::exit(check_cli::run(&check)),
        Ok(Command::KitUpdate(update)) => std::process::exit(kit_cli::run(&update)),
        Ok(Command::Inspect(inspect)) => inspect_cli::run(&inspect).map(|text| println!("{text}")),
        Ok(Command::Run(run)) => run_cli::run(&run).map(|(lines, said)| {
            for line in lines {
                println!("{line}");
            }
            eprintln!("{said}");
        }),
        // `diff`'s codes: 1 is "conflicts", so a failure is 2.
        Ok(Command::Merge {
            base,
            ours,
            theirs,
            out,
        }) => match run_merge(&base, &ours, &theirs, out.as_deref()) {
            Ok(clean) => std::process::exit(i32::from(!clean)),
            Err(msg) => {
                eprintln!("{msg}");
                std::process::exit(2);
            }
        },
        Ok(Command::TokensExport { file, mode, out }) => {
            run_tokens_export(&file, mode.as_deref(), out.as_deref())
        }
        Ok(Command::TokensImport {
            file,
            tokens,
            collection,
            removals,
            out,
        }) => run_tokens_import(
            &file,
            &tokens,
            collection.as_deref(),
            removals,
            out.as_deref(),
        )
        .map(|line| println!("{line}")),
        Ok(Command::Serve) => mcp::serve::run(),
        Ok(Command::Help(text)) => {
            println!("{text}");
            Ok(())
        }
        Ok(Command::McpProxy {
            client,
            document,
            all_tools,
        }) => mcp::proxy::run(client, document, all_tools),
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    if let Err(msg) = result {
        eprintln!("{msg}");
        std::process::exit(1);
    }
}

/// What to say when the raster limit took a scale down, or `None` when it did not.
///
/// `viewport_at` caps the multiplier silently, which a build script would not
/// notice (8× on a poster coming back 3.4×); like `plan`, this reports it.
///
/// Returns the sentence rather than printing it, so the decision is assertable
/// without capturing stderr. The size is named in the spec's own vocabulary
/// (`2x`, `512w`), not the multiplier it became.
///
/// `>=` rather than `==` is deliberate and currently indistinguishable: `px`
/// takes `ceil` of the same positive product, so `got` never exceeds `asked`.
/// The `>=` form survives a change to `px`'s clamp, and no test can tell the two apart.
///
/// `limit` is the side the format holds: `png::MAX_RASTER_SIDE`, or WebP's less.
fn clamp_note(
    view: mastik_core::kurbo::Rect,
    scale: ExportScale,
    got: (u32, u32),
    limit: u32,
) -> Option<String> {
    let w = view.width().max(1.0);
    let h = view.height().max(1.0);
    let f = scale.factor(w, h);
    let (asked_w, asked_h) = ((w * f).ceil(), (h * f).ceil());
    if f64::from(got.0) >= asked_w && f64::from(got.1) >= asked_h {
        return None;
    }
    Some(format!(
        "{} wants {asked_w:.0} × {asked_h:.0} px, past the {} limit — wrote {} × {} ({:.3}×)",
        scale.label(),
        limit,
        got.0,
        got.1,
        f64::from(got.0) / w,
    ))
}

/// The page `--page` names — its number from 1, or its exact name — or, unasked,
/// the first page that is not a draft (the first page when every one is).
fn chosen_page(
    doc: &mastik_core::Document,
    asked: Option<&str>,
) -> Result<mastik_core::NodeId, String> {
    let Some(asked) = asked else {
        return Ok(doc.gathered_pages().next().unwrap_or(doc.first_page()));
    };
    let by_number = asked
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| doc.pages().get(i))
        .map(|p| p.id);
    let by_name = || {
        doc.page_ids()
            .find(|id| doc.get(*id).is_some_and(|n| n.name() == asked))
    };
    by_number.or_else(by_name).ok_or_else(|| {
        let names: Vec<&str> = doc
            .page_ids()
            .filter_map(|id| doc.get(id).map(|n| n.name()))
            .collect();
        format!("no page {asked:?}; the pages are {}", names.join(", "))
    })
}

/// The layers `--layer` names, outermost and in document order: each by its id,
/// or by its exact name on the page `--page` names, else on any page. A name
/// two layers share is refused, listing their ids, and so are layers on two pages,
/// whose coordinates would overlap in one file.
fn chosen_layers(
    doc: &mastik_core::Document,
    asked: &[String],
    page: Option<&str>,
) -> Result<Vec<mastik_core::NodeId>, String> {
    let pages: Vec<mastik_core::NodeId> = match page {
        Some(_) => vec![chosen_page(doc, page)?],
        None => doc.page_ids().collect(),
    };
    let mut found = Vec::new();
    for a in asked {
        if let Some(id) = mastik_core::NodeId::from_wire(a).filter(|i| doc.get(*i).is_some()) {
            if doc.is_page(id) {
                return Err(format!("{a} is a page; name it with --page"));
            }
            found.push(id);
            continue;
        }
        let mut named = Vec::new();
        let mut stack = pages.clone();
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if !doc.is_page(id) && n.name() == a {
                named.push(id);
            }
            stack.extend(n.children().iter().copied());
        }
        match named.as_slice() {
            [] => {
                let on = page.map(|p| format!(" on page {p:?}")).unwrap_or_default();
                return Err(format!("no layer is named {a:?}{on}"));
            }
            [one] => found.push(*one),
            several => {
                let named = mastik_core::build::in_document_order(doc, several);
                let ids: Vec<String> = named.iter().map(|i| i.to_wire()).collect();
                return Err(format!(
                    "{} layers are named {a:?}; name one by its id: {}",
                    ids.len(),
                    ids.join(", ")
                ));
            }
        }
    }
    let mut on: Vec<_> = found.iter().filter_map(|i| doc.page_of(*i)).collect();
    on.sort_unstable();
    on.dedup();
    if on.len() > 1 {
        return Err("the layers are on different pages; export them one page at a time".into());
    }
    let outer = mastik_core::build::outermost(doc, &found);
    Ok(mastik_core::build::in_document_order(doc, &outer))
}

/// `mastik diff`'s output and whether the documents differ. The text says nothing
/// when they do not, as `diff` does; the JSON is written either way.
fn diff_files(a: &str, b: &str, json: bool) -> Result<(String, bool), String> {
    let load = |file: &str| {
        let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
        mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))
    };
    let d = mastik_core::diff::diff(&load(a)?, &load(b)?);
    let report = if json {
        serde_json::to_string_pretty(&d).map_err(|e| format!("cannot serialize the diff: {e}"))?
            + "\n"
    } else {
        d.text()
    };
    Ok((report, !d.is_empty()))
}

/// `mastik tokens export`: the DTCG file to `out`, or to stdout, its notes on
/// stderr. A `mode` the document does not have is refused with the names it does,
/// and an `out` naming the document.
fn run_tokens_export(file: &str, mode: Option<&str>, out: Option<&str>) -> Result<(), String> {
    if let Some(path) = out
        && same_file(std::path::Path::new(path), std::path::Path::new(file))
    {
        return Err(format!(
            "refusing to write the tokens over {file}, the document they are read from"
        ));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let modes: Vec<&mastik_core::Mode> = doc.variables().modes().collect();
    let mode = match mode {
        None => None,
        Some(name) => Some(
            modes
                .iter()
                .find(|m| m.name == name)
                .map(|m| m.id)
                .ok_or_else(|| {
                    let names: Vec<&str> = modes.iter().map(|m| m.name.as_str()).collect();
                    format!(
                        "{file} has no mode {name:?}; its modes: {}",
                        names.join(", ")
                    )
                })?,
        ),
    };
    let tokens = mastik_export::dtcg::tokens(&doc, mode);
    for note in &tokens.notes {
        eprintln!("note: {note}");
    }
    match out {
        Some(path) => {
            std::fs::write(path, &tokens.json).map_err(|e| format!("cannot write {path}: {e}"))
        }
        None => {
            print!("{}", tokens.json);
            Ok(())
        }
    }
}

/// Headless export (§7). Loads the document, resolves it, and writes one of the
/// export formats of one page, or of the layers `--layer` names — no window, no
/// GPU, no font service (text uses the bundled fallback, which keeps the output
/// deterministic in CI).
///
/// `scale` reaches only the raster arms, and the output path is not decorated with
/// it: `plan`'s `@2x` marker disambiguates a set of files, not this command's one
/// output.
///
/// `Width`/`Height` are resolved here, against the same box the render frames,
/// since `--width 512` is a different multiplier on every document.
fn run_export(e: &OneOff) -> Result<(), String> {
    use mastik_core::Resolved;

    let (file, format, scale, text) = (e.file.as_str(), e.format, e.scale, e.text);
    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let mut doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::settled(&mut doc);
    let layers = chosen_layers(&doc, &e.layers, e.page.as_deref())?;
    // The layers' page when they are asked for, so `--page` is only a scope.
    let page = match layers.first().and_then(|l| doc.page_of(*l)) {
        Some(page) => page,
        None => chosen_page(&doc, e.page.as_deref())?,
    };
    // What the file draws: the layers, else the whole page.
    let origins = match layers.is_empty() {
        true => vec![page],
        false => layers.clone(),
    };
    let subject = match layers.len() {
        0 => "the page",
        1 => "the layer",
        _ => "the layers",
    };
    // A drawing draws what the app's exports draw, frames' fills left out as
    // asked (`plan::for_export`); a document format carries the fill as data.
    let draws = matches!(
        format,
        ExportFormat::Svg
            | ExportFormat::Pdf
            | ExportFormat::PdfFrames
            | ExportFormat::Png
            | ExportFormat::Jpeg
            | ExportFormat::Webp
            | ExportFormat::Ico
            | ExportFormat::Icns
    );
    let stripped = draws
        .then(|| mastik_export::plan::for_export(&doc, &origins))
        .flatten();
    let (doc, res) = stripped.unwrap_or((doc, res));

    let out_path = e
        .out
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(file).with_extension(format.extension()));
    // `-o` naming the document itself would replace it with the export.
    if same_file(&out_path, std::path::Path::new(file)) {
        return Err(format!(
            "refusing to write the export over {file}, the document it is made from"
        ));
    }
    let replaced = out_path.exists();

    let data = match format {
        ExportFormat::Svg => mastik_export::svg::svg_with(&doc, &res, &origins, text)
            .svg
            .into_bytes(),
        ExportFormat::Pdf => {
            let export = mastik_export::pdf::pdf_of(&doc, &res, &origins, text)
                .ok_or_else(|| format!("cannot convert {file} to PDF"))?;
            if !export.approximated.is_empty() {
                eprintln!("drawn differently: {}", export.approximated.join(", "));
            }
            export.pdf
        }
        ExportFormat::PdfFrames => {
            let pages: Vec<(String, Vec<mastik_core::NodeId>)> = doc
                .get(page)
                .map(|r| r.children().to_vec())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|id| {
                    let n = doc.get(id)?;
                    (doc.shown(id) && matches!(n.kind(), mastik_core::NodeKind::Artboard { .. }))
                        .then(|| (n.name().to_string(), vec![id]))
                })
                .collect();
            if pages.is_empty() {
                return Err(format!("{file}: the page has no visible frames"));
            }
            let export = mastik_export::pdf::pdf_pages(&doc, &res, &pages, text)
                .ok_or_else(|| format!("cannot convert {file} to PDF"))?;
            if !export.approximated.is_empty() {
                eprintln!("drawn differently: {}", export.approximated.join(", "));
            }
            export.pdf
        }
        ExportFormat::Html | ExportFormat::React | ExportFormat::Vue => {
            use mastik_export::code::Target;
            // The layers asked for, else the page's top-level layers, laid out as
            // on the canvas.
            let origins = match layers.is_empty() {
                true => doc
                    .get(page)
                    .map(|r| r.children().to_vec())
                    .unwrap_or_default(),
                false => layers.clone(),
            };
            let target = match format {
                ExportFormat::React => Target::React,
                ExportFormat::Vue => Target::Vue,
                _ => Target::Html,
            };
            let export = target.export(&doc, &res, &origins);
            if !export.approximated.is_empty()
                || !export.positioned.is_empty()
                || !export.inlined.is_empty()
            {
                eprintln!(
                    "{} drawn differently, {} with fixed positions or as pictures, \
                     {} instances written out as layers — see report.txt",
                    export.approximated.len(),
                    export.positioned.len(),
                    export.inlined.len()
                );
            }
            export
                .zip()
                .map_err(|e| format!("cannot write the zip: {e}"))?
        }
        ExportFormat::Json => {
            let snap = mastik_export::snapshot(&doc, &res);
            serde_json::to_vec_pretty(&snap)
                .map_err(|e| format!("cannot serialize snapshot: {e}"))?
        }
        ExportFormat::Penpot | ExportFormat::Sketch => {
            let name = std::path::Path::new(file)
                .file_stem()
                .map_or_else(|| "Mastik".into(), |s| s.to_string_lossy().into_owned());
            let (bytes, approximated) = match format {
                ExportFormat::Sketch => mastik_export::sketch::export(&doc, &res, &name)
                    .map(|e| (e.bytes, e.approximated)),
                _ => mastik_export::penpot::export(&doc, &res, &name)
                    .map(|e| (e.bytes, e.approximated)),
            }
            .map_err(|e| format!("cannot write the zip: {e}"))?;
            if !approximated.is_empty() {
                eprintln!("drawn differently: {}", approximated.join(", "));
            }
            bytes
        }
        ExportFormat::Png if layers.is_empty() => {
            // Frame the whole page at `scale` device pixels per world unit. The
            // 512-square is this command's answer to an empty page (`extent`
            // returns `None`; the SVG writer wants a different fallback).
            let view = mastik_export::extent(&res, &[page])
                .unwrap_or_else(|| mastik_core::kurbo::Rect::new(0.0, 0.0, 512.0, 512.0));
            let vp = mastik_export::png::viewport_at(
                view,
                scale.factor(view.width().max(1.0), view.height().max(1.0)),
            );
            let limit = mastik_export::png::MAX_RASTER_SIDE;
            if let Some(note) = clamp_note(view, scale, vp.pixel_size, limit) {
                eprintln!("{note}");
            }
            mastik_export::png::png(&doc, &res, &vp, page)
        }
        ExportFormat::Png | ExportFormat::Jpeg | ExportFormat::Webp => {
            use mastik_core::ExportFormat as Spec;
            let spec_format = match format {
                ExportFormat::Jpeg => Spec::Jpeg,
                ExportFormat::Webp => Spec::Webp,
                _ => Spec::Png,
            };
            let mut spec = mastik_core::ExportSpec::new(spec_format, scale);
            if let Some(q) = e.quality {
                spec.quality = q;
            }
            let view = mastik_export::extent(&res, &origins)
                .ok_or_else(|| format!("{file}: {subject} has nothing to draw"))?;
            let (w, h) = (view.width().max(1.0), view.height().max(1.0));
            let opts = mastik_export::png::RasterOpts {
                scale: mastik_export::plan::format_capped(scale.factor(w, h), w, h, spec_format),
                // What `plan::background_color` makes of a transparent JPEG row.
                background: (format == ExportFormat::Jpeg)
                    .then_some(mastik_core::peniko::Color::WHITE),
                ..Default::default()
            };
            let r = mastik_export::png::raster_of(&doc, &res, &origins, &opts)
                .ok_or_else(|| format!("{file}: {subject} has nothing to draw"))?;
            let limit = match format {
                ExportFormat::Webp => mastik_export::webp::MAX_SIDE,
                _ => mastik_export::png::MAX_RASTER_SIDE,
            };
            if let Some(note) = clamp_note(view, scale, (r.width, r.height), limit) {
                eprintln!("{note}");
            }
            mastik_export::plan::encode(&r, &spec)
        }
        ExportFormat::Ico | ExportFormat::Icns => {
            // The layer's or the page's whole drawing, centred in a square at each size.
            let format = match format {
                ExportFormat::Ico => mastik_core::ExportFormat::Ico,
                _ => mastik_core::ExportFormat::Icns,
            };
            let spec = mastik_core::ExportSpec {
                sizes: e.sizes.clone(),
                ..mastik_core::ExportSpec::new(format, ExportScale::Times(1.0))
            };
            let entries = mastik_export::icon::entries(&doc, &res, origins[0], &spec)
                .ok_or_else(|| format!("{file}: {subject} has nothing to draw"))?;
            mastik_export::icon::encode(format, &entries).unwrap_or_default()
        }
        ExportFormat::Prototype => prototype_html(&doc, file)?.into_bytes(),
        ExportFormat::FlowMap => {
            let export = mastik_export::flow_map::flow_map(&doc, &res).ok_or_else(|| {
                match mastik_export::flow_map::plans(&doc).is_empty() {
                    true => format!("{file}: no page has a flow to map"),
                    false => format!("cannot convert {file} to a flow map"),
                }
            })?;
            if !export.approximated.is_empty() {
                eprintln!("drawn differently: {}", export.approximated.join(", "));
            }
            export.pdf
        }
    };

    std::fs::write(&out_path, data)
        .map_err(|e| format!("cannot write {}: {e}", out_path.display()))?;
    // A door that writes with no dialog owes its count of what it replaced.
    let note = if replaced { " (replaced)" } else { "" };
    println!("wrote {}{note}", out_path.display());
    Ok(())
}

/// `--prototype`: the HTML player of `doc`'s chosen flows. The installed faces of
/// the families the document names are registered first, so the file carries the
/// fonts the canvas would draw with.
fn prototype_html(doc: &mastik_core::Document, file: &str) -> Result<String, String> {
    let (wasm, glue) = player_build(doc)?;
    mcp::system_fonts::SystemFonts::load().register_for(doc);
    let player = mastik_export::prototype::Player {
        wasm: &wasm,
        glue: &glue,
    };
    let parts =
        mastik_export::prototype::Parts::of(doc, player).map_err(|e| format!("{file}: {e}"))?;
    Ok(parts.html())
}

/// The browser player `crates/mastik-web/build.sh` writes: from the folder
/// `MASTIK_PLAYER_DIR` names, else `player` beside the executable (Windows, the
/// Linux archive and AppImage) or one folder up (`target/player` for a `cargo
/// run` build), else where a package puts data: `../share/mastik/player` from
/// `/usr/bin`, `../Resources/player` from a macOS bundle's `Contents/MacOS`.
///
/// When `doc` runs a script, each folder's `-scripts` sibling is tried first: the
/// player that carries Luau, which only such a file pays for. Without it the
/// plain player plays, skipping *Run script* with a notice.
pub(crate) fn player_build(doc: &mastik_core::Document) -> Result<(Vec<u8>, String), String> {
    const WASM: &str = "mastik_web_bg.wasm";
    const GLUE: &str = "mastik_web.js";
    let scripts = !player::scripts_played(doc).is_empty();
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("MASTIK_PLAYER_DIR") {
        dirs.push(dir.into());
    }
    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors().skip(1).take(2) {
            dirs.push(dir.join("player"));
        }
        if let Some(up) = exe.parent().and_then(|d| d.parent()) {
            dirs.push(up.join("share").join("mastik").join("player"));
            dirs.push(up.join("Resources").join("player"));
        }
    }
    let dirs = prototype_export::player_folders(dirs, scripts);
    for dir in &dirs {
        if let (Ok(wasm), Ok(glue)) = (
            std::fs::read(dir.join(WASM)),
            std::fs::read_to_string(dir.join(GLUE)),
        ) {
            return Ok((wasm, glue));
        }
    }
    Err(format!(
        "no browser player found (looked for {WASM} in {}); build it with \
         crates/mastik-web/build.sh, or name its folder in MASTIK_PLAYER_DIR",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Headless run of the document's own export settings (§7).
///
/// Exits 1 when no layer has export settings, or when every file was skipped:
/// a build step that succeeds without producing its output goes unnoticed.
///
/// The whole plan is rendered before anything is written, so a failure part way
/// through leaves files that all came from one state of the document.
fn run_export_all(
    file: &str,
    out: Option<&str>,
    folders: bool,
    scale_override: Option<ExportScale>,
    page: Option<&str>,
) -> Result<(), String> {
    let report = export_all(file, out, folders, scale_override, page)?;
    println!("{report}");
    Ok(())
}

/// [`run_export_all`]'s work, answering the line it prints.
fn export_all(
    file: &str,
    out: Option<&str>,
    folders: bool,
    scale_override: Option<ExportScale>,
    page: Option<&str>,
) -> Result<String, String> {
    use mastik_core::Resolved;

    let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
    let mut doc = mastik_core::io::load(&bytes).map_err(|e| format!("cannot load {file}: {e}"))?;
    let res = Resolved::settled(&mut doc);

    // Every layer carrying settings, in document order — the same set and the same
    // order the panel's *Export all* collects (drafts left out), so the two produce
    // the same files. `--page` names the one page instead.
    let mut subjects = Vec::new();
    let mut stack: Vec<mastik_core::NodeId> = match page {
        Some(_) => vec![chosen_page(&doc, page)?],
        None => doc.gathered_pages().collect(),
    };
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if mastik_core::build::exported_in_bulk(&doc, id) {
            subjects.push(id);
        }
        stack.extend(node.children().iter().copied());
    }
    // `all_in_document_order`, matching the panel exactly — see `exportable_layers`.
    let subjects = mastik_core::build::all_in_document_order(&doc, &subjects);
    if subjects.is_empty() {
        return Err(format!("{file}: no layer has export settings"));
    }

    let dir = out
        .map(std::path::PathBuf::from)
        .or_else(|| std::path::Path::new(file).parent().map(Into::into))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let plan = mastik_export::plan(
        &doc,
        &res,
        &subjects,
        mastik_export::PlanOptions {
            folders_from_names: folders,
            scale_override,
            unset: None,
        },
    );

    // The pictures beside a linked table, for every row a file reads, as the
    // app's export worker reads them.
    let doc = panels::with_row_pictures(doc, &plan);
    let mut written = Vec::new();
    let mut skipped = 0usize;
    let mut missing_pictures = 0usize;
    let mut overflowing: Vec<(mastik_core::NodeId, usize)> = Vec::new();
    for f in &plan {
        let (data, filled) = mastik_export::plan::render_noted(&doc, &res, f);
        missing_pictures += filled.missing_pictures;
        for row in filled.overflowing {
            if !overflowing.contains(&(f.node, row)) {
                overflowing.push((f.node, row));
            }
        }
        match data {
            Some(data) => written.push((dir.join(&f.path), data)),
            None => {
                eprintln!("skipped {} — that layer has no size", f.path);
                skipped += 1;
            }
        }
    }
    // Every file skipped is a failure: `subjects.is_empty()` above counts layers,
    // not files produced. The raster arm is the reachable one — `png::raster_of`
    // opens with `extent(res, origins)?`, so an ink-less layer skips every raster
    // spec, while the SVG arm always returns `Some`.
    if written.is_empty() {
        return Err(format!(
            "{file}: every export was skipped — no layer with export settings has a size \
             ({skipped} skipped)"
        ));
    }
    // A door that writes with no dialog owes a count of what it replaced (§15 D636),
    // and this one is the worse case: with no `-o`, `dir` is the folder holding the
    // `.mastik`, where a hand-authored `logo.svg` may sit. The count is taken before
    // the write, since afterwards every path exists. Same sentence as
    // `write_export`'s, a separate implementation of the same walk.
    let replaced = write_planned(&written)?;
    let report = export_all_report(written.len(), replaced, skipped, &dir);
    let report = match missing_pictures {
        0 => report,
        n => format!("{report} ({} not found)", ui::count(n, "picture")),
    };
    Ok(match overflowing.len() {
        0 => report,
        1 => format!("{report} (1 row overflows its box)"),
        n => format!("{report} ({n} rows overflow their box)"),
    })
}

/// Write a rendered plan to disk, answering how many of those paths already held
/// a file.
///
/// The count is taken before the write, since afterwards every path exists; it has
/// the same shape as `panels::export::write_export`'s folder arm. Separate from
/// `run_export_all` so the count is assertable rather than only printed.
fn write_planned(files: &[(std::path::PathBuf, Vec<u8>)]) -> Result<usize, String> {
    let mut replaced = 0usize;
    for (path, data) in files {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        if path.exists() {
            replaced += 1;
        }
        std::fs::write(path, data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    Ok(replaced)
}

/// What `--all` says when it is done.
///
/// The folder comes first and the parenthetical counts follow, since a terminal
/// line is not truncated. `(n replaced)` is owed by any door that writes without
/// a dialog.
fn export_all_report(
    written: usize,
    replaced: usize,
    skipped: usize,
    dir: &std::path::Path,
) -> String {
    let mut out = format!(
        "wrote {} file{} to {}",
        written,
        if written == 1 { "" } else { "s" },
        dir.display(),
    );
    if replaced > 0 {
        out.push_str(&format!(" ({replaced} replaced)"));
    }
    if skipped > 0 {
        out.push_str(&format!(" ({skipped} skipped)"));
    }
    out
}

/// The running window's icon — taskbar button, Alt-Tab, and on macOS the Dock.
///
/// Distinct from the icon on `mastik.exe`, which `build.rs` compiles into the
/// resource table as a multi-size `.ico`: winit registers its window class with
/// `hIcon: 0` rather than falling back to the executable's, so each covers
/// surfaces the other does not.
///
/// One size is chosen, since `WM_SETICON` gets the same pixels for `ICON_BIG` and
/// `ICON_SMALL`. `icons/icon.png` is 512 so it has something left to give at any
/// display scaling; the 16px copies are a downscale done by the OS. (The 1024 is
/// for the macOS `.icns` alone, and the 64 is the top bar's mark.)
fn window_icon() -> eframe::egui::IconData {
    static PNG: &[u8] = include_bytes!("../../../icons/icon.png");
    // Compiled in, so this works on every run or on none; the test below holds
    // the "every" end.
    eframe::icon_data::from_png_bytes(PNG).expect("the bundled window icon is not a valid PNG")
}

fn run_gui(open: Option<std::path::PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        // The system's title bar off, or transparent on macOS, with the app
        // drawing what the host leaves to it (`chrome`).
        viewport: chrome::Chrome::current().viewport(
            eframe::egui::ViewportBuilder::default()
                .with_inner_size([1200.0, 800.0])
                .with_title("Mastik")
                .with_icon(window_icon()),
        ),
        // The window's last size, position and maximized state, beside
        // `prefs.json`; with no file yet, the size above.
        persistence_path: dirs::config_dir().map(|d| d.join("mastik").join("window.ron")),
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    open_event::install();
    eframe::run_native(
        "Mastik",
        options,
        Box::new(|cc| {
            // A quit becomes the window's close request, so unsaved work is asked about.
            #[cfg(target_os = "macos")]
            quit::install(&cc.egui_ctx);
            // A launch by the OS's open event has queued its files by now, as AppKit
            // delivers it before the window is made, and so has a later launch that
            // came while the window was being made; the first opens as
            // `mastik <file>` does, and a frame takes the rest.
            opens::wake_with(&cc.egui_ctx);
            let open = open.or_else(opens::take_first);
            Ok(Box::new(app::MastikApp::new(cc, open)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `mcp-proxy` takes `--client`, `--document` and `--tools core|all`, each
    /// with a value, and nothing else.
    #[test]
    fn mcp_proxy_takes_a_client_and_a_document() {
        let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse(&args(&[
                "mcp-proxy",
                "--client",
                "chat-1",
                "--document",
                "site"
            ])),
            Ok(Command::McpProxy {
                client: Some("chat-1".into()),
                document: Some("site".into()),
                all_tools: false,
            })
        );
        assert_eq!(
            parse(&args(&["mcp-proxy", "--tools", "all"])),
            Ok(Command::McpProxy {
                client: None,
                document: None,
                all_tools: true,
            })
        );
        assert!(parse(&args(&["mcp-proxy", "--tools", "some"])).is_err());
        assert!(parse(&args(&["mcp-proxy", "--client"])).is_err());
        assert!(parse(&args(&["mcp-proxy", "--port", "1"])).is_err());
    }

    /// `--help` and `help` list the commands, `help <command>` and a command's own
    /// `--help` show what it takes (the usage its refusals print), and `--version`
    /// names the version; a command `help` does not know is refused. Every listed
    /// command is one `parse` dispatches, though nothing checks that `parse`
    /// dispatches no command the list lacks.
    ///
    /// flip: `parse` checking a command's own `--help` after dispatch → fails at
    /// `export --help`.
    #[test]
    fn help_lists_the_commands_and_shows_each() {
        let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let help = |s: &[&str]| match parse(&args(s)) {
            Ok(Command::Help(text)) => text,
            other => panic!("{s:?} is not help: {other:?}"),
        };
        for overview in [&["--help"][..], &["-h"], &["help"]] {
            assert_eq!(help(overview), OVERVIEW);
        }
        for (name, usage) in COMMANDS {
            assert!(
                OVERVIEW.contains(&format!("\n  {name} ")),
                "{name} is listed"
            );
            assert_eq!(help(&["help", name]), usage);
            assert!(
                usage.starts_with(&format!(
                    "usage: mastik {}",
                    match name {
                        "gui" => "[gui]",
                        n => n,
                    }
                )),
                "{usage}"
            );
            // Dispatched: a bare command is never "unknown".
            let bare = parse(&args(&[name]));
            assert!(
                !bare
                    .as_ref()
                    .is_err_and(|e| e.starts_with("unknown command")),
                "{name}: {bare:?}"
            );
        }
        assert_eq!(
            help(&["export", "a.mastik", "--png", "--help"]),
            EXPORT_USAGE
        );
        assert_eq!(help(&["check", "-h"]), check_cli::USAGE);
        assert_eq!(
            help(&["--version"]),
            format!("mastik {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(parse(&args(&["help", "exprot"])).is_err());
        assert!(parse(&args(&["--version", "x"])).is_err());
    }

    /// The help's own command lines parse as it says: `gui <file>` opens the file
    /// and refuses anything else, `--help export` is `help export`, and a
    /// document's `--help` is the window's usage. flip: `gui` ignoring what
    /// follows → fails at "gui opens the file".
    #[test]
    fn the_usage_lines_parse_as_the_help_says() {
        let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        match parse(&args(&["gui", "a.mastik"])) {
            Ok(Command::Gui { open: Some(p) }) => assert!(p.ends_with("a.mastik")),
            other => panic!("gui opens the file: {other:?}"),
        }
        assert_eq!(
            parse(&args(&["gui", "--bogus"])).err().as_deref(),
            Some(GUI_USAGE)
        );
        assert!(matches!(
            parse(&args(&["--help", "export"])),
            Ok(Command::Help(u)) if u == EXPORT_USAGE
        ));
        assert!(matches!(
            parse(&args(&["a.mastik", "--help"])),
            Ok(Command::Help(u)) if u == GUI_USAGE
        ));
    }

    /// Only a GUI launch and Velopack's own re-invocations reach the install hook;
    /// a CLI command, `--version`, `help` or a typo does not.
    ///
    /// flip: `hook_runs` as the negation of a three-subcommand allowlist → fails at
    /// `["exprot", "a.mastik"]`.
    #[test]
    fn only_a_gui_launch_or_velopack_reaches_the_install_hook() {
        let args = |s: &[&str]| s.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        for not in [
            &["export", "a.mastik"][..],
            &["serve", "a.mastik"],
            &["mcp-proxy"],
            &["exprot", "a.mastik"],
            &["--version"],
            &["help"],
        ] {
            assert!(!hook_runs(&args(not)), "{not:?}");
        }
        for hook in [
            &[][..],
            &["gui"],
            &["--veloapp-install", "0.4.0"],
            &["--veloapp-updated", "0.4.0"],
            &["--veloapp-uninstall"],
            &["--VELOAPP-OBSOLETE", "0.4.0"],
        ] {
            assert!(hook_runs(&args(hook)), "{hook:?}");
        }
    }

    /// An argument that is not Unicode is refused, not panicked on — a lone
    /// surrogate on Windows, an invalid byte on Unix.
    #[test]
    fn an_argument_that_is_not_unicode_is_refused() {
        #[cfg(windows)]
        let bad = {
            use std::os::windows::ffi::OsStringExt;
            // A lone trailing surrogate.
            std::ffi::OsString::from_wide(&[0x61, 0xDC00, 0x62])
        };
        #[cfg(unix)]
        let bad = {
            use std::os::unix::ffi::OsStringExt;
            std::ffi::OsString::from_vec(vec![b'a', 0xE9, b'.', b'o'])
        };
        let ok = || std::ffi::OsString::from("export");
        assert_eq!(
            unicode_args([ok(), "a.mastik".into()].into_iter()),
            Ok(vec!["export".to_string(), "a.mastik".to_string()])
        );
        let refused = unicode_args([ok(), bad].into_iter()).unwrap_err();
        assert!(refused.contains("not valid Unicode"), "{refused}");
    }

    /// `window_icon` decodes as the 512px Mastik mark: a transparent corner, the
    /// "m" cut out of the drop, and opaque points on the drop. The size pins the
    /// file, since `icon-64.png` and `icon-1024.png` are the same artwork.
    #[test]
    fn window_icon_is_the_mastik_mark_at_the_size_windows_scales_from() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);

        let alpha_at = |x: usize, y: usize| icon.rgba[(y * 512 + x) * 4 + 3];
        assert_eq!(alpha_at(0, 0), 0, "the mark must not fill its box");
        assert_eq!(alpha_at(256, 256), 0, "the m's middle stem is cut out");
        assert_eq!(alpha_at(60, 300), 255, "the left of the drop is solid ink");
        assert_eq!(alpha_at(256, 470), 255, "and so is its bottom");
    }

    #[test]
    fn defaults_to_gui() {
        assert_eq!(parse(&[]), Ok(Command::Gui { open: None }));
        assert_eq!(parse(&["gui".into()]), Ok(Command::Gui { open: None }));
    }

    /// A first argument naming a document opens it, as an absolute path, and reaches
    /// the install hook as a GUI launch does; a typo of a subcommand is still refused.
    /// flip: `names_a_document` answering false → fails at the `.mastik` launch.
    #[test]
    fn a_document_on_the_command_line_opens_in_the_gui() {
        let open = |a: &str| match parse(&[a.to_string()]) {
            Ok(Command::Gui { open: Some(p) }) => Some(p),
            _ => None,
        };
        let named = open("Landing.MASTIK").expect("a .mastik name opens, whatever its case");
        assert!(named.is_absolute() && named.ends_with("Landing.MASTIK"));
        // An existing file of any name opens too (the loader says what is wrong with it).
        let file = std::env::temp_dir().join("mastik-launch-arg-test.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(open(file.to_str().unwrap()), Some(file.clone()));
        let _ = std::fs::remove_file(&file);
        assert!(hook_runs(&["a.mastik".to_string()]));
        assert!(
            parse(&["exprot".into()]).is_err(),
            "not a file, not a .mastik"
        );
        assert!(
            parse(&["--a.mastik".into()]).is_err(),
            "a flag is not a file"
        );
        assert!(
            parse(&["a.mastik".into(), "b.mastik".into()]).is_err(),
            "one document per launch"
        );
    }

    /// `serve` takes no document: an agent picks them with `use_document`.
    #[test]
    fn serve_takes_no_file() {
        assert!(matches!(parse(&["serve".into()]), Ok(Command::Serve)));
        assert!(parse(&["serve".into(), "a.mastik".into()]).is_err());
    }

    #[test]
    fn unknown_command_errors() {
        assert!(parse(&["frobnicate".into()]).is_err());
    }

    /// `diff` takes exactly two documents and one flag, and refuses the rest.
    #[test]
    fn diff_takes_two_documents_and_json() {
        let diff = |s: &[&str]| {
            let mut v = vec!["diff".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert_eq!(
            diff(&["a.mastik", "--json", "b.mastik"]),
            Ok(Command::Diff {
                a: "a.mastik".into(),
                b: "b.mastik".into(),
                json: true,
            })
        );
        for refused in [
            &["a.mastik"][..],
            &["a.mastik", "b.mastik", "c.mastik"],
            &["a.mastik", "b.mastik", "--svg"],
        ] {
            assert!(diff(refused).is_err(), "{refused:?}");
        }
    }

    /// `--frames` turns `--pdf` into a page per frame, takes `--outlines`, and is
    /// refused anywhere else.
    #[test]
    fn frames_applies_only_to_a_one_off_pdf() {
        let export = |s: &[&str]| {
            let mut v = vec!["export".to_string(), "a.mastik".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert!(matches!(
            export(&["--pdf", "--frames", "--outlines"]),
            Ok(Command::Export(OneOff {
                format: ExportFormat::PdfFrames,
                text: ExportText::Outlines,
                ..
            }))
        ));
        for refused in [
            &["--frames"][..],
            &["--svg", "--frames"],
            &["--all", "--frames"],
            &["--pdf", "--frames", "--scale", "2"],
        ] {
            assert!(export(refused).is_err(), "{refused:?}");
        }
    }

    /// `tokens import` takes a document, a tokens file, a collection, removals and
    /// an output, and refuses the rest.
    #[test]
    fn tokens_import_takes_a_document_and_a_tokens_file() {
        let tokens = |s: &[&str]| {
            let mut v = vec!["tokens".to_string(), "import".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert_eq!(
            tokens(&[
                "a.mastik",
                "t.json",
                "--removals",
                "--collection",
                "Brand",
                "-o",
                "b.mastik"
            ]),
            Ok(Command::TokensImport {
                file: "a.mastik".into(),
                tokens: "t.json".into(),
                collection: Some("Brand".into()),
                removals: true,
                out: Some("b.mastik".into()),
            })
        );
        for refused in [
            &["a.mastik"][..],
            &["a.mastik", "t.json", "u.json"],
            &["a.mastik", "t.json", "--collection"],
            &["a.mastik", "t.json", "--mode", "Dark"],
        ] {
            assert!(tokens(refused).is_err(), "{refused:?}");
        }
    }

    /// An import adds the file's tokens, changes a value it moved, keeps what it
    /// dropped unless `--removals` asks, writes nothing when nothing changes, and
    /// leaves the document alone under `-o`.
    ///
    /// flip: `one_shot` ticking removals whatever is asked → fails at the first
    /// red import's count, `1 removed`.
    #[test]
    fn tokens_import_applies_the_file_once() {
        let dir = std::env::temp_dir().join(format!("mastik-tokens-in-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.mastik");
        let blank = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(0xD6).mint(),
        ))
        .unwrap();
        std::fs::write(&doc, &blank).unwrap();
        let doc = doc.to_string_lossy().into_owned();
        let write = |name: &str, text: &str| {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            p.to_string_lossy().into_owned()
        };
        let both = write(
            "both.json",
            r##"{"brand": {"$type": "color", "$value": "#3366cc"},
                "sm": {"$type": "dimension", "$value": "8px"}}"##,
        );
        let red = write(
            "red.json",
            r##"{"brand": {"$type": "color", "$value": "#ff0000"}}"##,
        );
        let names = || -> Vec<String> {
            let d = mastik_core::io::load(&std::fs::read(&doc).unwrap()).unwrap();
            let mut n: Vec<String> = d.variables().iter().map(|v| v.name.clone()).collect();
            n.sort();
            n
        };

        let copy = dir.join("b.mastik").to_string_lossy().into_owned();
        run_tokens_import(&doc, &both, None, false, Some(&copy)).unwrap();
        assert_eq!(
            std::fs::read(&doc).unwrap(),
            blank,
            "-o leaves the document"
        );

        let line = run_tokens_import(&doc, &both, None, false, None).unwrap();
        assert!(line.contains(" 0 changed"), "{line}");
        assert_eq!(names(), ["brand", "sm"]);
        let applied = std::fs::read(&doc).unwrap();
        let line = run_tokens_import(&doc, &both, None, false, None).unwrap();
        assert!(line.starts_with("nothing to change"), "{line}");
        assert_eq!(std::fs::read(&doc).unwrap(), applied, "nothing written");

        let line = run_tokens_import(&doc, &red, None, false, None).unwrap();
        assert!(
            line.starts_with("0 added, 1 changed, 0 renamed, 0 removed"),
            "{line}"
        );
        assert_eq!(names(), ["brand", "sm"], "kept without --removals");
        let line = run_tokens_import(&doc, &red, None, true, None).unwrap();
        assert!(line.contains(" 1 removed"), "{line}");
        assert_eq!(names(), ["brand"]);

        assert!(run_tokens_import(&doc, &red, Some("Nope"), false, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `-o` naming the tokens file is refused and leaves it; an `-o` naming
    /// another file is written even when nothing changes, so it is never left
    /// missing.
    ///
    /// flip: `run_tokens_import` without its tokens-file check → fails at
    /// `unwrap_err`; returning before the write whenever nothing changes →
    /// fails at "wrote".
    #[test]
    fn tokens_import_guards_and_always_writes_its_output() {
        let dir = std::env::temp_dir().join(format!("mastik-tokens-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.mastik");
        let blank = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(0xD7).mint(),
        ))
        .unwrap();
        std::fs::write(&doc, &blank).unwrap();
        let doc = doc.to_string_lossy().into_owned();
        let tokens = dir.join("t.json");
        let text = r##"{"brand": {"$type": "color", "$value": "#3366cc"}}"##;
        std::fs::write(&tokens, text).unwrap();
        let tokens = tokens.to_string_lossy().into_owned();

        let err = run_tokens_import(&doc, &tokens, None, false, Some(&tokens)).unwrap_err();
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(std::fs::read_to_string(&tokens).unwrap(), text);

        run_tokens_import(&doc, &tokens, None, false, None).unwrap();
        let applied = std::fs::read(&doc).unwrap();
        let out = dir.join("build.mastik").to_string_lossy().into_owned();
        let line = run_tokens_import(&doc, &tokens, None, false, Some(&out)).unwrap();
        assert!(line.starts_with("nothing to change"), "{line}");
        assert!(line.ends_with(&format!("wrote {out}")), "{line}");
        assert_eq!(std::fs::read(&out).unwrap(), applied);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tokens file Windows PowerShell 5.1's `>` wrote, UTF-16 with its mark,
    /// imports as its text does.
    ///
    /// flip: `run_tokens_import` reading with `read_to_string` → fails at the
    /// `unwrap` of the import.
    #[test]
    fn tokens_import_reads_a_utf16_file() {
        let dir = std::env::temp_dir().join(format!("mastik-tokens-u16-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.mastik");
        let blank = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(0xD8).mint(),
        ))
        .unwrap();
        std::fs::write(&doc, &blank).unwrap();
        let doc = doc.to_string_lossy().into_owned();
        let text = r##"{"brand": {"$type": "color", "$value": "#3366cc"}}"##;
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let tokens = dir.join("t.json");
        std::fs::write(&tokens, &bytes).unwrap();
        let tokens = tokens.to_string_lossy().into_owned();
        let line = run_tokens_import(&doc, &tokens, None, false, None).unwrap();
        assert!(!line.starts_with("0 added"), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An export `-o` naming the document it reads is refused and leaves it.
    ///
    /// flip: `run_tokens_export` without its document check → fails at
    /// `unwrap_err`.
    #[test]
    fn tokens_export_refuses_to_write_over_its_document() {
        let dir = std::env::temp_dir().join(format!("mastik-tokens-ex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("a.mastik");
        let blank = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(0xD9).mint(),
        ))
        .unwrap();
        std::fs::write(&doc, &blank).unwrap();
        let doc = doc.to_string_lossy().into_owned();
        let err = run_tokens_export(&doc, None, Some(&doc)).unwrap_err();
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(std::fs::read(&doc).unwrap(), blank, "the document stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `merge` takes three documents and an output, and refuses the rest.
    #[test]
    fn merge_takes_three_documents_and_an_output() {
        let merge = |s: &[&str]| {
            let mut v = vec!["merge".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert_eq!(
            merge(&["o", "a", "b", "-o", "m"]),
            Ok(Command::Merge {
                base: "o".into(),
                ours: "a".into(),
                theirs: "b".into(),
                out: Some("m".into()),
            })
        );
        for refused in [
            &["o", "a"][..],
            &["o", "a", "b", "c"],
            &["o", "a", "b", "--x"],
        ] {
            assert!(merge(refused).is_err(), "{refused:?}");
        }
    }

    /// `run_merge` writes over ours by default, as git's driver protocol has it,
    /// and answers whether the merge was clean.
    #[test]
    fn run_merge_writes_over_ours_and_answers_clean() {
        use mastik_core::{IdSource, NodeKind, Operation, Transaction};
        let dir = std::env::temp_dir().join(format!("mastik-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = IdSource::new(0xD4);
        let root = ids.mint();
        let dot = ids.mint();
        let mut doc = mastik_core::Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: dot,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: mastik_core::kurbo::Size::new(10.0, 10.0),
                arc: mastik_core::EllipseArc::default(),
            },
            transform: None,
            name: Some("Dot".into()),
        }]))
        .unwrap();
        let named = |name: &str| {
            let mut d = doc.clone();
            d.apply(&Transaction(vec![Operation::SetName {
                id: dot,
                name: name.into(),
            }]))
            .unwrap();
            mastik_core::io::save(&d).unwrap()
        };
        let write = |file: &str, bytes: &[u8]| {
            let p = dir.join(file);
            std::fs::write(&p, bytes).unwrap();
            p.to_string_lossy().into_owned()
        };
        let base = write("o.mastik", &mastik_core::io::save(&doc).unwrap());
        let ours = write("a.mastik", &named("Ours"));
        let theirs = write("b.mastik", &mastik_core::io::save(&doc).unwrap());
        assert_eq!(run_merge(&base, &ours, &theirs, None), Ok(true));
        assert_eq!(std::fs::read(&ours).unwrap(), named("Ours"));

        let theirs = write("b.mastik", &named("Theirs"));
        assert_eq!(run_merge(&base, &ours, &theirs, None), Ok(false));
        assert!(run_merge(&base, "missing.mastik", &theirs, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `tokens export` takes one document, a mode and an output, and refuses the rest.
    #[test]
    fn tokens_export_takes_one_document_a_mode_and_an_output() {
        let tokens = |s: &[&str]| {
            let mut v = vec!["tokens".to_string()];
            v.extend(s.iter().map(|a| a.to_string()));
            parse(&v)
        };
        assert_eq!(
            tokens(&["export", "a.mastik", "--mode", "Dark", "-o", "t.json"]),
            Ok(Command::TokensExport {
                file: "a.mastik".into(),
                mode: Some("Dark".into()),
                out: Some("t.json".into()),
            })
        );
        for refused in [
            &["a.mastik"][..],
            &["export"],
            &["export", "a.mastik", "b.mastik"],
            &["export", "a.mastik", "--mode"],
            &["export", "a.mastik", "-o", "--mode"],
            &["export", "a.mastik", "--svg"],
        ] {
            assert!(tokens(refused).is_err(), "{refused:?}");
        }
    }

    /// The CLI writes the file `mastik_export::dtcg` makes, and refuses a mode the
    /// document does not have, naming the ones it does.
    #[test]
    fn tokens_export_writes_the_file_and_refuses_an_unknown_mode() {
        use mastik_core::{Cell, IdSource, Mode, Operation, Transaction, VarKind, Variable};
        let dir = std::env::temp_dir().join(format!("mastik-tokens-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = IdSource::new(0xD3);
        let mut doc = mastik_core::Document::new(ids.mint());
        let (m, dark, v) = (ids.mint_mode(), ids.mint_mode(), ids.mint_var());
        doc.apply(&Transaction(vec![
            Operation::AddMode {
                collection: mastik_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: m,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddMode {
                collection: mastik_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: dark,
                    name: "Dark".into(),
                },
                index: 1,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: v,
                    name: "space/4".into(),
                    kind: VarKind::Number,
                    cells: [(m, Cell::Number(4.0)), (dark, Cell::Number(8.0))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .unwrap();
        let src = dir.join("a.mastik");
        std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
        let out = dir.join("t.json");
        let (src, out) = (src.to_str().unwrap(), out.to_str().unwrap());

        run_tokens_export(src, Some("Dark"), Some(out)).unwrap();
        let written = std::fs::read_to_string(out).unwrap();
        assert_eq!(written, mastik_export::dtcg::tokens(&doc, Some(dark)).json);
        assert_ne!(
            written,
            mastik_export::dtcg::tokens(&doc, None).json,
            "--mode is read"
        );
        let err = run_tokens_export(src, Some("Dusk"), Some(out)).unwrap_err();
        assert!(err.contains("Dusk") && err.contains("Light"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two saves of one document differ in nothing and print nothing; an edit
    /// differs and names the layer; a missing file is an error, not a difference.
    #[test]
    fn diff_files_answers_whether_two_documents_differ() {
        use mastik_core::{IdSource, NodeKind, Operation, Transaction};
        let dir = std::env::temp_dir().join(format!("mastik-diff-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = IdSource::new(0xD2);
        let root = ids.mint();
        let rect = ids.mint();
        let mut doc = mastik_core::Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Ellipse {
                size: mastik_core::kurbo::Size::new(10.0, 10.0),
                arc: mastik_core::EllipseArc::default(),
            },
            transform: None,
            name: Some("Dot".into()),
        }]))
        .unwrap();
        let a = dir.join("a.mastik");
        std::fs::write(&a, mastik_core::io::save(&doc).unwrap()).unwrap();
        doc.apply(&Transaction(vec![Operation::SetName {
            id: rect,
            name: "Spot".into(),
        }]))
        .unwrap();
        let b = dir.join("b.mastik");
        std::fs::write(&b, mastik_core::io::save(&doc).unwrap()).unwrap();
        let (a, b) = (a.to_str().unwrap(), b.to_str().unwrap());

        assert_eq!(diff_files(a, a, false), Ok((String::new(), false)));
        let (text, differs) = diff_files(a, b, false).unwrap();
        assert!(differs);
        assert!(
            text.contains("~ Spot") && text.contains("name: “Dot” → “Spot”"),
            "{text}"
        );
        let (json, _) = diff_files(a, b, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["pages"][0]["layers"][0]["status"], "changed");
        let missing = dir.join("missing.mastik");
        assert!(diff_files(a, missing.to_str().unwrap(), false).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn export(args: &[&str]) -> Result<Command, String> {
        let mut v = vec!["export".to_string()];
        v.extend(args.iter().map(|s| s.to_string()));
        parse(&v)
    }

    /// A one-off export of `file`'s page in `format` to `out`, as typed with no
    /// other flag.
    fn one_off(file: &str, format: ExportFormat, out: Option<&str>) -> OneOff {
        OneOff {
            file: file.into(),
            format,
            out: out.map(Into::into),
            scale: ExportScale::Times(1.0),
            page: None,
            text: ExportText::Live,
            sizes: Vec::new(),
            layers: Vec::new(),
            quality: None,
        }
    }

    #[test]
    fn export_defaults_to_svg() {
        assert_eq!(
            export(&["a.mastik"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Svg,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
    }

    #[test]
    fn export_accepts_a_format_and_an_output_path() {
        assert_eq!(
            export(&["a.mastik", "--png", "-o", "out.png"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Png,
                out: Some("out.png".into()),
                scale: ExportScale::Times(1.0),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
        assert_eq!(
            export(&["--json", "a.mastik"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Json,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
        assert_eq!(
            export(&["a.mastik", "--html"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Html,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
        for (flag, format) in [
            ("--react", ExportFormat::React),
            ("--vue", ExportFormat::Vue),
            ("--penpot", ExportFormat::Penpot),
            ("--sketch", ExportFormat::Sketch),
            ("--prototype", ExportFormat::Prototype),
            ("--flow-map", ExportFormat::FlowMap),
        ] {
            assert_eq!(
                export(&["a.mastik", flag]).unwrap(),
                Command::Export(OneOff {
                    file: "a.mastik".into(),
                    format,
                    out: None,
                    scale: ExportScale::Times(1.0),
                    page: None,
                    text: ExportText::Live,
                    sizes: Vec::new(),
                    layers: Vec::new(),
                    quality: None,
                })
            );
        }
    }

    /// `--ico` and `--icns` take `--sizes`, checked against what each file holds;
    /// `--sizes` on any other format, a raster size on an icon and a bad list are
    /// refused.
    #[test]
    fn an_icon_export_takes_sizes_its_file_can_hold() {
        assert!(matches!(
            export(&["a.mastik", "--ico", "--sizes", "16, 32,256"]),
            Ok(Command::Export(OneOff { format: ExportFormat::Ico, ref sizes, .. })) if sizes == &[16, 32, 256]
        ));
        assert!(matches!(
            export(&["a.mastik", "--icns"]),
            Ok(Command::Export(OneOff { format: ExportFormat::Icns, ref sizes, .. })) if sizes.is_empty()
        ));
        for refused in [
            &["a.mastik", "--icns", "--sizes", "48"][..],
            &["a.mastik", "--ico", "--sizes", "512"],
            &["a.mastik", "--ico", "--sizes", "16,x"],
            &["a.mastik", "--png", "--sizes", "16"],
            &["a.mastik", "--ico", "--scale", "2"],
            &["a.mastik", "--ico", "--outlines"],
        ] {
            assert!(export(refused).is_err(), "{refused:?}");
        }
    }

    /// A format that writes every page refuses `--page`, naming itself.
    #[test]
    fn a_whole_document_format_takes_no_page() {
        for flag in ["--json", "--penpot", "--sketch"] {
            let err = export(&["a.mastik", flag, "--page", "2"]).unwrap_err();
            assert!(
                err.starts_with(&format!("{flag} writes every page")),
                "{err}"
            );
        }
    }

    /// `--outlines` outlines an SVG's or a PDF's text and is refused anywhere it
    /// would do nothing.
    #[test]
    fn outlines_apply_to_the_vector_formats_only() {
        assert_eq!(
            export(&["a.mastik", "--pdf", "--outlines"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Pdf,
                out: None,
                scale: ExportScale::Times(1.0),
                page: None,
                text: ExportText::Outlines,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
        assert!(
            export(&["a.mastik", "--outlines"]).is_ok(),
            "SVG is the default"
        );
        for refused in [
            &["a.mastik", "--png", "--outlines"][..],
            &["a.mastik", "--all", "--outlines"],
        ] {
            assert!(export(refused).is_err(), "{refused:?}");
        }
    }

    /// `--scale` is a raster argument and is refused wherever it cannot apply
    /// rather than ignored, so a build script cannot think it asked for 2× and get 1×.
    #[test]
    fn scale_is_a_raster_argument_and_is_refused_everywhere_else() {
        assert_eq!(
            export(&["a.mastik", "--png", "--scale", "2"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Png,
                out: None,
                scale: ExportScale::Times(2.0),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );
        // Fractional, because Figma's ladder starts at 0.5×.
        assert_eq!(
            export(&["a.mastik", "--png", "--scale", "0.5"]).unwrap(),
            Command::Export(OneOff {
                file: "a.mastik".into(),
                format: ExportFormat::Png,
                out: None,
                scale: ExportScale::Times(0.5),
                page: None,
                text: ExportText::Live,
                sizes: Vec::new(),
                layers: Vec::new(),
                quality: None,
            })
        );

        // The ways it cannot apply; with no format typed the SVG default is inherited.
        for args in [
            vec!["a.mastik", "--svg", "--scale", "2"],
            vec!["a.mastik", "--json", "--scale", "2"],
            vec!["a.mastik", "--scale", "2"],
            vec!["a.mastik", "--svg", "--width", "512"],
            vec!["a.mastik", "--height", "512"],
        ] {
            let err = export(&args).unwrap_err();
            assert!(
                err.contains("applies only to --png, --jpeg and --webp"),
                "{args:?}: {err}"
            );
        }
        for raster in ["--jpeg", "--webp"] {
            assert!(
                export(&["a.mastik", raster, "--width", "512"]).is_ok(),
                "{raster} takes a size"
            );
        }
        // On `--all` the message names the flag that would have worked.
        let err = export(&["a.mastik", "--all", "--scale", "2"]).unwrap_err();
        assert!(err.contains("--scale-override"), "{err}");

        // A number that cannot be a size is a parse error, not a silent 1×.
        for bad in ["0", "-2", "abc", "", "nan"] {
            assert!(
                export(&["a.mastik", "--png", "--scale", bad]).is_err(),
                "--scale {bad:?} was accepted"
            );
        }
        for flag in ["--scale", "--width", "--height"] {
            assert!(
                export(&["a.mastik", "--png", flag]).is_err(),
                "{flag} with nothing after it"
            );
            assert!(
                export(&["a.mastik", "--png", flag, "0"]).is_err(),
                "{flag} 0 was accepted"
            );
        }
    }

    /// `--width` and `--height` pin an axis; any two of the three size flags
    /// together are refused rather than resolved by a precedence rule.
    #[test]
    fn width_and_height_are_the_other_two_ways_to_ask_and_only_one_may_be_asked() {
        let png = |args: &[&str], want: ExportScale| {
            assert_eq!(
                export(args).unwrap(),
                Command::Export(OneOff {
                    file: "a.mastik".into(),
                    format: ExportFormat::Png,
                    out: None,
                    scale: want,
                    page: None,
                    text: ExportText::Live,
                    sizes: Vec::new(),
                    layers: Vec::new(),
                    quality: None,
                }),
                "{args:?}"
            );
        };
        png(
            &["a.mastik", "--png", "--width", "512"],
            ExportScale::Width(512),
        );
        png(
            &["a.mastik", "--png", "--height", "1024"],
            ExportScale::Height(1024),
        );

        // Every pair in both orders, so a rule firing one way round fails.
        for pair in [
            ["--scale", "--width"],
            ["--width", "--scale"],
            ["--scale", "--height"],
            ["--height", "--scale"],
            ["--width", "--height"],
            ["--height", "--width"],
        ] {
            let err = export(&["a.mastik", "--png", pair[0], "2", pair[1], "2"]).unwrap_err();
            assert!(
                err.contains("two answers to one question"),
                "{pair:?}: {err}"
            );
            // The message names both flags.
            assert!(err.contains(pair[0]) && err.contains(pair[1]), "{err}");
        }

        // The same flag twice is refused too, not last-one-wins like `--png --svg`.
        assert!(
            export(&["a.mastik", "--png", "--scale", "2", "--scale", "3"]).is_err(),
            "--scale twice"
        );
    }

    /// `--scale-override` belongs to `--all` and takes the export row's own
    /// vocabulary (`ExportScale::from_label`), so one flag covers all three sizes.
    #[test]
    fn scale_override_belongs_to_all_and_speaks_the_rows_vocabulary() {
        let all = |args: &[&str], want: Option<ExportScale>| {
            assert_eq!(
                export(args).unwrap(),
                Command::ExportAll {
                    file: "a.mastik".into(),
                    out: None,
                    folders: false,
                    scale_override: want,
                    page: None,
                },
                "{args:?}"
            );
        };
        all(&["a.mastik", "--all"], None);
        all(
            &["a.mastik", "--all", "--scale-override", "1x"],
            Some(ExportScale::Times(1.0)),
        );
        all(
            &["a.mastik", "--all", "--scale-override", "2"],
            Some(ExportScale::Times(2.0)),
        );
        all(
            &["a.mastik", "--all", "--scale-override", "512w"],
            Some(ExportScale::Width(512)),
        );

        // Not on a one-off, where `--scale` is the flag.
        let err = export(&["a.mastik", "--png", "--scale-override", "2x"]).unwrap_err();
        assert!(err.contains("applies only to --all"), "{err}");

        for bad in ["", "2px", "abc", "0x", "-1"] {
            assert!(
                export(&["a.mastik", "--all", "--scale-override", bad]).is_err(),
                "--scale-override {bad:?} was accepted"
            );
        }
    }

    #[test]
    fn export_rejects_missing_input_and_bad_flags() {
        assert!(export(&[]).is_err(), "no input file");
        assert!(export(&["a.mastik", "--gif"]).is_err(), "unknown format");
        assert!(export(&["a.mastik", "-o"]).is_err(), "-o without a value");

        // flip: the unconditional `args.get(i)` in the `-o` arm → the first fails;
        // `other => file = Some(…)` without the `file.is_some()` guard → the second
        // fails. The `-o --png` case is about a silent SVG: the format flag is lost.
        assert!(
            export(&["a.mastik", "-o", "--png"]).is_err(),
            "-o swallowed the format flag"
        );
        assert!(
            export(&["a.mastik", "b.mastik"]).is_err(),
            "a second document overwrote the first"
        );
        // The second name is refused wherever it sits.
        assert!(
            export(&["a.mastik", "--png", "b.mastik", "-o", "out.png"]).is_err(),
            "a second document after the flags"
        );
    }

    /// The headless path end to end: no window, no GPU, no font service.
    #[test]
    fn export_writes_each_format_without_a_display() {
        use mastik_core::kurbo::Size;
        use mastik_core::{Document, Fill, IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(0xE7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(32.0, 24.0),
                    corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: ab,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: mastik_core::peniko::Brush::Solid(
                        mastik_core::peniko::Color::from_rgba8(10, 20, 30, 255),
                    )
                    .into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();

        let dir = std::env::temp_dir().join(format!("mastik-export-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.mastik");
        std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
        let src = src.to_string_lossy().into_owned();

        for (format, ext) in [
            (ExportFormat::Svg, "svg"),
            (ExportFormat::Pdf, "pdf"),
            (ExportFormat::PdfFrames, "frames.pdf"),
            (ExportFormat::Png, "png"),
            (ExportFormat::Jpeg, "jpg"),
            (ExportFormat::Webp, "webp"),
            (ExportFormat::Ico, "ico"),
            (ExportFormat::Icns, "icns"),
            (ExportFormat::Json, "json"),
            (ExportFormat::Html, "zip"),
            (ExportFormat::React, "zip"),
            (ExportFormat::Vue, "zip"),
        ] {
            let out = dir.join(format!("out.{ext}"));
            run_export(&one_off(&src, format, Some(&out.to_string_lossy())))
                .expect("export succeeds");
            let written = std::fs::read(&out).expect("output exists");
            assert!(!written.is_empty(), "{ext} export was empty");
        }

        // PNG really is a PNG, not an empty stub.
        let png = std::fs::read(dir.join("out.png")).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");
        // And the zip a zip.
        let zip = std::fs::read(dir.join("out.zip")).unwrap();
        assert_eq!(&zip[..4], b"PK\x03\x04", "zip signature");
        // A JPEG a JPEG, and a WebP the page's pixels exactly, being lossless.
        let jpg = std::fs::read(dir.join("out.jpg")).unwrap();
        assert_eq!(&jpg[..3], b"\xff\xd8\xff", "JPEG signature");
        let webp = image::load_from_memory(&std::fs::read(dir.join("out.webp")).unwrap())
            .unwrap()
            .to_rgba8();
        let png = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(webp.dimensions(), png.dimensions());
        assert!(webp == png, "the WebP draws what the PNG does");
    }

    /// `--layer` repeats, `--quality` is JPEG's, and a layer is refused where the
    /// format writes whole pages; an icon takes one.
    #[test]
    fn export_takes_layers_and_a_quality() {
        assert!(matches!(
            export(&["a.mastik", "--png", "--layer", "Logo", "--layer", "5a:2"]),
            Ok(Command::Export(OneOff { ref layers, .. })) if layers == &["Logo", "5a:2"]
        ));
        assert!(matches!(
            export(&["a.mastik", "--jpeg", "--quality", "70"]),
            Ok(Command::Export(OneOff {
                format: ExportFormat::Jpeg,
                quality: Some(70),
                ..
            }))
        ));
        assert!(export(&["a.mastik", "--ico", "--layer", "Logo"]).is_ok());
        for refused in [
            &["a.mastik", "--layer"][..],
            &["a.mastik", "--layer", "--png"],
            &["a.mastik", "--png", "--quality", "70"],
            &["a.mastik", "--jpeg", "--quality", "0"],
            &["a.mastik", "--jpeg", "--quality", "101"],
            &["a.mastik", "--json", "--layer", "Logo"],
            &["a.mastik", "--sketch", "--layer", "Logo"],
            &["a.mastik", "--pdf", "--frames", "--layer", "Logo"],
            &["a.mastik", "--ico", "--layer", "A", "--layer", "B"],
            &["a.mastik", "--all", "--layer", "Logo"],
        ] {
            assert!(export(refused).is_err(), "{refused:?}");
        }
    }

    /// A page holding Card (a 40 × 20 frame) and two 10 × 10 rects named Dot, one
    /// inside Card and one beside it, on two pages: Card's on the first.
    fn layered(dir: &std::path::Path) -> (String, [mastik_core::NodeId; 4]) {
        use mastik_core::kurbo::{Affine, Size};
        use mastik_core::{Document, IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0x1A7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (card, inner, outer, other) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let page2 = ids.mint();
        let rect = |id, parent, at: (f64, f64), name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(at)),
            name: Some(name.into()),
        };
        let black = |id| Operation::SetFills {
            id,
            fills: mastik_core::keyed_by_position([mastik_core::Fill {
                brush: mastik_core::peniko::Brush::Solid(mastik_core::peniko::Color::BLACK).into(),
                visible: true,
            }]),
        };
        let page = doc.first_page();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: card,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Card".into()),
            },
            rect(inner, card, (5.0, 5.0), "Dot"),
            black(inner),
            rect(outer, page, (100.0, 0.0), "Dot"),
            black(outer),
            Operation::AddPage {
                id: page2,
                index: 1,
                name: "Two".into(),
                ground: None,
                draft: false,
                modes: Vec::new(),
                flows: Vec::new(),
                play: Default::default(),
            },
            rect(other, page2, (0.0, 0.0), "Other"),
            black(other),
        ]))
        .unwrap();
        let file = dir.join("layered.mastik");
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        (
            file.to_string_lossy().into_owned(),
            [card, inner, outer, other],
        )
    }

    /// `--layer` finds a layer by id or by a name only it has, scoped by `--page`;
    /// a shared name, a page's id, a missing name and layers on two pages are
    /// refused; a layer inside another asked for is drawn once, with it.
    #[test]
    fn layers_are_chosen_by_id_or_a_name_of_their_own() {
        let dir = std::env::temp_dir().join(format!("mastik-layers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (file, [card, inner, outer, other]) = layered(&dir);
        let doc = mastik_core::io::load(&std::fs::read(&file).unwrap()).unwrap();
        let pick = |asked: &[&str], page: Option<&str>| {
            let asked: Vec<String> = asked.iter().map(|a| a.to_string()).collect();
            chosen_layers(&doc, &asked, page)
        };
        assert_eq!(pick(&["Card"], None), Ok(vec![card]));
        assert_eq!(pick(&[&outer.to_wire()], None), Ok(vec![outer]));
        assert_eq!(pick(&["Other"], Some("Two")), Ok(vec![other]));
        assert_eq!(
            pick(&["Card", &inner.to_wire()], None),
            Ok(vec![card]),
            "the dot inside Card is drawn with it"
        );
        let shared = pick(&["Dot"], None).unwrap_err();
        assert!(shared.starts_with("2 layers are named \"Dot\""), "{shared}");
        assert!(shared.contains(&inner.to_wire()) && shared.contains(&outer.to_wire()));
        assert!(pick(&["Other"], Some("1")).is_err(), "Other is on page Two");
        assert!(pick(&["Nothing"], None).is_err());
        assert!(pick(&[&doc.first_page().to_wire()], None).is_err());
        assert!(pick(&["Card", "Other"], None).is_err(), "two pages");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An export of one layer is framed on that layer alone, whatever else the
    /// page holds, in every raster format.
    ///
    /// flip: `run_export` drawing the page whatever `--layer` asks → fails at
    /// "framed on Card".
    #[test]
    fn a_layer_exports_on_its_own() {
        let dir = std::env::temp_dir().join(format!("mastik-layer-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (file, _) = layered(&dir);
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp] {
            let out = dir.join(format!("card.{}", format.extension()));
            run_export(&OneOff {
                layers: vec!["Card".into()],
                scale: ExportScale::Times(2.0),
                ..one_off(&file, format, Some(&out.to_string_lossy()))
            })
            .unwrap();
            let img = image::load_from_memory(&std::fs::read(&out).unwrap()).unwrap();
            assert_eq!(
                (img.width(), img.height()),
                (80, 40),
                "framed on Card, {format:?}"
            );
        }
        let svg = dir.join("card.svg");
        run_export(&OneOff {
            layers: vec!["Card".into()],
            ..one_off(&file, ExportFormat::Svg, Some(&svg.to_string_lossy()))
        })
        .unwrap();
        let svg = std::fs::read_to_string(svg).unwrap();
        assert!(svg.contains("viewBox=\"0 0 40 20\""), "{svg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `mastik export --png` leaves out a frame's fill flagged out of exports, as
    /// the app's exports do; the flag-off control draws it.
    ///
    /// flip: `run_export` drawing the document as it is (`draws` false) → fails
    /// at "no ground".
    #[test]
    fn export_leaves_a_flagged_fill_out() {
        use mastik_core::kurbo::Size;
        use mastik_core::{Document, Fill, IdSource, NodeKind, Operation, Transaction};

        let dir = std::env::temp_dir().join(format!("mastik-export-flag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for hidden in [true, false] {
            let mut ids = IdSource::new(0xE8);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (frame, dot) = (ids.mint(), ids.mint());
            let fill = |id, color| Operation::SetFills {
                id,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: mastik_core::peniko::Brush::Solid(color).into(),
                    visible: true,
                }]),
            };
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: frame,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(16.0, 16.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: None,
                },
                fill(frame, mastik_core::peniko::Color::WHITE),
                Operation::CreateNode {
                    id: dot,
                    parent: frame,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(4.0, 4.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: None,
                },
                fill(dot, mastik_core::peniko::Color::BLACK),
                Operation::SetFillHiddenInExport { id: frame, hidden },
            ]))
            .unwrap();
            let src = dir.join(format!("flag-{hidden}.mastik"));
            std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
            let out = dir.join(format!("flag-{hidden}.png"));
            run_export(&one_off(
                &src.to_string_lossy(),
                ExportFormat::Png,
                Some(&out.to_string_lossy()),
            ))
            .expect("export succeeds");
            let png = image::load_from_memory(&std::fs::read(&out).unwrap())
                .unwrap()
                .to_rgba8();
            assert_eq!(png.width(), 16, "the fixture: the page is the frame");
            let want = if hidden { 0 } else { 255 };
            assert_eq!(
                png.get_pixel(12, 12).0[3],
                want,
                "no ground, hidden={hidden}"
            );
        }
    }

    /// `--scale 2` doubles both sides of the raster, read out of the PNG header so
    /// the assertion survives the whole write.
    ///
    /// flip: `run_export` pinned back to `viewport_over` → fails at `2× is not
    /// twice 1×`, with both files identical.
    ///
    /// flip: `factor`'s `Width`/`Height` arms exchanged → fails on the `Width` line,
    /// `(128, 96)` against `(96, 72)`. It bites only because the fixture is 32×24.
    #[test]
    fn scale_multiplies_the_pixels_the_png_actually_carries() {
        use mastik_core::kurbo::Size;
        use mastik_core::{Document, Fill, IdSource, NodeKind, Operation, Transaction};

        let mut ids = IdSource::new(0x5CA1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(32.0, 24.0),
                    corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: ab,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: mastik_core::peniko::Brush::Solid(
                        mastik_core::peniko::Color::from_rgba8(10, 20, 30, 255),
                    )
                    .into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();

        // Process-unique, so a second `cargo test` on the same machine cannot read
        // a half-written file and fail like an export defect.
        let dir = std::env::temp_dir().join(format!("mastik-export-scale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.mastik");
        std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
        let src = src.to_string_lossy().into_owned();

        let sides = |scale: ExportScale, name: &str| {
            let out = dir.join(name);
            run_export(&OneOff {
                scale,
                ..one_off(&src, ExportFormat::Png, Some(&out.to_string_lossy()))
            })
            .expect("export succeeds");
            let png = std::fs::read(&out).unwrap();
            // IHDR's width and height, big-endian, at a fixed offset in every PNG.
            let n = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
            (n(16), n(20))
        };

        let (w1, h1) = sides(ExportScale::Times(1.0), "one.png");
        let (w2, h2) = sides(ExportScale::Times(2.0), "two.png");
        assert!(
            w1 > 1 && h1 > 1,
            "the fixture has no size to double: {w1}×{h1}"
        );
        assert_eq!((w2, h2), (w1 * 2, h1 * 2), "2× is not twice 1×");

        // The pinned axis answers and the other follows: the fixture is 32×24, so 96
        // wide is 3× and 72 tall, and 96 tall is 4× and 128 wide. Asserting both
        // sides of both catches swapped axes.
        assert_eq!(sides(ExportScale::Width(96), "w.png"), (96, 72));
        assert_eq!(sides(ExportScale::Height(96), "h.png"), (128, 96));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The ad-hoc path reports a clamp (`viewport_at`'s), and stays silent on a floor.
    ///
    /// The third case's flip does not bite: `>=` to `==` in `clamp_note` changes
    /// nothing, because `px` takes `ceil` of the same positive product (0.01× of
    /// 100×50 asks for 1×1 and gets 1×1). It pins only that a sub-pixel scale is silent.
    #[test]
    fn a_clamped_scale_is_reported_and_a_floored_one_is_not() {
        use mastik_core::kurbo::Rect;
        let view = Rect::new(0.0, 0.0, 100.0, 50.0);

        // Served exactly: nothing to say.
        let limit = mastik_export::png::MAX_RASTER_SIDE;
        assert_eq!(
            clamp_note(view, ExportScale::Times(2.0), (200, 100), limit),
            None
        );

        // Past `MAX_RASTER_SIDE` on the long side; checked against what
        // `viewport_at` actually returned.
        let big = ExportScale::Times(2000.0);
        let vp = mastik_export::png::viewport_at(view, big.factor(100.0, 50.0));
        let note = clamp_note(view, big, vp.pixel_size, limit).expect("2000× is past the limit");
        assert!(
            note.contains(&format!(
                "past the {} limit",
                mastik_export::png::MAX_RASTER_SIDE
            )),
            "{note}"
        );
        // Named in the vocabulary that was typed, not the multiplier it became.
        assert!(note.starts_with("2000x "), "{note}");
        // The effective multiplier, derived from the cap so a change to
        // `MAX_RASTER_SIDE` moves it too.
        assert!(
            note.contains(&format!(
                "{:.3}×",
                f64::from(mastik_export::png::MAX_RASTER_SIDE) / 100.0
            )),
            "{note}"
        );
        // Proportions kept, within a pixel per side rather than exactly: both sides
        // are `ceil`ed independently after the float multiplier, so `h * 2 == w`
        // would be wrong.
        assert!(
            vp.pixel_size.1 * 2 - vp.pixel_size.0 <= 2,
            "aspect drifted: {:?}",
            vp.pixel_size
        );

        // A pinned axis says so in its own words, which is why `clamp_note` takes the
        // spec and not the factor it resolved to.
        let wide = ExportScale::Width(200_000);
        let vp = mastik_export::png::viewport_at(view, wide.factor(100.0, 50.0));
        let note = clamp_note(view, wide, vp.pixel_size, limit).expect("200000w is past the limit");
        assert!(note.starts_with("200000w "), "{note}");

        // Floored, not clamped: every side is lifted to a whole pixel, and silent.
        let vp = mastik_export::png::viewport_at(view, 0.01);
        assert_eq!(vp.pixel_size, (1, 1));
        assert_eq!(
            clamp_note(view, ExportScale::Times(0.01), vp.pixel_size, limit),
            None
        );
    }

    /// `-o` naming the document is refused and leaves it as it was.
    ///
    /// flip: `run_export` without its same-file check → the document is a zip.
    #[test]
    fn export_refuses_to_write_over_its_document() {
        let dir = std::env::temp_dir().join(format!("mastik-export-self-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.mastik");
        let saved = mastik_core::io::save(&mastik_core::Document::new(
            mastik_core::IdSource::new(7).mint(),
        ))
        .unwrap();
        std::fs::write(&src, &saved).unwrap();
        let src = src.to_string_lossy().into_owned();
        let err = run_export(&one_off(&src, ExportFormat::Vue, Some(&src))).unwrap_err();
        assert!(err.contains("refusing"), "{err}");
        assert_eq!(
            std::fs::read(&src).unwrap(),
            saved,
            "the document is untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_reports_a_missing_file_instead_of_panicking() {
        let err =
            run_export(&one_off("does-not-exist.mastik", ExportFormat::Svg, None)).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    /// `--all` is its own command; a format or `--folders` without it is refused
    /// rather than ignored.
    #[test]
    fn export_all_is_a_command_of_its_own() {
        assert_eq!(
            export(&["a.mastik", "--all"]).unwrap(),
            Command::ExportAll {
                file: "a.mastik".into(),
                out: None,
                folders: false,
                scale_override: None,
                page: None,
            }
        );
        assert_eq!(
            export(&["a.mastik", "--all", "--folders", "-o", "assets"]).unwrap(),
            Command::ExportAll {
                file: "a.mastik".into(),
                out: Some("assets".into()),
                folders: true,
                scale_override: None,
                page: None,
            }
        );
        assert!(
            export(&["a.mastik", "--all", "--png"]).is_err(),
            "--all takes its formats from the document"
        );
        assert!(
            export(&["a.mastik", "--folders"]).is_err(),
            "--folders means nothing without --all"
        );
    }

    /// `--all` runs the document's own settings headlessly. Asserts the filenames
    /// (suffix rule, extension per format, de-collision), not just that files
    /// appeared; the last two thirds cover `--scale-override` on the same fixture.
    ///
    /// flip: dropping the dedupe in the plan → fails at the override's names,
    /// leaving `["Close (2).png", "Close.png", "Close.svg"]`.
    #[test]
    fn export_all_runs_the_documents_own_settings() {
        use mastik_core::kurbo::Size;
        use mastik_core::{
            Document, ExportFormat as Fmt, ExportScale, ExportSpec, IdSource, NodeKind, Operation,
            Transaction,
        };

        let mut ids = IdSource::new(0xA11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(64.0, 64.0),
                    corner_radii: mastik_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Board".into()),
            },
            Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(24.0, 24.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Close".into()),
            },
            Operation::SetExports {
                id: rect,
                exports: mastik_core::keyed_by_position([
                    ExportSpec::new(Fmt::Png, ExportScale::Times(1.0)),
                    ExportSpec::new(Fmt::Png, ExportScale::Times(2.0)),
                    ExportSpec::new(Fmt::Svg, ExportScale::Times(1.0)),
                ]),
            },
        ]))
        .unwrap();

        // Process-unique: this one opens by deleting the directory.
        let dir = std::env::temp_dir().join(format!("mastik-export-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.mastik");
        std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
        let out = dir.join("assets");

        run_export_all(
            &src.to_string_lossy(),
            Some(&out.to_string_lossy()),
            false,
            None,
            None,
        )
        .expect("export --all succeeds");

        let mut names: Vec<String> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["Close.png", "Close.svg", "Close@2x.png"]);
        // The 2× really is twice as wide, which the filenames cannot say.
        let big = std::fs::read(out.join("Close@2x.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            48,
            "a 24pt layer at 2× is 48 pixels"
        );

        // `--scale-override 1x` collapses the set: the three specs become a PNG, a
        // PNG and an SVG, two of them the same file, so two files are written.
        let flat = dir.join("flat");
        run_export_all(
            &src.to_string_lossy(),
            Some(&flat.to_string_lossy()),
            false,
            Some(ExportScale::Times(1.0)),
            None,
        )
        .expect("export --all --scale-override succeeds");
        let mut names: Vec<String> = std::fs::read_dir(&flat)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["Close.png", "Close.svg"],
            "the override should have collapsed the 1× and 2× rows, not de-collided them"
        );
        let small = std::fs::read(flat.join("Close.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(small[16..20].try_into().unwrap()),
            24,
            "forced to 1×, a 24pt layer is 24 pixels"
        );

        // A pinned axis overrides just as well: across a set, "everything 96 wide"
        // has an answer.
        let wide = dir.join("wide");
        run_export_all(
            &src.to_string_lossy(),
            Some(&wide.to_string_lossy()),
            false,
            Some(ExportScale::Width(96)),
            None,
        )
        .expect("export --all --scale-override 96w succeeds");
        let big = std::fs::read(wide.join("Close@96w.png")).unwrap();
        assert_eq!(
            u32::from_be_bytes(big[16..20].try_into().unwrap()),
            96,
            "96w means 96 pixels wide whatever the layer's own size"
        );

        // A document with no export settings fails: a build step that writes
        // nothing must not exit 0.
        let bare = dir.join("bare.mastik");
        std::fs::write(&bare, mastik_core::io::save(&Document::new(root)).unwrap()).unwrap();
        let err = run_export_all(&bare.to_string_lossy(), None, false, None, None).unwrap_err();
        assert!(err.contains("no layer has export settings"), "{err}");
    }

    /// A document whose every export file is skipped fails too, and creates no
    /// output directory. The earlier gate counts layers; this counts files produced.
    ///
    /// The spec has to be raster: `plan::render`'s SVG branch always answers
    /// `Some`, so an SVG spec on the same empty group would pass under the defect.
    ///
    /// flip: removing the `written.is_empty()` branch → fails at `expect_err`.
    #[test]
    fn an_export_all_that_skips_every_file_fails_rather_than_exiting_zero() {
        use mastik_core::{
            Document, ExportFormat as Fmt, ExportScale, ExportSpec, IdSource, NodeKind, Operation,
            Transaction,
        };
        let mut ids = IdSource::new(0xE0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let empty = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: empty,
                parent: root,
                index: 0,
                // A group with no children has no extent, which is what makes
                // every raster spec on it render `None`.
                kind: NodeKind::Group,
                transform: None,
                name: Some("Empty".into()),
            },
            Operation::SetExports {
                id: empty,
                exports: mastik_core::keyed_by_position([ExportSpec::new(
                    Fmt::Png,
                    ExportScale::Times(1.0),
                )]),
            },
        ]))
        .unwrap();

        let dir =
            std::env::temp_dir().join(format!("mastik-export-skipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("doc.mastik");
        std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
        let out = dir.join("assets");

        let err = run_export_all(
            &src.to_string_lossy(),
            Some(&out.to_string_lossy()),
            false,
            None,
            None,
        )
        .expect_err("every file skipped is a failure");
        assert!(err.contains("every export was skipped"), "{err}");
        assert!(
            !out.exists(),
            "and nothing was created, which is the half stderr could not say"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--all` says how many files it replaced. The count is asserted on
    /// `write_planned`, since the run only prints it. The second `write_planned`
    /// shows a re-run legitimately replaces everything it wrote last time.
    ///
    /// flip: deleting `if path.exists() { replaced += 1 }` → fails at the first
    /// `write_planned` count, 0 against 1.
    #[test]
    fn export_all_says_how_many_files_it_replaced() {
        let out = std::path::Path::new("out");
        assert_eq!(export_all_report(1, 0, 0, out), "wrote 1 file to out");
        assert_eq!(
            export_all_report(4, 2, 0, out),
            "wrote 4 files to out (2 replaced)"
        );
        assert_eq!(
            export_all_report(4, 2, 1, out),
            "wrote 4 files to out (2 replaced) (1 skipped)",
            "both counts, and in the order they happened"
        );

        let dir = std::env::temp_dir().join(format!("mastik-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // A hand-authored file where a planned export lands, and a second planned
        // file in a folder that does not exist yet.
        let decoy = dir.join("logo.svg");
        std::fs::write(&decoy, b"hand-authored").unwrap();
        let files = vec![
            (decoy.clone(), b"exported".to_vec()),
            (dir.join("icons").join("close.png"), b"png".to_vec()),
        ];

        assert_eq!(
            write_planned(&files).expect("writes"),
            1,
            "one of the two paths already held a file"
        );
        assert_eq!(
            std::fs::read(&decoy).unwrap(),
            b"exported",
            "and it really is gone — the count is the only warning there is"
        );
        assert!(
            dir.join("icons").is_dir(),
            "the `/` in a name is still a folder the writer creates"
        );

        assert_eq!(
            write_planned(&files).expect("writes again"),
            2,
            "a re-run replaces everything it wrote last time, which is ordinary"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--all` reads the pictures beside a linked table for a per-row export,
    /// and counts a cell naming none.
    ///
    /// flip: `export_all` not calling `with_row_pictures` → fails at "found
    /// beside the table".
    #[test]
    fn export_all_reads_the_pictures_beside_a_linked_table() {
        use mastik_core::chart::{Column, DataLink, Table};
        use mastik_core::kurbo::Size;
        use mastik_core::list::DataSet;
        use mastik_core::{
            Document, ExportFormat as Fmt, ExportScale, ExportSpec, Fill, IdSource, NodeKind,
            Operation, PerRow, Transaction,
        };

        let dir = std::env::temp_dir().join(format!("mastik-all-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = app::rgba_to_png(&[0, 255, 0, 255], (1, 1)).unwrap();
        std::fs::write(dir.join("ada.png"), png).unwrap();

        let run = |cell: &str| {
            let mut ids = IdSource::new(0xA12);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (frame, photo, table) = (ids.mint(), ids.mint(), ids.mint_data_set());
            let mut t = Table::default();
            let column = t.next_column_id();
            t.columns.push(Column {
                id: column,
                name: "Photo".into(),
            });
            t.rows = vec![vec![cell.to_string()]];
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: frame,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(16.0, 16.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: Some("Card".into()),
                },
                Operation::CreateNode {
                    id: photo,
                    parent: frame,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(16.0, 16.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: Some("Photo".into()),
                },
                Operation::SetFills {
                    id: photo,
                    fills: mastik_core::keyed_by_position([Fill {
                        brush: mastik_core::image_brush(mastik_core::ImageId("seal".into())).into(),
                        visible: true,
                    }]),
                },
                Operation::SetDataSet {
                    id: table,
                    set: Some(Box::new(DataSet {
                        id: table,
                        name: "Speakers".into(),
                        table: t,
                        source: Some(DataLink {
                            path: dir.join("speakers.csv").to_string_lossy().into_owned(),
                            size: 0,
                            modified: 0,
                            header: None,
                        }),
                        pictures: Vec::new(),
                    })),
                },
                Operation::SetExports {
                    id: frame,
                    exports: mastik_core::keyed_by_position([ExportSpec {
                        per_row: Some(PerRow::new(table)),
                        ..ExportSpec::new(Fmt::Png, ExportScale::Times(1.0))
                    }]),
                },
            ]))
            .unwrap();
            let src = dir.join("doc.mastik");
            std::fs::write(&src, mastik_core::io::save(&doc).unwrap()).unwrap();
            let out = dir.join("out");
            export_all(
                &src.to_string_lossy(),
                Some(&out.to_string_lossy()),
                false,
                None,
                None,
            )
            .expect("export --all succeeds")
        };
        let found = run("ada.png");
        assert!(
            !found.contains("not found"),
            "found beside the table: {found}"
        );
        let missing = run("nobody.png");
        assert!(
            missing.ends_with("(1 picture not found)"),
            "a cell naming no file is counted: {missing}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
