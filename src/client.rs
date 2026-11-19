//! CLI client - sends commands to daemon over Unix socket.

use crate::protocol::{Accepted, Request, Response, VERSION};
use crate::Command;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn send_command(socket_path: &Path, command: Command) -> Result<()> {
    let qr = matches!(command, Command::Invite { qr: true, .. });
    // `accept` waits for the peer's answer, so say who it is waiting on.
    if let Command::Accept { handle, name } = &command {
        let who = match name {
            Some(name) => name.clone(),
            None => handle.chars().take(8).collect(),
        };
        eprintln!("joining {}...", who);
    }
    let request = match command {
        Command::Invite {
            key, name, expose, ..
        } => Request::Invite { key, name, expose },
        Command::Accept { handle, name } => Request::Accept {
            handle,
            name,
            wait: true,
        },
        Command::Forget { peer } => Request::Forget { peer },
        Command::Rename { peer, name } => Request::Rename { peer, name },
        Command::Expose { port, to, all } => Request::Expose { port, to, all },
        Command::Unexpose { port, to } => Request::Unexpose { port, to },
        Command::List => Request::List,
        Command::Key => Request::Key,
        Command::Project { peer, ip, name } => Request::Project {
            peer,
            ip: ip.map(|ip| ip.to_string()),
            name,
        },
        Command::Unproject { peer } => Request::Unproject { peer },
        Command::Daemon { .. } => unreachable!("daemon handled separately"),
    };

    let stream = UnixStream::connect(socket_path)
        .await
        .context("failed to connect to daemon - is it running?")?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    // Send request
    let request_json = serde_json::to_string(&request)?;
    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read response
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let response: Response = serde_json::from_str(&line)?;

    // Print response
    match response {
        Response::Ok => println!("OK"),
        Response::Accepted(accepted) => {
            let (line, ok) = accepted_line(&accepted);
            if ok {
                println!("{}", line);
            } else {
                eprintln!("{}", line);
                std::process::exit(1);
            }
        }
        Response::Key(key) => println!("{}", key),
        Response::Invite(invite) => {
            if qr {
                print!("{}", crate::qr::render(&invite)?);
            }
            println!("{}", invite);
        }
        Response::List(mut info) => {
            info.cli = VERSION.to_string();
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Response::Error(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}

/// What `accept` prints, and whether it succeeded. Pending is not a failure:
/// the peer is saved and the daemon keeps dialing it.
pub fn accepted_line(accepted: &Accepted) -> (String, bool) {
    match accepted {
        Accepted::Joined { name, ports } if ports.is_empty() => {
            (format!("joined {}; it shares no ports with you yet", name), true)
        }
        Accepted::Joined { name, ports } => {
            let ports: Vec<String> = ports.iter().map(|p| p.to_string()).collect();
            (format!("joined {}; it shares {}", name, ports.join(", ")), true)
        }
        Accepted::Refused { reason } if reason == "not authorized" => (
            "refused: invitation expired or already used. Ask for a new one.".to_string(),
            false,
        ),
        Accepted::Refused { reason } => (format!("refused: {}", reason), false),
        Accepted::Pending { turned_away: Some(_) } => (
            "not let in yet: it has not invited this machine's key. pai-sho keeps trying.".to_string(),
            true,
        ),
        Accepted::Pending { turned_away: None } => (
            "not reached yet: it is not online. pai-sho keeps trying, and the invitation must still be live when it gets through.".to_string(),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_names_the_ports() {
        let (line, ok) = accepted_line(&Accepted::Joined {
            name: "demo".into(),
            ports: vec![47831, 8000],
        });
        assert_eq!(line, "joined demo; it shares 47831, 8000");
        assert!(ok);
    }

    #[test]
    fn a_dead_code_says_to_ask_for_a_new_one() {
        let (line, ok) = accepted_line(&Accepted::Refused {
            reason: "not authorized".into(),
        });
        assert_eq!(
            line,
            "refused: invitation expired or already used. Ask for a new one."
        );
        assert!(!ok);
    }

    #[test]
    fn pending_is_not_a_failure() {
        assert!(accepted_line(&Accepted::Pending { turned_away: None }).1);
    }
}
