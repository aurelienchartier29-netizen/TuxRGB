//! Kernel command line, on every common distribution.
//!
//! The SMBus fix needs one kernel parameter (`acpi_enforce_resources=lax`).
//! How to add it depends on the system, so the method is picked at run time:
//!
//! | System                                   | Method                                   |
//! |------------------------------------------|------------------------------------------|
//! | Fedora Atomic, Bazzite, Aurora, Bluefin  | `rpm-ostree kargs --append-if-missing`   |
//! | Fedora, RHEL, Nobara (grubby)            | `grubby --update-kernel=ALL --args`      |
//! | Pop!_OS (kernelstub)                     | `kernelstub --add-options`               |
//! | Debian, Ubuntu, Mint, Arch, openSUSE…    | `/etc/default/grub` + grub-mkconfig      |
//! | systemd-boot (Arch, EndeavourOS…)        | `/boot/loader/entries/*.conf` options    |
//! | rEFInd                                   | `/boot/refind_linux.conf`                |
//! | NixOS, transactional openSUSE            | not changed: instructions are returned   |
//!
//! The text edits are pure functions (unit tested below); the helper only
//! writes files it found at fixed, validated locations, keeps a backup of
//! each and records them in the manifest so « Réinitialiser » can take the
//! parameter out again.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    RpmOstree,
    Grubby,
    Kernelstub,
    /// `/etc/default/grub`, then this regeneration command.
    Grub {
        regen: Vec<&'static str>,
    },
    /// Loader entries (and `/etc/kernel/cmdline` when present).
    SystemdBoot {
        entries: Vec<String>,
    },
    Refind,
    /// Nothing is changed automatically; the message explains what to do.
    Manual(String),
}

impl Method {
    pub fn name(&self) -> &'static str {
        match self {
            Method::RpmOstree => "rpm-ostree",
            Method::Grubby => "grubby",
            Method::Kernelstub => "kernelstub",
            Method::Grub { .. } => "grub",
            Method::SystemdBoot { .. } => "systemd-boot",
            Method::Refind => "refind",
            Method::Manual(_) => "manual",
        }
    }
}

pub const GRUB_DEFAULT: &str = "/etc/default/grub";
pub const KERNEL_CMDLINE: &str = "/etc/kernel/cmdline";
pub const REFIND_CONF: &str = "/boot/refind_linux.conf";
const ENTRY_DIRS: &[&str] = &[
    "/boot/loader/entries",
    "/efi/loader/entries",
    "/boot/efi/loader/entries",
];

/// Paths the helper may ever edit for the kernel command line.
pub fn editable_path(path: &str) -> bool {
    if [GRUB_DEFAULT, KERNEL_CMDLINE, REFIND_CONF].contains(&path) {
        return true;
    }
    ENTRY_DIRS.iter().any(|dir| {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|name| {
                name.ends_with(".conf")
                    && name.len() < 128
                    && !name.starts_with('.')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
            })
    })
}

/// Chooses how to change the kernel command line on this system.
pub fn detect(has_tool: impl Fn(&str) -> bool) -> Method {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let os_id = os_field(&os, "ID");
    if Path::new("/run/ostree-booted").exists() && has_tool("rpm-ostree") {
        return Method::RpmOstree;
    }
    if os_id == "nixos" || Path::new("/etc/NIXOS").exists() {
        return Method::Manual(
            "NixOS : ajoutez boot.kernelParams = [ \"acpi_enforce_resources=lax\" ]; \
             à configuration.nix puis lancez nixos-rebuild switch."
                .into(),
        );
    }
    if has_tool("transactional-update") && read_only_root() {
        return Method::Manual(
            "Système transactionnel : lancez « sudo transactional-update shell », ajoutez \
             acpi_enforce_resources=lax à GRUB_CMDLINE_LINUX_DEFAULT dans /etc/default/grub \
             puis « grub2-mkconfig -o /boot/grub2/grub.cfg », quittez et redémarrez."
                .into(),
        );
    }
    if has_tool("grubby") {
        return Method::Grubby;
    }
    if has_tool("kernelstub") {
        return Method::Kernelstub;
    }
    if Path::new(GRUB_DEFAULT).is_file() {
        let regen: Option<Vec<&'static str>> = if has_tool("update-grub") {
            Some(vec!["update-grub"])
        } else if has_tool("grub-mkconfig") && Path::new("/boot/grub/grub.cfg").exists() {
            Some(vec!["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])
        } else if has_tool("grub2-mkconfig") && Path::new("/boot/grub2/grub.cfg").exists() {
            Some(vec!["grub2-mkconfig", "-o", "/boot/grub2/grub.cfg"])
        } else {
            None
        };
        if let Some(regen) = regen {
            return Method::Grub { regen };
        }
    }
    let entries = loader_entries();
    if !entries.is_empty() {
        return Method::SystemdBoot { entries };
    }
    if Path::new(REFIND_CONF).is_file() {
        return Method::Refind;
    }
    Method::Manual(
        "Chargeur de démarrage non reconnu : ajoutez acpi_enforce_resources=lax aux \
         paramètres du noyau (ligne « options » ou « linux ») puis redémarrez."
            .into(),
    )
}

fn os_field<'a>(os_release: &'a str, key: &str) -> &'a str {
    os_release
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().trim_matches('"'))
        .unwrap_or("")
}

