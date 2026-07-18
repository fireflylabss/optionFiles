use std::path::PathBuf;

use clap::{
    Parser, Subcommand,
    builder::styling::{AnsiColor, Effects, Styles},
};

fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::White.on_default() | Effects::BOLD)
        .usage(AnsiColor::White.on_default() | Effects::BOLD)
        .literal(AnsiColor::BrightWhite.on_default())
        .placeholder(AnsiColor::BrightBlack.on_default())
        .error(AnsiColor::BrightRed.on_default() | Effects::BOLD)
}

/// optionFiles — minimal terminal file manager
#[derive(Debug, Parser)]
#[command(
    name = "optionfiles",
    version,
    about = "◆ optionFiles — minimal terminal file manager",
    long_about = "optionFiles (option files) — browse and manage local files without leaving the terminal.\n\n  binaries   optionfiles · fls\n  interface  keyboard · mouse · preview",
    after_help = "Examples:\n  fls\n  fls ~/Downloads\n  fls open .\n  fls info archive.zip\n  fls list ~/Music --all",
    styles = styles()
)]
pub struct Cli {
    /// Directory to open (shorthand for `open PATH`)
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,

    /// Show hidden files on startup
    #[arg(short = 'a', long, global = true)]
    pub all: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Open the interactive file manager
    #[command(visible_alias = "o")]
    Open {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Print entries without opening the interface
    #[command(visible_alias = "ls")]
    List {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show file or directory information
    #[command(visible_alias = "i")]
    Info { path: PathBuf },
    /// Print a directory tree
    #[command(visible_alias = "t")]
    Tree {
        #[arg(default_value = ".")]
        path: PathBuf,

        /// Maximum directory depth
        #[arg(short, long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=20))]
        depth: u8,
    },
}
