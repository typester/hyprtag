use serde::Deserialize;
use tokio::{net::UnixStream, io::{BufStream, AsyncWriteExt, AsyncBufReadExt}, process::Command};

use crate::hyprland_dir;

#[derive(Debug, Deserialize)]
pub struct MonitorInfo {
    pub id: u8,
    pub name: String,
    pub focused: bool,
}

pub async fn hyprctl_monitors() -> anyhow::Result<Vec<MonitorInfo>> {
    let out = Command::new("hyprctl").args(vec!["monitors", "-j"]).output().await?;
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub fn hyprctl_batch(args: Vec<String>) {
    if args.len() == 0 {
        tracing::debug!("no args");
        return;
    }

    tokio::spawn(async move {
        if let Err(err) = hyprctl_with_cmd(args).await {
            tracing::error!(%err, "hyprctl err");
        }
    });
}

async fn hyprctl_with_sock(args: Vec<String>) -> anyhow::Result<()> {
    let socket = hyprland_dir()?.join(".socket.sock").to_string_lossy().to_string();
    let socket = UnixStream::connect(socket).await?;
    let mut stream = BufStream::new(socket);

    let mut buf = String::new();

    for arg in args.iter() {
        buf.truncate(0);
        buf.push_str(arg);
        buf.push_str("\n");
        stream.write_all(buf.as_bytes()).await?;

        stream.read_line(&mut buf).await?;
        if !buf.starts_with("ok") {
            tracing::error!(%buf, "hyprctl returns error");
        }
    }

    Ok(())
}

async fn hyprctl_with_cmd(args: Vec<String>) -> anyhow::Result<()> {
    let args = vec![
        "--batch".into(),
        args.join(";"),
    ];
    tracing::debug!("hyprctl: {}", args.join(" "));
    let out = Command::new("hyprctl").args(args).output().await?;

    tracing::debug!("hyprctl result: {:?}", out);

    Ok(())
}
