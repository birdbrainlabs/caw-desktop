//! Types into the focused window. Linux/Wayland uses `wtype`, which talks to
//! the compositor's virtual-keyboard protocol directly (no portal, no prompt).

use anyhow::{bail, Context, Result};
use tokio::process::Command;

pub async fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    run(&["--", text]).await
}

pub async fn press_key(key: &str, count: u32) -> Result<()> {
    let name = match key {
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "enter" => "Return",
        "tab" => "Tab",
        "escape" => "Escape",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "home" => "Home",
        "end" => "End",
        other => bail!("unknown key {other:?}"),
    };
    let count = count.clamp(1, 200);
    let mut args = Vec::with_capacity(count as usize * 2);
    for _ in 0..count {
        args.extend(["-k", name]);
    }
    run(&args).await
}

async fn run(args: &[&str]) -> Result<()> {
    // CAW_DRY_RUN=1: log instead of typing (for tests).
    if std::env::var_os("CAW_DRY_RUN").is_some() {
        eprintln!("caw-desktop: [dry run] wtype {args:?}");
        return Ok(());
    }
    let out = Command::new("wtype").args(args).output().await.context("running wtype (is it installed?)")?;
    if !out.status.success() {
        bail!("wtype failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}
