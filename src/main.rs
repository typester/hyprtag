use std::path::{Path, PathBuf};

use anyhow::bail;
use tokio::{net::{UnixStream, UnixListener}, io::{BufStream, AsyncBufReadExt}, sync::mpsc, process::Command};
use tracing_subscriber::EnvFilter;

#[derive(Debug)]
struct State {
    active_tags: u32,
    active_tag: u8,
    active_tag_index: usize,
    active_window: String,
    tags: Vec<Tag>,
}

impl State {
    fn new() -> Self {
        let tags = (0..32).map(|n| Tag::new(n + 1)).collect();
        State {
            active_tags: 1,
            active_tag: 1,
            active_tag_index: 0,
            active_window: "".into(),
            tags,
        }
    }

}

#[derive(Debug)]
struct Tag {
    id: u8,
    windows: Vec<String>,
}

impl Tag {
    fn new(id: u8) -> Self {
        Self {
            id,
            windows: vec![],
        }
    }
}

#[derive(Debug)]
enum Ctrl {
    ShowTag(u32),
    ToggleTag(u32),
    MoveToTag(u32, Option<String>),
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

                        let tag = match args[0].parse::<u32>() {
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

                        let tag = match args[0].parse::<u32>() {
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

                        let tag = match args[0].parse::<u32>() {
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

fn hyprland_dir() -> anyhow::Result<PathBuf> {
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
                },

                "closewindow" => {
                    tracing::info!("closewindow: {}", id);
                    let query = state.tags.iter_mut().enumerate().find_map(|(tag_index, tag)| {
                        tag.windows.iter().enumerate().find_map(|(window_index, w)| {
                            if w == &id {
                                Some((tag_index, window_index))
                            } else {
                                None
                            }
                        })
                    });

                    if let Some((tag_index, window_index)) = query {
                        if let Some(tag) = state.tags.get_mut(tag_index) {
                            tag.windows.remove(window_index);
                        }
                    }
                },

                "activewindowv2" => {
                    let active_tag = state.tags.iter().find(|tag| {
                        tag.windows.iter().find(|w| w == &id).is_some()
                    });

                    if let Some(active_tag) = active_tag {
                        tracing::info!("active window: {}", id);
                        state.active_tag = active_tag.id;
                        state.active_tag_index = (active_tag.id - 1) as usize;
                        state.active_window = id.into();
                    } else {
                        tracing::info!("open window: {}", id);
                        // open window
                        if let Some(tag) = state.tags.get_mut(state.active_tag_index) {
                            tag.windows.push(id.into());
                        }
                        state.active_window = id.into();
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
            let target_index = (tag - 1) as usize;
            let window = match window {
                Some(w) => w,
                None => state.active_window.clone(),
            };
            if window == "" {
                return;
            }

            tracing::debug!("window: {}", window);

            let query = state.tags.iter_mut().enumerate().find_map(|(tag_index, tag)| {
                tag.windows.iter().enumerate().find_map(|(window_index, w)| {
                    if w == &window {
                        Some((tag_index, window_index))
                    } else {
                        None
                    }
                })
            });

            if let Some((tag_index, window_index)) = query {
                if tag_index == target_index {
                    tracing::debug!(%window, "the window is already on tag:{}", tag_index + 1);
                } else {
                    let current_tag = state.tags.get_mut(tag_index).expect("out of index");
                    current_tag.windows.remove(window_index);

                    let target_tag = match state.tags.get_mut(target_index) {
                        None => {
                            tracing::error!("tag is out of range");
                            return;
                        },
                        Some(t) => t,
                    };
                    target_tag.windows.push(window.clone());

                    if state.active_tag == target_tag.id {
                        // show
                        hyprctl(vec![
                            format!("dispatch movetoworkspacesilent {},address:0x{}", 1, window),
                        ]);
                    } else {
                        // hide
                        hyprctl(vec![
                            format!("dispatch movetoworkspacesilent {},address:0x{}", 100+target_index, window),
                        ]);
                    }
                }
            }
        },

        Ctrl::ShowTag(tag) => {
            let target_index = tag - 1;
            for n in 0..32 {
                if state.active_tags & 1<<n != 0 {
                    if n != target_index {
                        // hide
                        let t = state.tags.get_mut(n as usize).unwrap();
                        let arg = t.windows.iter().map(|w| {
                            format!("dispatch movetoworkspacesilent {},address:0x{}", 100+n, w)
                        }).collect();
                        hyprctl(arg);
                    }
                } else {
                    if n == target_index {
                        // show
                        let t = state.tags.get_mut(n as usize).unwrap();
                        let mut arg: Vec<String> = t.windows.iter().map(|w| {
                            format!("dispatch movetoworkspacesilent 1,address:0x{}", w)
                        }).collect();

                        // keep focus or focus first window
                        let focus = t.windows.iter().find(|w| **w == state.active_window)
                            .or(t.windows.iter().next());
                        if let Some(focus) = focus {
                            arg.push(format!("dispatch focuswindow address:0x{}", focus));
                        }
                        hyprctl(arg);
                    }
                }
            }
            state.active_tag = tag as u8;
            state.active_tags = 1<<target_index;
            state.active_tag_index = (tag - 1) as usize;
        },

        Ctrl::ToggleTag(tag) => {
            let target_index = (tag - 1) as usize;
            if state.active_tags & (1<<target_index) == 0 {
                // show
                let t = match state.tags.get_mut(target_index) {
                    None => {
                        return
                    },
                    Some(t) => t,
                };
                let arg = t.windows.iter().map(|w| {
                    format!("dispatch movetoworkspacesilent 1,address:0x{}", w)
                }).collect();
                hyprctl(arg);

                state.active_tags = state.active_tags | (1 << target_index);
            } else {
                tracing::debug!(%state.active_tags, %target_index, "toggle off"); 
                let active_tags = state.active_tags & !(1<<target_index);
                if active_tags == 0 {
                    tracing::error!("cannot toggle last active tag");
                    return;
                }

                // hide
                let t = match state.tags.get_mut(target_index) {
                    None => {
                        return
                    },
                    Some(t) => t,
                };
                let arg = t.windows.iter().map(|w| {
                    format!("dispatch movetoworkspacesilent {},address:0x{}", 100+target_index, w)
                }).collect();
                hyprctl(arg);

                state.active_tags = active_tags;
            }

            let mut windows: Vec<String> = vec![];
            for n in 0..32 {
                if state.active_tags & 1<<n != 0 {
                    if let Some(tags) = state.tags.get(n) {
                        windows.extend(tags.windows.clone());
                    }
                }
            }
            let focus = windows.iter().find(|w| **w == state.active_window)
                .or(windows.iter().next());
            if let Some(focus) = focus {
                hyprctl(vec![format!("dispatch focuswindow address:0x{}", focus)])
            }
        },
    }
}

fn hyprctl(args: Vec<String>) {
    if args.len() == 0 {
        tracing::debug!("no args");
    }
    tokio::spawn(async move {
        let args = vec![
            "--batch".into(),
            args.join(";"),
        ];
        tracing::debug!("hyprctl: {}", args.join(" "));
        let out = match Command::new("hyprctl")
            .args(args)
            .output()
            .await
        {
            Ok(out) => out,
            Err(err) => {
                tracing::error!(%err, "failed to exec hyprtl");
                return;
            },
        };

        tracing::debug!("hyprctl result: {:?}", out);
    });
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
