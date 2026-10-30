//! Command line of the `jarvis-api` binary.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "jarvis-api",
    version,
    about = "Jarvis MT backend: HTTP API, web UI, login, voice",
    long_about = "Run without a subcommand to start the server. `user` manages login accounts \
                  and `service` manages the Windows service."
)]
pub struct Cli {
    /// Config file (default: $JARVIS_CONFIG or config/jarvis.toml).
    #[arg(long, short = 'c', global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Start the server (the default).
    Serve,
    /// Manage login accounts.
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Manage the Windows service.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum UserCommand {
    /// Create a user. Asks for the password twice (no echo) and prints the authenticator QR code.
    Add {
        name: String,
        /// Read the password from the first line of stdin instead of asking (for scripts).
        #[arg(long)]
        password_stdin: bool,
    },
    /// Issue a new authenticator secret (lost phone). Ends the user's sessions.
    ResetTotp { name: String },
    /// List users.
    List,
    /// Delete a user.
    Remove {
        name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum ServiceCommand {
    /// Register Jarvis as a Windows service (needs an elevated prompt).
    Install,
    /// Remove the Windows service.
    Uninstall,
    /// Start the installed service.
    Start,
    /// Stop the installed service.
    Stop,
    /// Entry point the Service Control Manager calls. Not for interactive use.
    Run {
        /// Directory to run in (holds config/, data/, logs/ and models/).
        #[arg(long, value_name = "DIR")]
        workdir: Option<PathBuf>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("jarvis-api").chain(args.iter().copied()))
    }

    #[test]
    fn no_arguments_means_serve() {
        let cli = parse(&[]).unwrap();
        assert_eq!(cli.command, None);
        assert_eq!(cli.config, None);
        assert_eq!(parse(&["serve"]).unwrap().command, Some(Command::Serve));
    }

    #[test]
    fn user_commands() {
        assert_eq!(
            parse(&["user", "add", "ninad"]).unwrap().command,
            Some(Command::User {
                command: UserCommand::Add {
                    name: "ninad".into(),
                    password_stdin: false
                }
            })
        );
        assert_eq!(
            parse(&["user", "add", "n", "--password-stdin"])
                .unwrap()
                .command,
            Some(Command::User {
                command: UserCommand::Add {
                    name: "n".into(),
                    password_stdin: true
                }
            })
        );
        assert_eq!(
            parse(&["user", "reset-totp", "n"]).unwrap().command,
            Some(Command::User {
                command: UserCommand::ResetTotp { name: "n".into() }
            })
        );
        assert_eq!(
            parse(&["user", "list"]).unwrap().command,
            Some(Command::User {
                command: UserCommand::List
            })
        );
        assert_eq!(
            parse(&["user", "remove", "n", "-y"]).unwrap().command,
            Some(Command::User {
                command: UserCommand::Remove {
                    name: "n".into(),
                    yes: true
                }
            })
        );
        assert!(parse(&["user", "add"]).is_err(), "a name is required");
        assert!(parse(&["user"]).is_err());
    }

    #[test]
    fn service_commands_and_global_config() {
        for (word, expected) in [
            ("install", ServiceCommand::Install),
            ("uninstall", ServiceCommand::Uninstall),
            ("start", ServiceCommand::Start),
            ("stop", ServiceCommand::Stop),
        ] {
            assert_eq!(
                parse(&["service", word]).unwrap().command,
                Some(Command::Service { command: expected })
            );
        }
        let cli = parse(&["--config", "c.toml", "service", "run", "--workdir", "D:/J"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("c.toml")));
        assert_eq!(
            cli.command,
            Some(Command::Service {
                command: ServiceCommand::Run {
                    workdir: Some(PathBuf::from("D:/J"))
                }
            })
        );
        // The global flag may also come after the subcommand.
        let cli = parse(&["user", "list", "-c", "x.toml"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("x.toml")));
        assert!(parse(&["service", "explode"]).is_err());
    }
}
