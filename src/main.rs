mod browser;
mod cdp;
mod config;
mod mcp;
mod ownership;
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
    /// Run the Rust stdio MCP server. Does not auto-start Chrome or adopt existing tabs.
    Mcp {
        #[arg(long)]
        profile: Profile,
        #[arg(long)]
        agent: String,
    },
    /// Print configuration for review. Does not modify existing agent settings. Defaults to development.
    Config {
        client: config::Client,
        #[arg(long, conflicts_with = "all_profiles")]
        profile: Option<Profile>,
        #[arg(long)]
        all_profiles: bool,
        #[arg(long)]
        agent: Option<String>,
    },
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
        Action::Mcp { profile, agent } => {
            return mcp::serve(mcp::Server::new(store, profile, agent)?);
        }
        Action::Config {
            client,
            profile,
            all_profiles,
            agent,
        } => {
            let profiles = if all_profiles {
                Profile::ALL.to_vec()
            } else {
                vec![profile.unwrap_or(Profile::Development)]
            };
            print!(
                "{}",
                config::render(
                    client,
                    &profiles,
                    &std::env::current_exe()?,
                    &store,
                    &agent.unwrap_or_else(|| client.name().into())
                )?
            );
            return Ok(());
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
