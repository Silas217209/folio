//! Desktop handoff: email opens a composer with a retained PDF attached;
//! Share opens the desktop application chooser. Folio does not send mail itself.
use anyhow::{Context, Result};
use std::path::Path;
#[derive(Clone, Copy, Debug)]
pub enum Intent {
    Keep,
    Email,
    ChooseApp,
}
#[cfg(target_os = "linux")]
pub async fn handoff(path: &Path, intent: Intent, subject: &str) -> Result<()> {
    let (result, message) = match intent {
        Intent::Keep => return Ok(()),
        Intent::Email => {
            let file = std::fs::File::open(path)?;
            (
                ashpd::desktop::email::EmailRequest::default()
                    .subject(Some(subject))
                    .attach(file.into())
                    .send()
                    .await,
                "Could not open the email composer with the PDF attached",
            )
        }
        Intent::ChooseApp => {
            let file = std::fs::File::open(path)?;
            (
                ashpd::desktop::open_uri::OpenFileRequest::default()
                    .ask(Some(true))
                    .writeable(Some(false))
                    .send_file(&file)
                    .await,
                "Could not open the desktop application chooser",
            )
        }
    };
    match result.and_then(|request| request.response()) {
        Ok(()) | Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Ok(()),
        Err(error) => Err(error).context(message),
    }
}

#[cfg(target_os = "macos")]
pub async fn handoff(path: &Path, intent: Intent, subject: &str) -> Result<()> {
    let mode = match intent {
        Intent::Keep => return Ok(()),
        Intent::Email => "email",
        Intent::ChooseApp => "share",
    };
    native_handoff(&crate::runtime::share_helper()?, mode, path, subject)
}

#[cfg(any(target_os = "macos", test))]
fn native_handoff(helper: &Path, mode: &str, path: &Path, subject: &str) -> Result<()> {
    use std::{io::BufRead, process::Stdio};
    let mut child = std::process::Command::new(helper)
        .arg(mode)
        .arg(
            path.canonicalize()
                .context("Could not locate the retained PDF")?,
        )
        .arg(subject)
        .stdout(Stdio::piped())
        .spawn()
        .context("Could not open native macOS sharing")?;
    let mut response = String::new();
    let result = std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut response);
    // The helper retains the native sharing service; don't keep the editor's
    // export button busy while the user writes their email. Later service errors
    // are presented by the helper using a native alert. Reap it after completion.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    result.context("The macOS sharing helper did not respond")?;
    anyhow::ensure!(
        response.trim() == "READY",
        "macOS sharing failed: {}",
        response
            .trim()
            .strip_prefix("ERROR\t")
            .unwrap_or("The native sharing helper exited before opening")
    );
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub async fn handoff(_: &Path, intent: Intent, _: &str) -> Result<()> {
    match intent {
        Intent::Keep => Ok(()),
        _ => anyhow::bail!("PDF sharing is not implemented on this platform"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn native_handoff_preserves_arguments_and_reports_launch_and_service_errors() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("Invoice 'quoted' name.pdf");
        std::fs::write(&path, "PDF").unwrap();
        let helper = temporary.path().join("sharing helper");
        // Check the argument vector rather than interpreting user input as code.
        std::fs::write(
            &helper,
            r#"#!/bin/sh
[ "$1" = email ] || exit 2
[ -f "$2" ] || exit 3
[ "$3" = 'Invoice $(false) with spaces' ] || exit 4
printf 'READY\n'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        native_handoff(&helper, "email", &path, "Invoice $(false) with spaces").unwrap();
        std::fs::write(
            &helper,
            "#!/bin/sh\nprintf 'ERROR\tNo mail application\n'\nexit 1\n",
        )
        .unwrap();
        let error = native_handoff(&helper, "email", &path, "subject").unwrap_err();
        assert!(error.to_string().contains("No mail application"));
        std::fs::write(&helper, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(
            native_handoff(&helper, "share", &path, "subject")
                .unwrap_err()
                .to_string()
                .contains("exited before opening")
        );
        std::fs::remove_file(&helper).unwrap();
        assert!(native_handoff(&helper, "share", &path, "subject").is_err());
    }
}
