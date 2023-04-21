use std::path::{Path, PathBuf};

use anyhow::bail;
use hyprctl::hyprctl;
use tokio::{net::{UnixStream, UnixListener}, io::{BufStream, AsyncBufReadExt}, sync::mpsc};
use tracing_subscriber::EnvFilter;

use state::{State, Changes};

pub mod state;
pub mod hyprctl;

#[derive(Debug)]
enum Ctrl {
    ShowTag(u8),
    ToggleTag(u8),
    MoveToTag(u8, Option<String>),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).compact().init();

    let mut state = State::new();

    let hypr_dir = hyprland_dir()?;
    let hypr_event_sock = hypr_dir.join(".socket2.sock").to_string_lossy().to_string();

    let hypr_event_sock = UnixStream::connect(&hypr_event_sock).await?;
    let mut hypr_event_stream = BufStream::new(hypr_event_sock);

    let ctrl_sock = hypr_dir.join(".hyprtagctl.sock").to_string_lossy().to_string();
    let ctrl_sock = UnixListener::bind(&ctrl_sock)?;

    let (tx, mut rx) = mpsc::channel(10);
    tokio::spawn(async move {
        ctrl_listener(tx, ctrl_sock).await
    });

    loop {
        let mut buf = String::new();

        tokio::select! {
            r = hypr_event_stream.read_line(&mut buf) => {
                match r {
                    Err(err) => bail!(err),
                    Ok(r) => {
                        if r == 0 {
                            break;
                        }
                        handle_event_stream(&mut state, &buf);
                    },
                }
            }

            msg = rx.recv() => {
                match msg {
                    None => {
                        // tx closed
                        break;
                    },

                    Some(msg) => {
                        handle_ctrl(&mut state, msg);
                    },
                }
            }
        }
    }

    Ok(())
}

async fn ctrl_listener(tx: mpsc::Sender<Ctrl>, listener: UnixListener) {
    loop {
        match listener.accept().await {
            Err(err) => tracing::error!(%err, "accept failed"),

            Ok((stream, _addr)) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_ctrl_socket(tx, stream).await
                });
            }
        }
    }
}

async fn handle_ctrl_socket(tx: mpsc::Sender<Ctrl>, stream: UnixStream) {
    let mut stream = BufStream::new(stream);
    let mut buf = String::new();

    loop {
        let r = stream.read_line(&mut buf).await;
        match r {
            Err(err) => {
                tracing::error!(%err, "failed to read");
                continue;
            },

            Ok(r) => {
                if r == 0 {
                    break;
                }

                tracing::debug!("recv: {}", buf);

                let mut p = &buf[..];
                if p.ends_with("\r\n") {
                    p = &buf[..buf.len()-2];
                } else if p.ends_with("\n") {
                    p = &buf[..buf.len()-1];
                }

                tracing::debug!("p: {}", p);

                let chunks: Vec<&str> = p.split(" ").collect();

                if chunks.len() == 0 {
                    tracing::error!("invalid input: {}", p);
                    continue;
                }
                let cmd = chunks[0];
                let args = &chunks[1..];

                match cmd {
                    "move" => {
                        if args.len() < 1 {
                            tracing::error!("require move args");
                            continue;
                        }

                        let tag = match args[0].parse::<u8>() {
                            Ok(tag) => tag,
                            Err(_) => {
                                tracing::error!("invalid tag: {}", args[0]);
                                continue;
                            },
                        };

                        tracing::debug!("handle move?");
                        tx.send(Ctrl::MoveToTag(tag, None)).await.expect("send error");
                    },
                    "show" => {
                        if args.len() < 1 {
                            tracing::error!("require move args");
                            continue;
                        }

                        let tag = match args[0].parse::<u8>() {
                            Ok(tag) => tag,
                            Err(_) => {
                                tracing::error!("invalid tag: {}", args[0]);
                                continue;
                            },
                        };
                        tx.send(Ctrl::ShowTag(tag)).await.expect("send error");
                    },
                    "toggle" => {
                        if args.len() < 1 {
                            tracing::error!("require move args");
                            continue;
                        }

                        let tag = match args[0].parse::<u8>() {
                            Ok(tag) => tag,
                            Err(_) => {
                                tracing::error!("invalid tag: {}", args[0]);
                                continue;
                            },
                        };
                        tx.send(Ctrl::ToggleTag(tag)).await.expect("send error");
                    },
                    _ => {},
                }
            },
        }
    }
}

