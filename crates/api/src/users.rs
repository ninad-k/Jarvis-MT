//! `jarvis-api user ...`: create, list, reset and remove login accounts.
//!
//! These commands open the same database as the server and need `JARVIS_JWT_SECRET`
//! (TOTP secrets are encrypted with a key derived from it). Run them on the machine that
//! runs Jarvis; there is no network interface for managing users.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, bail};
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

use crate::audit::Audit;
use crate::auth::crypto::{PasswordHasherCfg, validate_new_password, validate_username};
use crate::auth::{Accounts, Enrollment};
use crate::cli::UserCommand;
use crate::config::Config;
use crate::state::JWT_SECRET_ENV;

/// Run one `user` subcommand against the configured database.
pub async fn run(config_path: Option<&Path>, command: UserCommand) -> anyhow::Result<()> {
    let config = Config::load_with(config_path)?;
    let secret = std::env::var(JWT_SECRET_ENV).unwrap_or_default();
    if secret.is_empty() {
        bail!(
            "{JWT_SECRET_ENV} is not set. Put it in .env (see .env.example); the TOTP secrets \
             are encrypted with a key derived from it."
        );
    }
    let audit = Audit::connect(&config.database.url).await?;
    let accounts = Accounts::new(
        audit.pool().clone(),
        &secret,
        PasswordHasherCfg::owasp(),
        std::sync::Arc::new(crate::clock::WallClock),
    )?;
    let mut out = std::io::stdout().lock();
    execute(&accounts, command, &mut out, &mut std::io::stdin().lock()).await
}

/// The commands, with output and (for `--password-stdin` and confirmations) input injected.
pub async fn execute(
    accounts: &Accounts,
    command: UserCommand,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
) -> anyhow::Result<()> {
    match command {
        UserCommand::Add {
            name,
            password_stdin,
        } => {
            validate_username(&name)?;
            let password = read_new_password(password_stdin, input)?;
            let enrollment = accounts.add_user(&name, &password).await?;
            writeln!(out, "User '{name}' created.")?;
            print_enrollment(out, &enrollment)?;
        }
        UserCommand::ResetTotp { name } => {
            let enrollment = accounts.reset_totp(&name).await?;
            writeln!(
                out,
                "New authenticator secret for '{name}'. Their sessions were ended and any \
                 lockout cleared. The old secret no longer works."
            )?;
            print_enrollment(out, &enrollment)?;
        }
        UserCommand::List => {
            let users = accounts.list_users().await?;
            if users.is_empty() {
                writeln!(out, "No users. Create one with: jarvis-api user add <name>")?;
            }
            let now_ms = chrono::Utc::now().timestamp_millis();
            for u in users {
                let locked = u.locked_until.is_some_and(|until| until > now_ms);
                writeln!(
                    out,
                    "{}\tcreated {}\tlast login {}{}",
                    u.username,
                    u.created_at,
                    u.last_login_at.as_deref().unwrap_or("never"),
                    if locked { "\tLOCKED" } else { "" }
                )?;
            }
        }
        UserCommand::Remove { name, yes } => {
            if !yes {
                write!(out, "Remove user '{name}'? Type the name to confirm: ")?;
                out.flush()?;
                let mut line = String::new();
                input.read_line(&mut line)?;
                if line.trim() != name {
                    bail!("not confirmed; nothing removed");
                }
            }
            if accounts.remove_user(&name).await? {
                writeln!(
                    out,
                    "User '{name}' removed. Their sessions stop working at once."
                )?;
            } else {
                bail!("no user named '{name}'");
            }
        }
    }
    Ok(())
}

/// Ask for the password twice without echo, or read one line from stdin.
fn read_new_password(from_stdin: bool, input: &mut dyn BufRead) -> anyhow::Result<String> {
    let password = if from_stdin {
        let mut line = String::new();
        input
            .read_line(&mut line)
            .context("reading the password from stdin")?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        let first = rpassword::prompt_password("Password (at least 12 characters): ")
            .context("reading the password")?;
        let second =
            rpassword::prompt_password("Repeat the password: ").context("reading the password")?;
        if first != second {
            bail!("the two passwords do not match");
        }
        first
    };
    validate_new_password(&password)?;
    Ok(password)
}

fn print_enrollment(out: &mut dyn Write, e: &Enrollment) -> anyhow::Result<()> {
    writeln!(
        out,
        "\nAdd this account to your authenticator app (scan the QR code, or enter the key by hand).\n\
         It is shown once; running `user reset-totp` makes a new one.\n"
    )?;
    writeln!(out, "{}", qr_text(&e.uri)?)?;
    writeln!(out, "URI: {}", e.uri)?;
    writeln!(out, "Key: {}", e.secret_base32)?;
    Ok(())
}

/// The URI as a QR code drawn with Unicode half blocks (light on dark, as in a terminal).
pub fn qr_text(uri: &str) -> anyhow::Result<String> {
    let code = QrCode::new(uri.as_bytes()).context("the otpauth URI does not fit in a QR code")?;
    Ok(code
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_is_drawn_for_an_otpauth_uri() {
        let art =
            qr_text("otpauth://totp/Jarvis%20MT:ninad?secret=ABCDEFGH&issuer=Jarvis%20MT").unwrap();
        assert!(art.lines().count() > 10);
        assert!(art.contains('\u{2588}') || art.contains('\u{2580}') || art.contains('\u{2584}'));
    }
}
