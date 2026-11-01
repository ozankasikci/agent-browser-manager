mod browser;
mod cdp;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Profile {
    Personal,
    Business,
    Development,
}
impl Profile {
    pub const ALL: [Self; 3] = [Self::Personal, Self::Business, Self::Development];
    pub fn name(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Business => "business",
            Self::Development => "development",
        }
    }
    pub fn port(self) -> u16 {
        match self {
            Self::Personal => 9222,
            Self::Business => 9223,
            Self::Development => 9224,
        }
    }
    pub fn endpoint(self) -> String {
        format!("http://127.0.0.1:{}", self.port())
    }
}

#[derive(Parser)]
#[command(
    version,
    about = "Manage dedicated local Chrome profiles and session-owned MCP tabs"
)]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Absolute private runtime directory; defaults to ~/Library/Application Support/Agent Browser Manager"
    )]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Initialize three empty private profile directories. Never imports existing Chrome data.
    Init,
    /// Launch one dedicated Chrome profile, or verify that its managed process is already running.
    Start {
        profile: Profile,
        #[arg(long)]
        chrome: Option<PathBuf>,
        #[arg(long, help = "Use Chrome's headless mode for local verification")]
        headless: bool,
    },
    /// Close only the verified managed Chrome process for this profile. Preserves all profile data.
    Stop { profile: Profile },
    /// Inspect managed state without printing tabs, URLs, cookies, or browser websocket identifiers.
    Status { profile: Option<Profile> },
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = store::Store::new(cli.data_dir)?;
    let result = match cli.command {
        Action::Init => {
            store.init()?;
            json!({"initialized":true,"data_dir":store.root,"profiles":Profile::ALL.map(Profile::name)})
        }
        Action::Start {
            profile,
            chrome,
            headless,
        } => browser::start(&store, profile, chrome.as_deref(), headless)?,
        Action::Stop { profile } => browser::stop(&store, profile)?,
        Action::Status { profile } => {
            let profiles = profile.map_or_else(|| Profile::ALL.to_vec(), |p| vec![p]);
            let status: Result<Vec<_>> = profiles
                .into_iter()
                .map(|p| browser::status(&store, p))
                .collect();
            json!(status?)
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("abm: {e:#}");
        std::process::exit(1);
    }
}