pub(crate) fn hyprland_dir() -> anyhow::Result<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")?;
    Ok(Path::new("/tmp/hypr").join(sig))
}

fn parse_line<'a>(line: &'a str) -> anyhow::Result<(&'a str, &'a str)> {
    let line = &line[..line.len() - 1]; // remove \n
    let chunks: Vec<&str> = line.split(">>").collect();

    if chunks.len() >= 2 {
        let args: Vec<&str> = chunks[1].split(",").collect();
        Ok((chunks[0], args[0]))
    } else if chunks.len() == 1 {
        Ok((chunks[0], &""))
    } else {
        bail!("invalid line: {}", line)
    }
}

fn handle_event_stream(state: &mut State, buf: &str) {
    tracing::debug!("[event] {:?}", buf);

    match parse_line(&buf) {
        Err(err) => {
            tracing::error!(%err, "invalid message received");
        },
        Ok((cmd, id)) => {
            if id == "" {
                return;
            }
            match cmd {
                "openwindow" => {
                    if let Err(err) = state.new_window_added(id.into()) {
                        tracing::error!(%err, "openwindow error");
                    }
                },

                "closewindow" => {
                    tracing::info!("closewindow: {}", id);
                    if let Err(err) = state.window_removed(id.into()) {
                        tracing::error!(%err, "closewindow error");
                    }
                },

                "activewindowv2" => {
                    if let Err(err) = state.focus_window_changed(id.into()) {
                        tracing::error!(%err, "activewindowv2 error");
                    }
                },

                _ => (),
            }
        },
    }

    tracing::debug!("state: {:?}", state);
}

fn handle_ctrl(state: &mut State, msg: Ctrl) {
    tracing::debug!(?msg, "handle_ctrl");
    match msg {
        Ctrl::MoveToTag(tag, window) => {
            let changes = match state.move_window(tag, window) {
                Ok(changes) => changes,
                Err(err) => {
                    tracing::error!(%err, "Ctrl::MoveToTag error");
                    return;
                },
            };

            handle_changes(changes);
        },

        Ctrl::ShowTag(tag) => {
            let changes = match state.set_visible_tags(1<<(tag-1)) {
                Ok(changes) => changes,
                Err(err) => {
                    tracing::error!(%err, "Ctrl::ShowTag error");
                    return;
                },
            };
            handle_changes(changes);
        },

        Ctrl::ToggleTag(tag) => {
            let changes = match state.toggle_tag(tag) {
                Ok(changes) => changes,
                Err(err) => {
                    tracing::error!(%err, "Ctrl::ToggleTag error");
                    return;
                },
            };
            handle_changes(changes);
        },
    }
}

fn handle_changes(changes: Changes) {
    let mut args: Vec<String> = vec![];
    args.extend(
        changes.window_removed.iter().map(|w| format!("dispatch movetoworkspacesilent {},address:0x{}", w.tag + 100, w.addr)).collect::<Vec<String>>()
    );
    args.extend(
        changes.window_added.iter().map(|w| format!("dispatch movetoworkspacesilent {},address:0x{}", 1, w.addr)).collect::<Vec<String>>()
    );
    if let Some(focus) = changes.focus {
        args.push(format!("dispatch focuswindow address:0x{}", focus));
    }

    hyprctl(args);
}

#[cfg(test)]
mod tests {
    use crate::parse_line;

    #[test]
    fn test_parse_line() {
        let line = "openwindow>>12345,hoge\n";

        let (command, id) = parse_line(line).unwrap();
        assert_eq!(command, "openwindow");
        assert_eq!(id, "12345");
    }
}