fn read_only_root() -> bool {
    std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .any(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.len() > 3 && f[1] == "/" && f[3].split(',').any(|o| o == "ro")
        })
}

/// systemd-boot entries that carry an `options` line.
fn loader_entries() -> Vec<String> {
    let mut out = Vec::new();
    for dir in ENTRY_DIRS {
        let Ok(it) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in it.flatten() {
            let path = e.path();
            let Some(p) = path.to_str() else { continue };
            if !editable_path(p) || !e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if std::fs::read_to_string(&path)
                .map(|t| t.lines().any(|l| l.trim_start().starts_with("options")))
                .unwrap_or(false)
            {
                out.push(p.to_owned());
            }
        }
    }
    out.sort();
    out
}

fn has_token(line: &str, karg: &str) -> bool {
    line.split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .any(|t| t == karg)
}

/// Adds `karg` to `GRUB_CMDLINE_LINUX_DEFAULT` (or `GRUB_CMDLINE_LINUX`).
/// Returns `None` when it is already there or no such line exists.
pub fn grub_add(text: &str, karg: &str) -> Option<String> {
    let key = ["GRUB_CMDLINE_LINUX_DEFAULT=", "GRUB_CMDLINE_LINUX="]
        .into_iter()
        .find(|k| text.lines().any(|l| l.trim_start().starts_with(k)))?;
    let mut changed = false;
    let mut out = String::with_capacity(text.len() + karg.len() + 2);
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let trimmed = body.trim_start();
        if !changed && trimmed.starts_with(key) {
            if has_token(trimmed, karg) {
                return None;
            }
            let value = &trimmed[key.len()..];
            let new_value = match value.chars().next() {
                Some(q @ ('"' | '\'')) if value.len() >= 2 && value.ends_with(q) => {
                    let inner = value[1..value.len() - 1].trim_end();
                    if inner.is_empty() {
                        format!("{q}{karg}{q}")
                    } else {
                        format!("{q}{inner} {karg}{q}")
                    }
                }
                _ if value.trim().is_empty() => format!("\"{karg}\""),
                _ => format!("\"{} {karg}\"", value.trim()),
            };
            let indent = &body[..body.len() - trimmed.len()];
            out.push_str(&format!("{indent}{key}{new_value}"));
            if line.ends_with('\n') {
                out.push('\n');
            }
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

/// Removes every occurrence of `karg` from the GRUB command-line variables.
pub fn grub_remove(text: &str, karg: &str) -> Option<String> {
    edit_lines(text, karg, |t| {
        t.starts_with("GRUB_CMDLINE_LINUX_DEFAULT=") || t.starts_with("GRUB_CMDLINE_LINUX=")
    })
}

/// Adds `karg` to every `options` line of a loader entry.
pub fn entry_add(text: &str, karg: &str) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(text.len() + karg.len() + 2);
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let t = body.trim_start();
        if (t.starts_with("options ") || t.starts_with("options\t") || t == "options")
            && !has_token(t, karg)
        {
            out.push_str(body.trim_end());
            out.push(' ');
            out.push_str(karg);
            if line.ends_with('\n') {
                out.push('\n');
            }
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

pub fn entry_remove(text: &str, karg: &str) -> Option<String> {
    edit_lines(text, karg, |t| t.starts_with("options"))
}

/// `/etc/kernel/cmdline`: a single line of parameters.
pub fn cmdline_add(text: &str, karg: &str) -> Option<String> {
    if has_token(text, karg) {
        return None;
    }
    let line = text.trim_end();
    Some(if line.is_empty() {
        format!("{karg}\n")
    } else {
        format!("{line} {karg}\n")
    })
}

pub fn cmdline_remove(text: &str, karg: &str) -> Option<String> {
    edit_lines(text, karg, |_| true)
}

/// rEFInd: `"Title" "options"` lines; the option string is the 2nd quote.
pub fn refind_add(text: &str, karg: &str) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(text.len() + 2 * karg.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let quotes: Vec<usize> = body.match_indices('"').map(|(i, _)| i).collect();
        if body.trim_start().starts_with('#') || quotes.len() < 4 || has_token(body, karg) {
            out.push_str(line);
            continue;
        }
        let close = quotes[3];
        let inner = body[quotes[2] + 1..close].trim_end();
        out.push_str(&body[..quotes[2] + 1]);
        out.push_str(inner);
        if !inner.is_empty() {
            out.push(' ');
        }
        out.push_str(karg);
        out.push_str(&body[close..]);
        if line.ends_with('\n') {
            out.push('\n');
        }
        changed = true;
    }
    changed.then_some(out)
}

pub fn refind_remove(text: &str, karg: &str) -> Option<String> {
    edit_lines(text, karg, |t| !t.starts_with('#'))
}

/// Removes the token `karg` (and one separating space) from matching lines.
fn edit_lines(text: &str, karg: &str, matches: impl Fn(&str) -> bool) -> Option<String> {
    let mut changed = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        if matches(body.trim_start()) && has_token(body, karg) {
            let mut s = body.to_owned();
            for pattern in [format!(" {karg}"), format!("{karg} "), karg.to_owned()] {
                if let Some(i) = find_token(&s, &pattern, karg) {
                    s.replace_range(i..i + pattern.len(), "");
                    break;
                }
            }
            out.push_str(&s);
            if line.ends_with('\n') {
                out.push('\n');
            }
            changed = true;
        } else {
            out.push_str(line);
        }
    }
    changed.then_some(out)
}

/// Position of `pattern` (which contains `karg`) where `karg` is a whole
/// token, not part of a longer parameter.
fn find_token(s: &str, pattern: &str, karg: &str) -> Option<usize> {
    let offset = pattern.find(karg)?;
    s.match_indices(pattern).map(|(i, _)| i).find(|&i| {
        let start = i + offset;
        let end = start + karg.len();
        let before = s[..start].chars().next_back();
        let after = s[end..].chars().next();
        let sep = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || c == '"' || c == '\'');
        sep(before) && sep(after)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const K: &str = "acpi_enforce_resources=lax";

    #[test]
    fn grub_line_gets_the_parameter_once() {
        let text = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash\"\nGRUB_CMDLINE_LINUX=\"\"\n";
        let out = grub_add(text, K).unwrap();
        assert!(out.contains(&format!("GRUB_CMDLINE_LINUX_DEFAULT=\"quiet splash {K}\"")));
        assert!(out.contains("GRUB_CMDLINE_LINUX=\"\"\n"));
        assert!(out.starts_with("GRUB_TIMEOUT=5\n"));
        assert_eq!(grub_add(&out, K), None, "never added twice");
        let back = grub_remove(&out, K).unwrap();
        assert_eq!(back, text);
    }

    #[test]
    fn grub_variants() {
        // Only GRUB_CMDLINE_LINUX, single quotes, empty value.
        assert_eq!(
            grub_add("GRUB_CMDLINE_LINUX='rhgb'\n", K).unwrap(),
            format!("GRUB_CMDLINE_LINUX='rhgb {K}'\n")
        );
        assert_eq!(
            grub_add("GRUB_CMDLINE_LINUX_DEFAULT=\"\"\n", K).unwrap(),
            format!("GRUB_CMDLINE_LINUX_DEFAULT=\"{K}\"\n")
        );
        assert_eq!(grub_add("GRUB_TIMEOUT=5\n", K), None);
        // A longer parameter with the same prefix is not "already there".
        let t = "GRUB_CMDLINE_LINUX_DEFAULT=\"acpi_enforce_resources=lax2\"\n";
        assert!(grub_add(t, K).unwrap().contains(&format!("lax2 {K}")));
    }

    #[test]
    fn loader_entry_options() {
        let text = "title Arch Linux\nlinux /vmlinuz-linux\noptions root=UUID=1 rw quiet\n";
        let out = entry_add(text, K).unwrap();
        assert!(out.ends_with(&format!("options root=UUID=1 rw quiet {K}\n")));
        assert_eq!(entry_add(&out, K), None);
        assert_eq!(entry_remove(&out, K).unwrap(), text);
        assert_eq!(entry_add("title x\nlinux /vmlinuz\n", K), None);
    }

    #[test]
    fn kernel_cmdline_file() {
        assert_eq!(
            cmdline_add("root=/dev/sda2 rw\n", K).unwrap(),
            format!("root=/dev/sda2 rw {K}\n")
        );
        assert_eq!(cmdline_add(&format!("rw {K}\n"), K), None);
        assert_eq!(
            cmdline_remove(&format!("rw {K} quiet\n"), K).unwrap(),
            "rw quiet\n"
        );
    }

    #[test]
    fn refind_options() {
        let text =
            "\"Boot with standard options\"  \"root=UUID=1 rw quiet\"\n# comment \"a\" \"b\"\n";
        let out = refind_add(text, K).unwrap();
        assert!(out.starts_with(&format!(
            "\"Boot with standard options\"  \"root=UUID=1 rw quiet {K}\"\n"
        )));
        assert!(out.contains("# comment \"a\" \"b\""));
        assert_eq!(refind_add(&out, K), None);
        assert_eq!(refind_remove(&out, K).unwrap(), text);
    }

    #[test]
    fn only_known_paths_are_editable() {
        assert!(editable_path("/etc/default/grub"));
        assert!(editable_path("/boot/loader/entries/arch.conf"));
        assert!(editable_path("/efi/loader/entries/2024-linux-6.9.conf"));
        for bad in [
            "/boot/loader/entries/../../etc/shadow",
            "/boot/loader/entries/x.conf/../../etc",
            "/boot/loader/entries/.hidden.conf",
            "/boot/loader/entries/sub/x.conf",
            "/etc/passwd",
            "/boot/loader/entries/x.txt",
        ] {
            assert!(!editable_path(bad), "{bad}");
        }
    }
}
