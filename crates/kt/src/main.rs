mod cli;
mod discovery;
mod error;
mod git;
mod install_channel;
mod install_target;
mod lockfile;
mod manifest;
mod skill;
mod skills_sh;
mod ui;
mod update_check;

use clap::{CommandFactory, Parser, Subcommand};

const HELP_FOOTER: &str = concat!(
    "License: ",
    env!("CARGO_PKG_LICENSE"),
    "\nRepository: ",
    env!("CARGO_PKG_REPOSITORY")
);

const INIT_AFTER_HELP: &str = "\
Details:
  Creates a manifest with dependencies and publish fields. Existing
  .agents/skills directories are adopted as dependencies: known public skills
  are resolved to remote dependencies when possible, and unmatched skills stay
  as local path dependencies. Adoption reports lookup, clone, and fallback
  progress for each existing skill. If skills.json already exists, Ktesio
  leaves it untouched.

Example:
  kt init .";

const INSTALL_AFTER_HELP: &str = "\
Details:
  With no argument, installs every dependency declared in skills.json. With
  name:repo, adds one skill after the repo is fetched and copied successfully.
  With a bare repo URL or local path, reads published skills from that repo and lets you
  choose which skills to install. GitHub owner/repo shorthand resolves to an
  HTTPS clone URL by default; use --ssh to prefer SSH. Add /skill or --skill to
  install one published skill from a multi-skill repository.

Examples:
  kt install
  kt install docs:https://github.com/example/agent-docs.git
  kt install docs:hashicorp/agent-skills/run-acceptance-tests
  kt install --all https://github.com/example/agent-docs.git";

const SEARCH_AFTER_HELP: &str = "\
Details:
  Searches skills.sh for public skill listings and prints install targets that
  Ktesio can clone from git. Ktesio uses the skills.sh public API responsibly,
  respects rate limits with bounded retries, and will use the documented
  authenticated API when KTESIO_SKILLS_SH_API_KEY is configured.

Examples:
  kt search tests
  kt search \"react native\" --limit 10
  kt search tests --install";

const UPGRADE_AFTER_HELP: &str = "\
Details:
  Fetches latest upstream commits, checks out each skill's default branch, and
  updates skills.lock. Per-skill failures are reported after the rest run.

Example:
  kt upgrade";

const SELF_UPDATE_AFTER_HELP: &str = "\
Details:
  Updates the kt binary using the current install channel. Homebrew installs run
  brew upgrade, Cargo installs run cargo install --force, and manual binary
  installs download and verify the latest GitHub Release archive.

Example:
  kt self-update";

const PUBLISH_AFTER_HELP: &str = "\
Details:
  Publishes local skill paths from this repo so other projects can install
  them. Use publish add to expose a local file or directory directly.

Example:
  kt publish
  kt publish add docs skills/docs";

const LIST_AFTER_HELP: &str = "\
Details:
  Shows each known skill with repo, commit, and status. Statuses include
  installed, missing, not locked, and orphaned.

Example:
  kt list";

const SHOW_AFTER_HELP: &str = "\
Details:
  Shows the repo URL, locked commit, local installation path, and current status
  for one skill.

Example:
  kt show docs";

const DOCTOR_AFTER_HELP: &str = "\
Details:
  Validates skills.json, skills.lock, installed skill directories, published local paths,
  orphaned lock entries, and git availability.

Example:
  kt doctor";

const UNINSTALL_AFTER_HELP: &str = "\
Details:
  Removes one skill from skills.json, skills.lock, and .agents/skills. The
  remove subcommand is an alias for uninstall.

Examples:
  kt uninstall docs
  kt remove docs";

