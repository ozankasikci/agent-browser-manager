mod browser;
mod cdp;
mod config;
mod mcp;
mod ownership;
mod profile;
mod store;
mod transfer;

use anyhow::Result;
use clap::{Parser, Subcommand};
use profile::Profile;
use serde_json::json;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Manage named persistent Chrome profiles and session-owned MCP tabs"
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
    /// Initialize default private profile directories without importing existing Chrome data.
    Init,
    /// Launch a named profile, creating it with a stable localhost port if it is new.
    Start {
        profile: String,
        #[arg(long)]
        chrome: Option<PathBuf>,
        #[arg(long)]
        headless: bool,
    },
    /// Close only the verified managed process for this profile. Preserve profile data.
    Stop { profile: String },
    /// Inspect one profile, or all default and registered custom profiles.
    Status { profile: Option<String> },
    /// Create or list named persistent profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileAction,
    },
    /// Run the Rust stdio MCP server for an existing profile. Does not auto-start Chrome.
    Mcp {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        agent: String,
    },
    /// Print config without modifying agent settings. Defaults to development.
    Config {
        client: config::Client,
        #[arg(long, conflicts_with = "all_profiles")]
        profile: Option<String>,
        #[arg(long)]
        all_profiles: bool,
        #[arg(long)]
        agent: Option<String>,
    },
}
#[derive(Subcommand)]
enum ProfileAction {
    /// Create a persistent named profile. Its port remains stable after registration.
    Create {
        name: String,
        #[arg(long, help = "Optional unused localhost port in 9225..=9324")]
        port: Option<u16>,
    },
    /// List all registered profiles and their localhost endpoints without launching browsers.
    List,
    /// Remove a stopped profile from active use and archive its data privately.
    Remove { name: String },
    /// Encrypt the whole Chrome profile and portable login cookies.
    Export {
        name: String,
        #[arg(
            long,
            help = "Capture live cookies, then close the running profile for a consistent snapshot"
        )]
        stop: bool,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
        #[arg(long,default_value_t=transfer::DEFAULT_MAX_BYTES)]
        max_bytes: u64,
        #[arg(long)]
        chrome: Option<PathBuf>,
    },
    /// Import an encrypted transfer into a new profile without overwriting data.
    Import {
        archive: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
        #[arg(long,default_value_t=transfer::DEFAULT_MAX_BYTES)]
        max_bytes: u64,
        #[arg(long)]
        chrome: Option<PathBuf>,
    },
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    let store = store::Store::new(cli.data_dir)?;
    let result = match cli.command {
        Action::Init => {
            store.init()?;
            json!({"initialized":true,"data_dir":store.root,"profiles":store.profiles()?.iter().map(Profile::name).collect::<Vec<_>>()})
        }
        Action::Start {
            profile,
            chrome,
            headless,
        } => {
            let profile = store.create_profile(&profile, None)?;
            browser::start(&store, &profile, chrome.as_deref(), headless)?
        }
        Action::Stop { profile } => browser::stop(&store, &store.resolve_profile(&profile)?)?,
        Action::Status { profile } => {
            let profiles = match profile {
                Some(name) => vec![store.resolve_profile(&name)?],
                None => store.profiles()?,
            };
            let values: Result<Vec<_>> = profiles
                .iter()
                .map(|p| browser::status(&store, p))
                .collect();
            json!(values?)
        }
        Action::Profile {
            command: ProfileAction::Create { name, port },
        } => {
            let p = store.create_profile(&name, port)?;
            json!({"profile":p.name(),"port":p.port(),"endpoint":p.endpoint()})
        }
        Action::Profile {
            command: ProfileAction::Remove { name },
        } => {
            let archive = store.remove_profile(&name)?;
            json!({"profile":name,"removed":true,"archive":archive})
        }
        Action::Profile {
            command:
                ProfileAction::Export {
                    name,
                    stop,
                    output,
                    passphrase_file,
                    max_bytes,
                    chrome,
                },
        } => transfer::export(
            &store,
            &name,
            &output,
            transfer::passphrase(passphrase_file.as_deref(), true)?,
            max_bytes,
            chrome.as_deref(),
            stop,
        )?,
        Action::Profile {
            command:
                ProfileAction::Import {
                    archive,
                    name,
                    port,
                    passphrase_file,
                    max_bytes,
                    chrome,
                },
        } => transfer::import(
            &store,
            &archive,
            name.as_deref(),
            port,
            transfer::passphrase(passphrase_file.as_deref(), false)?,
            max_bytes,
            chrome.as_deref(),
        )?,
        Action::Profile {
            command: ProfileAction::List,
        } => json!(
            store
                .profiles()?
                .iter()
                .map(|p| json!({"profile":p.name(),"port":p.port(),"endpoint":p.endpoint()}))
                .collect::<Vec<_>>()
        ),
        Action::Mcp { profile, agent } => {
            return mcp::serve(mcp::Server::new(
                store.clone(),
                store.resolve_profile(&profile)?,
                agent,
            )?);
        }
        Action::Config {
            client,
            profile,
            all_profiles,
            agent,
        } => {
            let profiles = if all_profiles {
                store.profiles()?
            } else {
                vec![store.resolve_profile(profile.as_deref().unwrap_or("development"))?]
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
