use amt::{AmtClient, BootDevice, ClientOptions, HostConfig, HostDb, PowerState, Protocol};
use anyhow::{Context, Result, anyhow};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::process::Command as ProcessCommand;
use std::str::FromStr;

#[derive(Debug, Parser)]
#[command(version, about = "Small Intel AMT power and boot control CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage saved hosts.
    Host {
        #[command(subcommand)]
        command: HostCommand,
    },
    /// Run a command against a saved host alias.
    Run {
        alias: String,
        #[arg(long)]
        accept_invalid_certs: bool,
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Run a command directly against a host.
    Direct {
        #[command(flatten)]
        target: DirectTarget,
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Print the CLI version.
    Version,
}

#[derive(Debug, Subcommand)]
enum HostCommand {
    /// List saved host aliases.
    List,
    /// Show a saved host alias without printing the password.
    Get { alias: String },
    /// Add or update a saved host alias.
    Set {
        alias: String,
        host: String,
        #[arg(long, default_value = "admin")]
        username: String,
        #[arg(long, env = "AMT_PASSWORD")]
        password: Option<String>,
        #[arg(long)]
        password_ref: Option<String>,
        #[arg(long, default_value = "http")]
        protocol: ProtocolArg,
    },
    /// Remove a saved host alias.
    Remove { alias: String },
    /// Print the host database path.
    Path,
}

#[derive(Debug, Subcommand)]
enum DeviceCommand {
    /// Change or inspect power state.
    Power {
        #[command(subcommand)]
        command: PowerCommand,
    },
    /// Set the next boot target.
    Boot {
        #[command(subcommand)]
        command: BootCommand,
    },
    /// Set PXE for next boot and reboot.
    Pxeboot,
    /// Print the AMT platform UUID.
    Uuid,
    /// Print the AMT Server header reported by firmware.
    Version,
}

#[derive(Debug, Subcommand)]
enum PowerCommand {
    On,
    Off,
    Reboot,
    Reset,
    Sleep,
    Hibernate,
    Status,
}

#[derive(Debug, Subcommand)]
enum BootCommand {
    Pxe,
    Hd,
    Cd,
}

#[derive(Debug, Args)]
struct DirectTarget {
    #[arg(long)]
    host: String,
    #[arg(long, default_value = "admin")]
    username: String,
    #[arg(long, env = "AMT_PASSWORD")]
    password: Option<String>,
    #[arg(long)]
    password_ref: Option<String>,
    #[arg(long, default_value = "http")]
    protocol: ProtocolArg,
    #[arg(long)]
    accept_invalid_certs: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProtocolArg {
    Http,
    Https,
}

impl ProtocolArg {
    fn as_protocol(self) -> Protocol {
        match self {
            Self::Http => Protocol::Http,
            Self::Https => Protocol::Https,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Host { command } => run_host(command),
        Command::Run {
            alias,
            accept_invalid_certs,
            command,
        } => {
            let db = HostDb::load()?;
            let host = db
                .get(&alias)
                .ok_or_else(|| anyhow!("host alias {alias} not found"))?;
            let options = options_from_host(host, accept_invalid_certs)?;
            run_device(AmtClient::new(options)?, command)
        }
        Command::Direct { target, command } => {
            let options = ClientOptions {
                host: target.host,
                username: target.username,
                password: resolve_password(target.password, target.password_ref)?,
                protocol: target.protocol.as_protocol(),
                accept_invalid_certs: target.accept_invalid_certs,
            };
            run_device(AmtClient::new(options)?, command)
        }
        Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn run_host(command: HostCommand) -> Result<()> {
    match command {
        HostCommand::List => {
            let db = HostDb::load()?;
            for alias in db.hosts.keys() {
                println!("{alias}");
            }
            Ok(())
        }
        HostCommand::Get { alias } => {
            let db = HostDb::load()?;
            let host = db
                .get(&alias)
                .ok_or_else(|| anyhow!("host alias {alias} not found"))?;
            println!(
                "{alias} => {} ({}, {}, {})",
                host.host,
                host.username,
                host.protocol,
                credential_label(host)
            );
            Ok(())
        }
        HostCommand::Set {
            alias,
            host,
            username,
            password,
            password_ref,
            protocol,
        } => {
            ensure_one_credential(password.as_ref(), password_ref.as_ref())?;
            let mut db = HostDb::load()?;
            db.set(
                alias,
                HostConfig {
                    host,
                    username,
                    password,
                    password_ref,
                    protocol: protocol_to_string(protocol),
                },
            );
            db.save()
        }
        HostCommand::Remove { alias } => {
            let mut db = HostDb::load()?;
            if !db.remove(&alias) {
                return Err(anyhow!("host alias {alias} not found"));
            }
            db.save()
        }
        HostCommand::Path => {
            println!("{}", amt::config::config_path()?.display());
            Ok(())
        }
    }
}

fn run_device(client: AmtClient, command: DeviceCommand) -> Result<()> {
    match command {
        DeviceCommand::Power { command } => match command {
            PowerCommand::On => client.power(PowerState::On),
            PowerCommand::Off => client.power(PowerState::Off),
            PowerCommand::Reboot => client.power(PowerState::Reboot),
            PowerCommand::Reset => client.power(PowerState::Reset),
            PowerCommand::Sleep => client.power(PowerState::Sleep),
            PowerCommand::Hibernate => client.power(PowerState::Hibernate),
            PowerCommand::Status => {
                println!("{}", client.power_status()?);
                Ok(())
            }
        },
        DeviceCommand::Boot { command } => match command {
            BootCommand::Pxe => client.set_next_boot(BootDevice::Pxe),
            BootCommand::Hd => client.set_next_boot(BootDevice::HardDrive),
            BootCommand::Cd => client.set_next_boot(BootDevice::Cd),
        },
        DeviceCommand::Pxeboot => client.pxe_boot(),
        DeviceCommand::Uuid => {
            println!("{}", client.uuid()?);
            Ok(())
        }
        DeviceCommand::Version => {
            println!(
                "{}",
                client.version()?.unwrap_or_else(|| "unknown".to_string())
            );
            Ok(())
        }
    }
}

fn options_from_host(host: &HostConfig, accept_invalid_certs: bool) -> Result<ClientOptions> {
    Ok(ClientOptions {
        host: host.host.clone(),
        username: host.username.clone(),
        password: resolve_password(host.password.clone(), host.password_ref.clone())?,
        protocol: Protocol::from_str(&host.protocol)
            .with_context(|| format!("invalid protocol in host entry: {}", host.protocol))?,
        accept_invalid_certs,
    })
}

fn ensure_one_credential(password: Option<&String>, password_ref: Option<&String>) -> Result<()> {
    match (password, password_ref) {
        (Some(_), Some(_)) => Err(anyhow!("use either --password or --password-ref, not both")),
        (None, None) => Err(anyhow!(
            "missing credential: use --password, AMT_PASSWORD, or --password-ref"
        )),
        _ => Ok(()),
    }
}

fn resolve_password(password: Option<String>, password_ref: Option<String>) -> Result<String> {
    ensure_one_credential(password.as_ref(), password_ref.as_ref())?;
    if let Some(password) = password {
        return Ok(password);
    }

    let password_ref = password_ref.expect("credential presence checked");
    resolve_password_ref(&password_ref)
}

fn resolve_password_ref(password_ref: &str) -> Result<String> {
    let Some(entry) = password_ref.strip_prefix("gopass:") else {
        return Err(anyhow!(
            "unsupported password_ref {password_ref}; expected gopass:<path>"
        ));
    };
    let output = ProcessCommand::new("gopass")
        .arg("show")
        .arg("-o")
        .arg(entry)
        .output()
        .with_context(|| format!("failed to run gopass for {entry}"))?;
    if !output.status.success() {
        return Err(anyhow!("gopass failed for {entry}"));
    }
    let mut secret = String::from_utf8(output.stdout).context("gopass output was not UTF-8")?;
    while secret.ends_with(['\n', '\r']) {
        secret.pop();
    }
    if secret.is_empty() {
        return Err(anyhow!("gopass returned an empty secret for {entry}"));
    }
    Ok(secret)
}

fn credential_label(host: &HostConfig) -> String {
    match (&host.password_ref, &host.password) {
        (Some(password_ref), _) => format!("password_ref={password_ref}"),
        (None, Some(_)) => "password=stored".to_string(),
        (None, None) => "password=missing".to_string(),
    }
}

fn protocol_to_string(protocol: ProtocolArg) -> String {
    match protocol {
        ProtocolArg::Http => "http",
        ProtocolArg::Https => "https",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_command() {
        let cli = Cli::try_parse_from(["amt", "version"]).unwrap();
        assert!(matches!(cli.command, Command::Version));
    }

    #[test]
    fn parses_direct_power_status_command() {
        let cli = Cli::try_parse_from([
            "amt",
            "direct",
            "--host",
            "192.0.2.10",
            "--password-ref",
            "gopass:homelab/amt",
            "power",
            "status",
        ])
        .unwrap();

        match cli.command {
            Command::Direct { target, command } => {
                assert_eq!(target.host, "192.0.2.10");
                assert_eq!(target.username, "admin");
                assert_eq!(target.password_ref.as_deref(), Some("gopass:homelab/amt"));
                assert!(target.password.is_none());
                assert!(matches!(target.protocol, ProtocolArg::Http));
                assert!(matches!(
                    command,
                    DeviceCommand::Power {
                        command: PowerCommand::Status
                    }
                ));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_https_direct_uuid_command() {
        let cli = Cli::try_parse_from([
            "amt",
            "direct",
            "--host",
            "nuc.example",
            "--password-ref",
            "gopass:homelab/amt",
            "--protocol",
            "https",
            "--accept-invalid-certs",
            "uuid",
        ])
        .unwrap();

        match cli.command {
            Command::Direct { target, command } => {
                assert_eq!(target.host, "nuc.example");
                assert!(matches!(target.protocol, ProtocolArg::Https));
                assert_eq!(target.password_ref.as_deref(), Some("gopass:homelab/amt"));
                assert!(target.accept_invalid_certs);
                assert!(matches!(command, DeviceCommand::Uuid));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn parses_host_set_without_exposing_password_in_get_shape() {
        let cli = Cli::try_parse_from([
            "amt",
            "host",
            "set",
            "nuc",
            "192.0.2.10",
            "--password-ref",
            "gopass:homelab/amt",
            "--protocol",
            "https",
        ])
        .unwrap();

        match cli.command {
            Command::Host {
                command:
                    HostCommand::Set {
                        alias,
                        host,
                        username,
                        password,
                        password_ref,
                        protocol,
                    },
            } => {
                assert_eq!(alias, "nuc");
                assert_eq!(host, "192.0.2.10");
                assert_eq!(username, "admin");
                assert!(password.is_none());
                assert_eq!(password_ref.as_deref(), Some("gopass:homelab/amt"));
                assert!(matches!(protocol, ProtocolArg::Https));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn rejects_ambiguous_credentials() {
        let password = "secret".to_string();
        let password_ref = "gopass:homelab/amt".to_string();
        let err = ensure_one_credential(Some(&password), Some(&password_ref)).unwrap_err();
        assert!(
            err.to_string()
                .contains("either --password or --password-ref")
        );
    }
}