const AGENT_AFTER_HELP: &str = "\
Details:
  Manages Agent Instances in the Fleet. register creates an isolated Agent Home
  under a unique name from a native adapter (--kind) or a manifest adapter
  (--manifest <dir-or-file>), validating its Capability Declaration and Metering
  Source before any state is written; it prints the Agent Home path and the
  effective per-OS Capability Declaration. start launches a registered instance
  (its state becomes running); stop requests a graceful shutdown and escalates to
  a forced kill after the window (--timeout <secs>, default 30), leaving no
  surviving process. pause/resume suspend and resume a running instance with
  honest per-OS semantics: a guaranteed pause really suspends the process (SIGSTOP
  on Unix), a best-effort pause proceeds cooperatively and prints a visible
  qualifier note, and an unsupported pause fails fast quoting the Capability
  Declaration. remove deletes the registry entry and, with --delete, the Agent
  Home too (--retain, the default, keeps it); list shows the Fleet; show renders
  one instance's effective capabilities and runtime status. Both list and show
  accept --json for a machine-readable document (usage is now real token totals
  from the Usage Ledger, while budget/cap stays the honest seed: '—' in the table,
  null in JSON). config set writes a key to the Agent Instance layer (validated at write
  time — an unknown key outside the agent.* pass-through namespace is rejected
  with the nearest valid key suggested, and nothing is persisted); config get
  prints the effective (resolved) config, where a key set at the instance layer
  overrides the same key at the kind/engine-default layer, every time (FR-11);
  each value names its source layer (a Source column, or a source field with
  --json), and starting an instance persists an effective-config snapshot in the
  Agent Home. A secret:NAME value is resolved from the environment or the engine
  secrets file at start and delivered to the agent, but is MASKED in config get,
  the snapshot, logs, and events (FR-14); config get --reveal is the sole way to
  print it unmasked. Removing a running instance requires --force.

Examples:
  kt agent register demo --kind mock
  kt agent register my-agent --manifest ./my-agent
  kt agent start my-agent
  kt agent pause my-agent
  kt agent resume my-agent
  kt agent stop my-agent --timeout 10
  kt agent show demo
  kt agent list
  kt agent list --json
  kt agent config set demo model gpt-4
  kt agent config set demo agent.api_key secret:OPENAI_KEY
  kt agent config get demo
  kt agent config get demo model
  kt agent config get demo --json
  kt agent config get demo --reveal
  kt agent remove demo --delete";

#[derive(Parser)]
#[command(
    name = "kt",
    version,
    about = "Agentic skills package manager",
    after_help = HELP_FOOTER
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new skills.json manifest
    #[command(
        about = "Initialize a new skills.json manifest",
        after_help = INIT_AFTER_HELP
    )]
    Init {
        /// Directory path where skills.json will be created
        path: String,
    },
    /// Install skills from skills.json
    #[command(
        about = "Install skills from skills.json",
        after_help = INSTALL_AFTER_HELP
    )]
    Install {
        /// Install every discovered published skill from a repo target
        #[arg(long)]
        all: bool,
        /// Resolve GitHub owner/repo shorthand to an SSH clone URL
        #[arg(long)]
        ssh: bool,
        /// Install one named source published skill from the target repo
        #[arg(long)]
        skill: Option<String>,
        /// Accept safe defaults for prompts
        #[arg(long)]
        yes: bool,
        /// Fail instead of prompting for interactive choices
        #[arg(long = "no-input")]
        no_input: bool,
        /// Optional: skill name and repo URL (format: name:url)
        target: Option<String>,
    },
    /// Search public skill listings
    #[command(
        about = "Search public skill listings",
        after_help = SEARCH_AFTER_HELP
    )]
    Search {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Maximum number of results to return
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Select and install one result after searching
        #[arg(long)]
        install: bool,
        /// Fail instead of prompting for interactive choices
        #[arg(long = "no-input")]
        no_input: bool,
        /// Search query
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
    },
    /// Upgrade all installed skills to latest versions
    #[command(
        about = "Upgrade all installed skills to latest versions",
        after_help = UPGRADE_AFTER_HELP
    )]
    Upgrade,
    /// Update the kt binary
    #[command(
        name = "self-update",
        about = "Update the kt binary",
        after_help = SELF_UPDATE_AFTER_HELP
    )]
    SelfUpdate,
    /// Publish local skills from this repo
    #[command(
        about = "Publish local skills from this repo",
        after_help = PUBLISH_AFTER_HELP
    )]
    Publish {
        #[command(subcommand)]
        command: Option<PublishCommands>,
    },
    /// List installed skills
    #[command(
        about = "List installed skills",
        after_help = LIST_AFTER_HELP
    )]
    List {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show details for a specific skill
    #[command(
        about = "Show details for a specific skill",
        after_help = SHOW_AFTER_HELP
    )]
    Show {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
        /// Name of the skill to inspect
        package_name: String,
    },
    /// Validate project skill state
    #[command(
        about = "Validate project skill state",
        after_help = DOCTOR_AFTER_HELP
    )]
    Doctor,
    /// Remove a skill from the project
    #[command(
        alias = "remove",
        about = "Remove a skill from the project",
        after_help = UNINSTALL_AFTER_HELP
    )]
    Uninstall {
        /// Name of the skill to remove
        package_name: String,
    },
    /// Manage Agent Instances in the Fleet
    #[command(
        about = "Manage Agent Instances in the Fleet",
        after_help = AGENT_AFTER_HELP
    )]
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
}

