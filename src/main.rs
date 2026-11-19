use anyhow::Result;
use clap::Parser;
use pai_sho::{client, daemon, launchd, Command, DaemonService};

#[derive(Parser)]
#[clap(
    name = "pai-sho",
    about = "Forward ports between your own machines, peer to peer over iroh",
    long_about = "Forward ports between your own machines, peer to peer over iroh. \
Neither side needs an account, a public IP, or an open inbound port.\n\n\
Two machines link by invitation: one runs `invite`, the other runs `accept`. \
Nothing is reachable until you grant a port to a peer's key, and then only that \
peer can reach it.\n\n\
A peer's ports come up under the name you gave it, like vibenv-ndyg.pai-sho.",
    version
)]
struct Cli {
    /// Path to Unix socket
    #[arg(long, default_value_t = default_socket())]
    socket: String,

    #[command(subcommand)]
    command: Command,
}

/// /tmp/pai-sho.sock on unix. On Windows, pai-sho.sock in the user's temp
/// directory (%TEMP%), the nearest thing to /tmp.
fn default_socket() -> String {
    if cfg!(windows) {
        std::env::temp_dir()
            .join("pai-sho.sock")
            .to_string_lossy()
            .into_owned()
    } else {
        "/tmp/pai-sho.sock".to_string()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("pai_sho=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    let socket_path = std::path::Path::new(&cli.socket);

    match cli.command {
        Command::Daemon {
            service,
            host,
            accept,
            ports,
            key_path,
            resolver,
            tun,
            socket_owner,
            socket_mode,
            name,
        } => match service {
            Some(DaemonService::Install) => launchd::install()?,
            Some(DaemonService::Up) => launchd::up()?,
            Some(DaemonService::Down) => launchd::down()?,
            Some(DaemonService::Remove) => launchd::remove()?,
            Some(DaemonService::Status) => launchd::status()?,
            None => {
                daemon::run(
                    host,
                    socket_path,
                    accept,
                    ports,
                    key_path,
                    resolver,
                    tun,
                    socket_owner,
                    socket_mode,
                    name,
                )
                .await?;
            }
        },
        _ => {
            client::send_command(socket_path, cli.command).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)
    }

    #[test]
    fn expose_needs_a_grantee() {
        assert!(parse(&["pai-sho", "expose", "5555"]).is_err());
    }

    #[test]
    fn expose_takes_a_named_peer() {
        let cli = parse(&["pai-sho", "expose", "5555", "--to", "abc"]).unwrap();
        match cli.command {
            Command::Expose { port, to, all } => {
                assert_eq!(port, 5555);
                assert_eq!(to, vec!["abc".to_string()]);
                assert!(!all);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_takes_several_named_peers() {
        let cli = parse(&["pai-sho", "expose", "5555", "--to", "a", "--to", "b"]).unwrap();
        match cli.command {
            Command::Expose { to, .. } => assert_eq!(to, vec!["a".to_string(), "b".to_string()]),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_takes_all() {
        let cli = parse(&["pai-sho", "expose", "5555", "--all"]).unwrap();
        match cli.command {
            Command::Expose { to, all, .. } => {
                assert!(all);
                assert!(to.is_empty());
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn expose_refuses_both_at_once() {
        assert!(parse(&["pai-sho", "expose", "5555", "--to", "a", "--all"]).is_err());
    }

    #[test]
    fn unexpose_needs_no_grantee() {
        let cli = parse(&["pai-sho", "unexpose", "5555"]).unwrap();
        match cli.command {
            Command::Unexpose { port, to } => {
                assert_eq!(port, 5555);
                assert!(to.is_none());
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn invite_needs_nothing() {
        let cli = parse(&["pai-sho", "invite"]).unwrap();
        match cli.command {
            Command::Invite {
                key,
                name,
                expose,
                qr,
            } => {
                assert!(key.is_none());
                assert!(name.is_none());
                assert!(expose.is_empty());
                assert!(!qr);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn invite_takes_a_key_a_name_and_ports() {
        let cli = parse(&[
            "pai-sho",
            "invite",
            "abc",
            "--as",
            "rustdev",
            "--expose",
            "3001,7331",
        ])
        .unwrap();
        match cli.command {
            Command::Invite {
                key, name, expose, ..
            } => {
                assert_eq!(key, Some("abc".to_string()));
                assert_eq!(name, Some("rustdev".to_string()));
                assert_eq!(expose, vec![3001, 7331]);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn invite_can_draw_a_qr_code() {
        let cli = parse(&["pai-sho", "invite", "--qr"]).unwrap();
        match cli.command {
            Command::Invite { qr, .. } => assert!(qr),
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn accept_needs_a_handle() {
        assert!(parse(&["pai-sho", "accept"]).is_err());
    }

    #[test]
    fn accept_takes_a_handle_and_a_name() {
        let cli = parse(&["pai-sho", "accept", "abc.def", "--as", "buildbox"]).unwrap();
        match cli.command {
            Command::Accept { handle, name } => {
                assert_eq!(handle, "abc.def");
                assert_eq!(name, Some("buildbox".to_string()));
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn the_old_names_are_gone() {
        for old in [
            "ticket",
            "grant-token",
            "pin",
            "add-peer",
            "remove-peer",
            "surfaces",
        ] {
            assert!(parse(&["pai-sho", old]).is_err(), "{} still parses", old);
        }
    }

    #[test]
    fn the_daemon_accepts_invitations() {
        let cli = parse(&["pai-sho", "daemon", "--accept", "abc.def", "-e", "3001"]).unwrap();
        match cli.command {
            Command::Daemon {
                service,
                accept,
                ports,
                ..
            } => {
                assert!(service.is_none());
                assert_eq!(accept, vec!["abc.def".to_string()]);
                assert_eq!(ports, vec![3001]);
            }
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn daemon_service_commands_parse() {
        for (arg, want) in [
            ("install", DaemonService::Install),
            ("up", DaemonService::Up),
            ("down", DaemonService::Down),
            ("remove", DaemonService::Remove),
            ("status", DaemonService::Status),
        ] {
            let cli = parse(&["pai-sho", "daemon", arg]).unwrap();
            match cli.command {
                Command::Daemon { service, .. } => assert_eq!(service, Some(want)),
                _ => panic!("wrong command"),
            }
        }
    }

    #[test]
    fn daemon_service_rejects_run_flags() {
        assert!(parse(&["pai-sho", "daemon", "install", "--tun", "utun"]).is_err());
        assert!(parse(&["pai-sho", "daemon", "--tun", "utun", "install"]).is_err());
    }
}
