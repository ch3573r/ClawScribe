//! Open validated web links without invoking a command shell.
use std::process::Command;

fn browser_command(value: &str) -> Result<Command, String> {
    let invalid = || "Only web links can be opened.".to_string();
    let url = url::Url::parse(value).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(invalid());
    }
    // URL parsers accept some punctuation in opaque domain names. Browser
    // destinations must be DNS names or IP literals, not shell-shaped hosts.
    if let Some(url::Host::Domain(host)) = url.host() {
        if !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            return Err(invalid());
        }
    }
    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else if cfg!(target_os = "macos") {
        Command::new("open")
    } else {
        Command::new("xdg-open")
    };
    command.arg(url.as_str());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

pub(crate) fn open_url_in_default_browser(value: &str) -> Result<(), String> {
    browser_command(value)?
        .spawn()
        .map_err(|_| "Could not open the web link. Check your default browser.".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_query_is_one_intact_launcher_argument() {
        let value = "https://example.com/a?b=1&c=2";
        let command = browser_command(value).unwrap();
        assert_eq!(command.get_args().last().unwrap(), value);
        assert_ne!(command.get_program(), "cmd");
        assert_eq!(
            command.get_args().count(),
            if cfg!(windows) { 2 } else { 1 }
        );
    }

    #[test]
    fn non_web_and_invalid_host_targets_are_refused() {
        for value in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            r"C:\Windows\System32\calc.exe",
            "https://x&calc",
            "ms-settings:",
            "",
        ] {
            assert_eq!(
                browser_command(value).unwrap_err(),
                "Only web links can be opened."
            );
        }
    }
}