#[derive(Subcommand)]
enum PublishCommands {
    /// Add or update one published local skill in skills.json
    Add {
        /// Published skill name
        skill: String,
        /// Local file or directory path to publish
        path: String,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    /// Register a new Agent Instance under a unique name
    Register {
        /// Fleet-unique instance name (^[a-z0-9][a-z0-9_-]*$)
        name: String,
        /// Native adapter kind (e.g. mock). Mutually exclusive with --manifest.
        #[arg(
            long,
            conflicts_with = "manifest",
            required_unless_present = "manifest"
        )]
        kind: Option<String>,
        /// Path to a manifest adapter directory (or adapter.toml file).
        /// Mutually exclusive with --kind.
        #[arg(long)]
        manifest: Option<String>,
    },
    /// Remove an Agent Instance from the Fleet
    Remove {
        /// Name of the Agent Instance to remove
        name: String,
        /// Delete the Agent Home directory as well
        #[arg(long, conflicts_with = "retain")]
        delete: bool,
        /// Keep the Agent Home directory on disk (default)
        #[arg(long)]
        retain: bool,
        /// Remove even if the instance is running
        #[arg(long)]
        force: bool,
    },
    /// Start a registered Agent Instance
    Start {
        /// Name of the Agent Instance to start
        name: String,
    },
    /// Stop a running Agent Instance (graceful, then forced after the window)
    Stop {
        /// Name of the Agent Instance to stop
        name: String,
        /// Graceful-shutdown window in seconds before a forced kill (default 30)
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Pause a running Agent Instance (honest per-OS: guaranteed/best-effort/unsupported)
    Pause {
        /// Name of the Agent Instance to pause
        name: String,
    },
    /// Resume a paused Agent Instance
    Resume {
        /// Name of the Agent Instance to resume
        name: String,
    },
    /// List every Agent Instance in the Fleet
    List {
        /// Emit the Fleet as a machine-readable JSON document (FR-4 / AD-14)
        #[arg(long)]
        json: bool,
    },
    /// Show an Agent Instance's effective per-OS Capability Declaration
    Show {
        /// Name of the Agent Instance to inspect
        name: String,
        /// Emit the instance's runtime status as a machine-readable JSON document
        #[arg(long)]
        json: bool,
    },
    /// Get or set an Agent Instance's unified config (layered, FR-11)
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Set a config key on the Agent Instance layer (validated at write time)
    Set {
        /// Name of the Agent Instance
        name: String,
        /// Config key (a known unified key, or an `agent.*` pass-through key)
        key: String,
        /// Value to set (stored verbatim; a `secret:NAME` reference is resolved +
        /// masked at start/read, FR-14 — the reference is what is stored here)
        value: String,
    },
    /// Get an Agent Instance's effective (resolved) config value(s) with per-value source
    Get {
        /// Name of the Agent Instance
        name: String,
        /// Optional config key; omitted prints the whole effective config
        key: Option<String>,
        /// Emit the effective config (value + source layer per leaf) as JSON (FR-13)
        #[arg(long)]
        json: bool,
        /// Reveal secret values unmasked (the SOLE explicit acknowledgment; FR-14).
        /// Without it, `secret:` values are masked in both the table and --json.
        /// Never un-masks the persisted snapshot, logs, or events. Re-resolves
        /// secrets LIVE (env, then the secrets file) at read time, so a revealed
        /// value may differ from what a running instance resolved at its start.
        #[arg(long)]
        reveal: bool,
    },
}

#[cfg(not(tarpaulin_include))]
fn main() {
    if let Err(err) = run_cli() {
        ui::error(err);
        std::process::exit(1);
    }
}

