use std::path::Path;

use anyhow::Result;
use arboard::Clipboard;

use crate::target::{ClipboardFormat, Target};

/// Render the clipboard string for a successful transfer to `target`.
///
/// `local_source`   — the local file that was sent (to derive `{basename}`).
/// `remote_abs_dir` — the resolved absolute directory on the remote (with `~`
///                   already expanded). Clipboard returns `{remote_abs_dir}/{basename}`
///                   as the remote path.
pub fn render(
    target: &Target,
    local_source: &Path,
    remote_abs_dir: &str,
    default_format: ClipboardFormat,
    custom_template: &str,
) -> String {
    let basename = local_source
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let remote_path = format!("{}/{}", remote_abs_dir.trim_end_matches('/'), basename);

    let fmt = target.clipboard_format.unwrap_or(default_format);
    match fmt {
        ClipboardFormat::RemotePath => remote_path,
        ClipboardFormat::ScpStyle => match &target.user {
            Some(u) => format!("{}@{}:{}", u, target.host, remote_path),
            None => format!("{}:{}", target.host, remote_path),
        },
        ClipboardFormat::SshPath => match &target.user {
            Some(u) => format!("ssh://{}@{}{}", u, target.host, remote_path),
            None => format!("ssh://{}{}", target.host, remote_path),
        },
        ClipboardFormat::Custom => render_template(custom_template, target, &remote_path, &basename),
    }
}

fn render_template(template: &str, t: &Target, path: &str, basename: &str) -> String {
    template
        .replace("{user}", t.user.as_deref().unwrap_or(""))
        .replace("{host}", &t.host)
        .replace("{port}", &t.ssh_port().to_string())
        .replace("{path}", path)
        .replace("{basename}", basename)
}

pub fn write(text: &str) -> Result<()> {
    let mut cb = Clipboard::new().map_err(|e| anyhow::anyhow!("clipboard init: {e}"))?;
    cb.set_text(text.to_string())
        .map_err(|e| anyhow::anyhow!("clipboard set: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn target(user: Option<&str>, fmt: Option<ClipboardFormat>) -> Target {
        Target {
            name: "dev".into(),
            host: "dev.example.com".into(),
            user: user.map(|s| s.to_string()),
            remote_dir: "~/uploads".into(),
            ssh_port: None,
            ssh_key: None,
            clipboard_format: fmt,
        }
    }

    #[test]
    fn remote_path_format() {
        let t = target(Some("ubuntu"), None);
        let out = render(
            &t,
            &PathBuf::from("/local/shot.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "/home/ubuntu/uploads/shot.png");
    }

    #[test]
    fn scp_style_with_user() {
        let t = target(Some("ubuntu"), Some(ClipboardFormat::ScpStyle));
        let out = render(
            &t,
            &PathBuf::from("/local/shot.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "ubuntu@dev.example.com:/home/ubuntu/uploads/shot.png");
    }

    #[test]
    fn scp_style_without_user() {
        let t = target(None, Some(ClipboardFormat::ScpStyle));
        let out = render(
            &t,
            &PathBuf::from("/local/shot.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "dev.example.com:/home/ubuntu/uploads/shot.png");
    }

    #[test]
    fn ssh_path_format() {
        let t = target(Some("ubuntu"), Some(ClipboardFormat::SshPath));
        let out = render(
            &t,
            &PathBuf::from("/local/shot.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "ssh://ubuntu@dev.example.com/home/ubuntu/uploads/shot.png");
    }

    #[test]
    fn custom_template_all_placeholders() {
        let t = target(Some("ubuntu"), Some(ClipboardFormat::Custom));
        let out = render(
            &t,
            &PathBuf::from("/local/shot.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{port}:{path} basename={basename}",
        );
        assert_eq!(
            out,
            "ubuntu@dev.example.com:22:/home/ubuntu/uploads/shot.png basename=shot.png"
        );
    }

    #[test]
    fn chinese_filename_scp_style() {
        let t = target(Some("ubuntu"), Some(ClipboardFormat::ScpStyle));
        let out = render(
            &t,
            &PathBuf::from("/local/截图.png"),
            "/home/ubuntu/uploads",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "ubuntu@dev.example.com:/home/ubuntu/uploads/截图.png");
    }

    #[test]
    fn strips_trailing_slash_from_remote_dir() {
        let t = target(Some("ubuntu"), None);
        let out = render(
            &t,
            &PathBuf::from("/local/a.png"),
            "/home/ubuntu/uploads/",
            ClipboardFormat::RemotePath,
            "{user}@{host}:{path}",
        );
        assert_eq!(out, "/home/ubuntu/uploads/a.png");
    }
}