#[cfg(not(tarpaulin_include))]
fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    if should_check_for_updates(&cli.command) {
        if let Some(notice) = update_check::maybe_notice() {
            ui::update_notice(&notice.current_version, &notice.latest_version);
        }
    }

    match cli.command {
        Some(Commands::Init { path }) => cli::init::run(&path),
        Some(Commands::Install {
            all,
            ssh,
            skill,
            yes,
            no_input,
            target,
        }) => cli::install::run_with_options(
            target.as_deref(),
            cli::install::InstallOptions {
                all,
                yes,
                no_input,
                ssh,
                skill,
            },
        ),
        Some(Commands::Search {
            json,
            limit,
            install,
            no_input,
            query,
        }) => cli::search::run_with_options(
            &query.join(" "),
            cli::search::SearchOptions {
                json,
                limit,
                install,
                no_input,
            },
        ),
        Some(Commands::Upgrade) => cli::upgrade::run(),
        Some(Commands::SelfUpdate) => cli::self_update::run(),
        Some(Commands::Publish { command }) => match command {
            Some(PublishCommands::Add { skill, path }) => cli::publish::run_add(&skill, &path),
            None => cli::publish::run(),
        },
        Some(Commands::List { json }) => cli::list::run_with_options(json),
        Some(Commands::Show { json, package_name }) => {
            cli::show::run_with_options(&package_name, json)
        }
        Some(Commands::Doctor) => cli::doctor::run(),
        Some(Commands::Uninstall { package_name }) => cli::uninstall::run(&package_name),
        Some(Commands::Agent { command }) => match command {
            AgentCommands::Register {
                name,
                kind,
                manifest,
            } => {
                let adapter = cli::agent::AdapterArg::from_flags(kind, manifest)?;
                cli::agent::register(&name, &adapter)
            }
            AgentCommands::Remove {
                name,
                delete,
                retain,
                force,
            } => cli::agent::remove(
                &name,
                cli::agent::DispositionArg::from_flags(delete, retain),
                force,
            ),
            AgentCommands::Start { name } => cli::agent::start(&name),
            AgentCommands::Stop { name, timeout } => cli::agent::stop(&name, timeout),
            AgentCommands::Pause { name } => cli::agent::pause(&name),
            AgentCommands::Resume { name } => cli::agent::resume(&name),
            AgentCommands::List { json } => cli::agent::list(json),
            AgentCommands::Show { name, json } => cli::agent::show(&name, json),
            AgentCommands::Config { command } => match command {
                ConfigCommands::Set { name, key, value } => {
                    cli::agent::config_set(&name, &key, &value)
                }
                ConfigCommands::Get {
                    name,
                    key,
                    json,
                    reveal,
                } => cli::agent::config_get(&name, key.as_deref(), json, reveal),
            },
        },
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

fn should_check_for_updates(command: &Option<Commands>) -> bool {
    matches!(command, Some(command) if !matches!(command, Commands::SelfUpdate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_struct_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn test_cli_subcommands_exist() {
        let cmd = Cli::command();
        assert!(cmd.find_subcommand("init").is_some());
        assert!(cmd.find_subcommand("install").is_some());
        assert!(cmd.find_subcommand("search").is_some());
        assert!(cmd.find_subcommand("upgrade").is_some());
        assert!(cmd.find_subcommand("self-update").is_some());
        assert!(cmd.find_subcommand("publish").is_some());
        assert!(cmd.find_subcommand("list").is_some());
        assert!(cmd.find_subcommand("show").is_some());
        assert!(cmd.find_subcommand("doctor").is_some());
        assert!(cmd.find_subcommand("uninstall").is_some());
        assert!(cmd.find_subcommand("remove").is_some());
        assert!(cmd.find_subcommand("agent").is_some());
    }

    #[test]
    fn test_agent_subcommands_exist() {
        let cmd = Cli::command();
        let agent = cmd
            .find_subcommand("agent")
            .expect("agent subcommand should exist");
        assert!(agent.get_subcommands().any(|c| c.get_name() == "register"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "remove"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "start"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "stop"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "pause"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "resume"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "list"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "show"));
        assert!(agent.get_subcommands().any(|c| c.get_name() == "config"));
    }

    #[test]
    fn test_agent_config_parse() {
        // Story 2-1: `config set <name> <key> <value>` and
        // `config get <name> [key]` parse (a nested subcommand).
        assert!(
            Cli::try_parse_from(["kt", "agent", "config", "set", "demo", "model", "gpt-4"]).is_ok()
        );
        assert!(Cli::try_parse_from(["kt", "agent", "config", "get", "demo"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "config", "get", "demo", "model"]).is_ok());
        // Story 2-3: `config get` accepts `--json` (whole config or single key).
        assert!(Cli::try_parse_from(["kt", "agent", "config", "get", "demo", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from(["kt", "agent", "config", "get", "demo", "model", "--json"])
                .is_ok()
        );
        // set requires all three positional args.
        assert!(Cli::try_parse_from(["kt", "agent", "config", "set", "demo", "model"]).is_err());
        assert!(Cli::try_parse_from(["kt", "agent", "config", "set", "demo"]).is_err());
        // get requires at least a name.
        assert!(Cli::try_parse_from(["kt", "agent", "config", "get"]).is_err());
        // config requires a subcommand.
        assert!(Cli::try_parse_from(["kt", "agent", "config"]).is_err());
    }

    #[test]
    fn test_agent_start_stop_parse() {
        // `start <name>` and `stop <name> [--timeout <secs>]` parse.
        assert!(Cli::try_parse_from(["kt", "agent", "start", "svc"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "stop", "svc"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "stop", "svc", "--timeout", "10"]).is_ok());
        // start requires a name.
        assert!(Cli::try_parse_from(["kt", "agent", "start"]).is_err());
        // --timeout must be a number.
        assert!(Cli::try_parse_from(["kt", "agent", "stop", "svc", "--timeout", "abc"]).is_err());
    }

    #[test]
    fn test_agent_list_and_show_accept_json_flag() {
        // Story 1-7: `--json` is ADDED to the Agent `list`/`show` subcommands
        // (they took none before). Bare forms and the `--json` forms both parse.
        assert!(Cli::try_parse_from(["kt", "agent", "list"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "list", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "show", "svc"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "show", "svc", "--json"]).is_ok());
        // `show --json` still requires a name.
        assert!(Cli::try_parse_from(["kt", "agent", "show", "--json"]).is_err());
    }

    #[test]
    fn test_agent_pause_resume_parse() {
        // `pause <name>` and `resume <name>` parse (story 1-5).
        assert!(Cli::try_parse_from(["kt", "agent", "pause", "svc"]).is_ok());
        assert!(Cli::try_parse_from(["kt", "agent", "resume", "svc"]).is_ok());
        // Both require a name.
        assert!(Cli::try_parse_from(["kt", "agent", "pause"]).is_err());
        assert!(Cli::try_parse_from(["kt", "agent", "resume"]).is_err());
    }

    #[test]
    fn test_agent_register_requires_kind_or_manifest() {
        // Neither flag → clap error (required_unless_present).
        assert!(Cli::try_parse_from(["kt", "agent", "register", "demo"]).is_err());
        // --kind alone parses.
        assert!(Cli::try_parse_from(["kt", "agent", "register", "demo", "--kind", "mock"]).is_ok());
        // --manifest alone parses.
        assert!(
            Cli::try_parse_from(["kt", "agent", "register", "demo", "--manifest", "./a"]).is_ok()
        );
        // Both together → conflict error.
        assert!(Cli::try_parse_from([
            "kt",
            "agent",
            "register",
            "demo",
            "--kind",
            "mock",
            "--manifest",
            "./a"
        ])
        .is_err());
    }

    #[test]
    fn test_cli_help_includes_license_and_repository() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("License: PolyForm-Noncommercial-1.0.0"));
        assert!(help.contains("Repository: https://github.com/iMagdy/ktesio"));
    }

    #[test]
    fn test_cli_without_subcommand_is_allowed_for_help_display() {
        let cli = Cli::try_parse_from(["kt"]).expect("bare kt should parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_subcommand_help_includes_details_and_examples() {
        for (command, detail) in [
            ("init", "Creates a manifest with dependencies"),
            ("install", "installs every dependency"),
            ("upgrade", "Fetches latest upstream commits"),
            ("self-update", "Updates the kt binary"),
            ("publish", "Publishes local skill paths"),
            ("list", "Shows each known skill"),
            ("show", "Shows the repo URL"),
            ("doctor", "Validates skills.json"),
            ("uninstall", "Removes one skill"),
            ("agent", "Manages Agent Instances"),
        ] {
            let mut cmd = Cli::command();
            let help = cmd
                .find_subcommand_mut(command)
                .expect("subcommand should exist")
                .render_help()
                .to_string();
            assert!(help.contains(detail), "{} help missing detail", command);
            assert!(help.contains("Example"), "{} help missing example", command);
        }
    }

    #[test]
    fn test_self_update_skips_passive_update_check() {
        let cli = Cli::try_parse_from(["kt", "self-update"]).expect("self-update should parse");

        assert!(!should_check_for_updates(&cli.command));
    }
}
