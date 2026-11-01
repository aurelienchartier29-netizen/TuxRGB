//! Strict validation of udev rules received on stdin.
//!
//! OpenRGB's generated rules only *match* devices and grant access to the
//! logged-in user (`TAG+="uaccess"`: an ACL for the person sitting at the
//! active session, nobody else). Anything able to run code or change
//! ownership (`RUN`, `PROGRAM`, `IMPORT`, `OWNER`, `SYMLINK`…) is rejected,
//! so the helper cannot be abused to install arbitrary udev rules as root.
//!
//! No universal permission is ever installed either: [`sanitize`] removes
//! the few OpenRGB rules that make sysfs files writable by every user
//! (`chmod a+w`, ASUS / faustus laptop keyboards) and the validator refuses
//! any `MODE` giving rights to "others". [`neutralize`] even revokes such
//! grants coming from a distribution's own rules.

pub const MAX_RULES_BYTES: usize = 4 * 1024 * 1024;
/// Longest accepted line (the longest OpenRGB rule is ~200 characters).
pub const MAX_LINE: usize = 4096;
/// Marker written in TuxRGB's rules files (diagnostics check it).
pub const RULES_FORMAT: &str = "# TuxRGB rules format 2: uaccess only, no world-writable grants.";

pub struct Validated {
    pub text: String,
    pub rule_count: usize,
}

const MATCH_KEYS: &[&str] = &[
    "ACTION",
    "SUBSYSTEM",
    "SUBSYSTEMS",
    "KERNEL",
    "KERNELS",
    "DRIVER",
    "DRIVERS",
    "DEVPATH",
    "TAG",
    "TAGS",
];
const MATCH_PREFIXES: &[&str] = &["ATTR{", "ATTRS{", "ENV{"];

pub fn validate(input: &str) -> Result<Validated, String> {
    if input.contains('\0') {
        return Err("caractère NUL dans les règles".into());
    }
    let mut out = String::with_capacity(input.len());
    let mut rule_count = 0usize;
    for (n, raw) in input.lines().enumerate() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if line.len() > MAX_LINE {
            return Err(format!("ligne {}: trop longue", n + 1));
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(format!("ligne {}: caractère de contrôle", n + 1));
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if line.ends_with('\\') {
            return Err(format!(
                "ligne {}: continuation de ligne non autorisée",
                n + 1
            ));
        }
        if !trimmed.is_ascii() {
            return Err(format!(
                "ligne {}: caractère non ASCII dans une règle",
                n + 1
            ));
        }
        validate_rule(trimmed).map_err(|e| format!("ligne {}: {e}", n + 1))?;
        rule_count += 1;
        out.push_str(trimmed);
        out.push('\n');
    }
    if rule_count == 0 {
        return Err("aucune règle udev reçue".into());
    }
    Ok(Validated {
        text: out,
        rule_count,
    })
}

fn validate_rule(rule: &str) -> Result<(), String> {
    let mut grants = 0;
    for token in split_tokens(rule)? {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (key, op, value) = parse_token(token)?;
        match op {
            "==" | "!=" => {
                let ok = MATCH_KEYS.contains(&key)
                    || MATCH_PREFIXES
                        .iter()
                        .any(|p| key.starts_with(p) && key.ends_with('}'));
                if !ok {
                    return Err(format!("clé de correspondance interdite `{key}`"));
                }
            }
            "=" | "+=" => {
                match (key, op, value) {
                    ("TAG", "+=", "uaccess") => grants += 1,
                    // OpenRGB tags each rule with the controller name.
                    ("TAG", "+=", t) if is_tag(t) => {}
                    // Only ever *removes* write access from "group" and "others"
                    // (written by `neutralize`, harmless by construction).
                    ("RUN", "+=", cmd) if is_sysfs_restrict(cmd) => {}
                    ("MODE", "=", m) if is_safe_mode(m) => grants += 1,
                    ("GOTO", "=", label) | ("LABEL", "=", label) if is_label(label) => {}
                    _ => return Err(format!("affectation interdite `{token}`")),
                }
            }
            _ => return Err(format!("opérateur interdit `{op}`")),
        }
        if value.contains('`') || value.contains('$') {
            return Err(format!("valeur interdite `{value}`"));
        }
    }
    let _ = grants;
    Ok(())
}

/// Identity of a rule: its match part (`KEY==value` tokens, sorted), so the
/// same device rule is recognised even when OpenRGB renamed its tag.
fn rule_key(rule: &str) -> Option<String> {
    let mut parts: Vec<String> = split_tokens(rule)
        .ok()?
        .into_iter()
        .filter_map(|t| {
            let (k, op, v) = parse_token(t.trim()).ok()?;
            matches!(op, "==" | "!=").then(|| format!("{k}{op}{v}"))
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort();
    Some(parts.join(","))
}

/// Rules of `ours` that `vendor` (the distribution's 60-openrgb.rules) does
/// not already have, each with the comment block right above it. Returns the
/// text and the number of rules.
pub fn complement(ours: &str, vendor: &str) -> (String, usize) {
    let known: std::collections::HashSet<String> = vendor
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(rule_key)
        .collect();
    let mut out = String::new();
    let mut comments: Vec<&str> = Vec::new();
    let mut count = 0;
    for line in ours.lines() {
        let t = line.trim();
        if t.is_empty() {
            comments.clear();
        } else if t.starts_with('#') {
            comments.push(line);
        } else {
            if rule_key(t).is_none_or(|k| !known.contains(&k)) {
                for c in comments.drain(..) {
                    out.push_str(c);
                    out.push('\n');
                }
                out.push_str(t);
                out.push_str("\n\n");
                count += 1;
            }
            comments.clear();
        }
    }
    (out, count)
}

/// Owner and group only: never any right for "others".
fn is_safe_mode(mode: &str) -> bool {
    matches!(mode, "0600" | "0660" | "600" | "660")
}

/// `MODE` value giving read or write access to every user.
fn is_world_mode(mode: &str) -> bool {
    let digits = mode.trim_start_matches('0');
    u32::from_str_radix(mode, 8)
        .map(|m| m & 0o007 != 0)
        .unwrap_or(true)
        && !digits.is_empty()
}

fn is_sysfs_path(path: &str) -> bool {
    (path.starts_with("/sys/") || path.starts_with("/sys%p/"))
        && !path.contains("..")
        && path.len() < 256
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/_-.%:".contains(c))
}

/// `/usr/bin/env chmod go-w /sys/...`: takes write access away.
fn is_sysfs_restrict(cmd: &str) -> bool {
    cmd.strip_prefix("/usr/bin/env chmod go-w ")
        .is_some_and(is_sysfs_path)
}

/// One rule grants write access to every user (`chmod a+w`, `o+w`, `666`…).
fn world_writable_grant(rule: &str) -> Option<String> {
    for token in split_tokens(rule).ok()? {
        let Ok((key, op, value)) = parse_token(token.trim()) else {
            continue;
        };
        match (key, op) {
            ("RUN", "+=") if value.contains("chmod") && !is_sysfs_restrict(value) => {
                return Some(value.to_owned());
            }
            ("MODE", "=" | ":=") if is_world_mode(value) => return Some(format!("MODE={value}")),
            _ => {}
        }
    }
    None
}

/// Removes the rules that would give every user write access (OpenRGB's
/// `chmod a+w` on ASUS / faustus laptop keyboard sysfs files). They are
/// kept as comments so the file still documents them. Returns the text and
/// a description of each removed rule.
pub fn sanitize(input: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(input.len());
    let mut removed = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        if !t.is_empty()
            && !t.starts_with('#')
            && let Some(grant) = world_writable_grant(t)
        {
            out.push_str("# TuxRGB : règle retirée (accès en écriture pour tous) : ");
            out.push_str(t);
            out.push('\n');
            removed.push(grant);
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, removed)
}

/// For every rule of the distribution's file that makes a sysfs file
/// writable by everyone, a rule with the same match that takes that right
/// back (`chmod go-w`, run after it since 61 > 60). Returns the text and the
/// number of revocations.
pub fn neutralize(vendor: &str) -> (String, usize) {
    let mut out = String::new();
    let mut count = 0;
    for line in vendor.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Ok(tokens) = split_tokens(t) else {
            continue;
        };
        let mut matches = Vec::new();
        let mut paths = Vec::new();
        for token in tokens {
            let Ok((key, op, value)) = parse_token(token.trim()) else {
                continue;
            };
            match op {
                "==" | "!=" => matches.push(token.trim().to_owned()),
                "+=" if key == "RUN" => {
                    if let Some(p) = value
                        .strip_prefix("/usr/bin/env chmod a+w ")
                        .or_else(|| value.strip_prefix("/usr/bin/env chmod o+w "))
                        .filter(|p| is_sysfs_path(p))
                    {
                        paths.push(p.to_owned());
                    }
                }
                _ => {}
            }
        }
        if matches.is_empty() {
            continue;
        }
        for p in paths {
            out.push_str(&format!(
                "{}, RUN+=\"/usr/bin/env chmod go-w {p}\"\n",
                matches.join(", ")
            ));
            count += 1;
        }
    }
    (out, count)
}

/// Tag names are inert labels, except `systemd` which can start units.
fn is_tag(s: &str) -> bool {
    !s.is_empty()
        && s != "systemd"
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Splits on commas outside double quotes.
fn split_tokens(rule: &str) -> Result<Vec<&str>, String> {
    let mut tokens = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (i, c) in rule.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                tokens.push(&rule[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if in_quotes {
        return Err("guillemet non fermé".into());
    }
    tokens.push(&rule[start..]);
    Ok(tokens)
}

/// `KEY{attr}OP"value"` → (key, op, value).
fn parse_token(token: &str) -> Result<(&str, &str, &str), String> {
    let quote = token
        .find('"')
        .ok_or_else(|| format!("valeur sans guillemets `{token}`"))?;
    if !token.ends_with('"') || token.len() < quote + 2 {
        return Err(format!("valeur mal formée `{token}`"));
    }
    let value = &token[quote + 1..token.len() - 1];
    if value.contains('"') {
        return Err(format!("valeur mal formée `{token}`"));
    }
    let head = token[..quote].trim_end();
    for op in ["==", "!=", "+=", "-=", ":=", "="] {
        if let Some(key) = head.strip_suffix(op) {
            let key = key.trim_end();
            if key.is_empty()
                || !key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_{}-.*?/[]".contains(c))
            {
                return Err(format!("clé invalide `{key}`"));
            }
            return Ok((key, op, value));
        }
    }
    Err(format!("opérateur manquant `{token}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines `displays::hardware_display::udev_rules` adds (same format).
    #[test]
    fn accepts_the_hardware_display_rules() {
        let input = "\n# TuxRGB 3.1 – hardware displays (AIO screens, digital displays):\n\
                     # access for the logged-in user only.\n\
                     SUBSYSTEMS==\"usb|hidraw\", ATTRS{idVendor}==\"1e71\", ATTRS{idProduct}==\"3008\", TAG+=\"uaccess\", TAG+=\"TuxRGB_Display\"\n\
                     SUBSYSTEMS==\"usb|hidraw\", ATTRS{idVendor}==\"3633\", ATTRS{idProduct}==\"0002\", TAG+=\"uaccess\", TAG+=\"TuxRGB_Display\"\n";
        let v = validate(input).unwrap();
        assert_eq!(v.rule_count, 2);
        let (clean, removed) = sanitize(input);
        assert!(removed.is_empty());
        assert_eq!(validate(&clean).unwrap().rule_count, 2);
    }

    #[test]
    fn accepts_openrgb_style_rules() {
        let input = r#"#-----------------------------------------------#
# OpenRGB udev rules
#-----------------------------------------------#
SUBSYSTEM=="i2c-dev", KERNEL=="i2c-[0-99]*", TAG+="uaccess"
SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c539", TAG+="uaccess"
ACTION!="add|change", GOTO="openrgb_end"
LABEL="openrgb_end"
SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="2516", ATTRS{idProduct}=="0051", TAG+="uaccess", TAG+="AMD_Wraith_Prism"
ACTION=="add", SUBSYSTEM=="leds", KERNEL=="asus::kbd_backlight", RUN+="/usr/bin/env chmod a+w /sys%p/kbd_rgb_mode"
ACTION=="add", SUBSYSTEM=="platform", KERNEL=="faustus", RUN+="/usr/bin/env chmod a+w /sys/bus/platform/devices/%k/kbbl/kbbl_red"
"#;
        // Raw: the world-writable grants are refused…
        assert!(validate(input).is_err());
        // …sanitized: they are commented out, everything else is kept.
        let (clean, removed) = sanitize(input);
        assert_eq!(removed.len(), 2);
        let v = validate(&clean).unwrap();
        assert_eq!(v.rule_count, 5);
        assert!(
            !v.text
                .lines()
                .any(|l| !l.starts_with('#') && l.contains("a+w"))
        );
    }

    #[test]
    fn never_installs_universal_permissions() {
        for bad in [
            r#"KERNEL=="i2c-[0-99]*", MODE="0666""#,
            r#"KERNEL=="i2c-[0-99]*", MODE="0664""#,
            r#"KERNEL=="i2c-[0-99]*", MODE="0777""#,
            r#"KERNEL=="i2c-[0-99]*", MODE="0606""#,
            r#"SUBSYSTEM=="leds", RUN+="/usr/bin/env chmod a+w /sys%p/brightness""#,
            r#"SUBSYSTEM=="leds", RUN+="/usr/bin/env chmod o+w /sys%p/brightness""#,
            r#"SUBSYSTEM=="leds", RUN+="/usr/bin/env chmod 777 /sys%p/brightness""#,
        ] {
            assert!(validate(bad).is_err(), "accepted: {bad}");
            let (clean, removed) = sanitize(bad);
            assert_eq!(removed.len(), 1, "{bad}");
            assert!(clean.starts_with('#'));
        }
        assert!(validate(r#"KERNEL=="i2c-[0-99]*", TAG+="uaccess", MODE="0660""#).is_ok());
    }

    #[test]
    fn neutralizes_vendor_world_writable_rules() {
        let vendor = r#"ACTION=="add", SUBSYSTEM=="leds", KERNEL=="asus::kbd_backlight", RUN+="/usr/bin/env chmod a+w /sys%p/brightness"
ACTION=="add", SUBSYSTEM=="leds", KERNEL=="x", RUN+="/usr/bin/env chmod a+w /etc/shadow"
SUBSYSTEMS=="usb|hidraw", ATTRS{idVendor}=="046d", TAG+="uaccess"
"#;
        let (fix, n) = neutralize(vendor);
        assert_eq!(n, 1, "{fix}");
        assert!(fix.contains(
            r#"KERNEL=="asus::kbd_backlight", RUN+="/usr/bin/env chmod go-w /sys%p/brightness""#
        ));
        assert!(!fix.contains("shadow"));
        assert!(validate(&fix).is_ok());
    }

    #[test]
    fn rejects_control_characters_and_huge_lines() {
        assert!(validate("SUBSYSTEM==\"usb\", TAG+=\"uaccess\"\u{1b}[2J").is_err());
        let long = format!("SUBSYSTEM==\"{}\", TAG+=\"uaccess\"", "a".repeat(5000));
        assert!(validate(&long).is_err());
    }

    #[test]
    fn rejects_dangerous_rules() {
        for bad in [
            r#"SUBSYSTEM=="usb", RUN+="/bin/sh -c 'id'""#,
            r#"SUBSYSTEM=="usb", PROGRAM=="/bin/true""#,
            r#"SUBSYSTEM=="usb", OWNER="attacker""#,
            r#"SUBSYSTEM=="usb", MODE="4777""#,
            r#"SUBSYSTEM=="usb", IMPORT{program}="x""#,
            r#"SUBSYSTEM=="usb", TAG+="systemd", ENV{SYSTEMD_WANTS}="evil.service""#,
            r#"SUBSYSTEM=="usb", TAG+="uaccess" \"#,
            r#"SUBSYSTEM=="usb, TAG+="uaccess""#,
            r#"SUBSYSTEM=="usb", TAG+="systemd""#,
            r#"SUBSYSTEM=="usb", RUN+="/usr/bin/env chmod a+w /etc/shadow""#,
            r#"SUBSYSTEM=="usb", RUN+="/usr/bin/env chmod a+w /sys/../etc/shadow""#,
            r#"SUBSYSTEM=="usb", RUN+="/usr/bin/env chmod a+w /sys/x;reboot""#,
            r#"SUBSYSTEM=="usb", RUN+="/usr/bin/env sh -c reboot""#,
        ] {
            assert!(validate(bad).is_err(), "accepted: {bad}");
        }
    }

    #[test]
    fn accepts_the_real_openrgb_1_0_rules() {
        let real = include_str!("../../../data/udev/60-openrgb.rules");
        let (clean, removed) = sanitize(real);
        assert!(removed.len() <= 16, "{removed:?}");
        let v = validate(&clean).expect("sanitized OpenRGB rules must validate");
        assert!(v.rule_count > 1000, "{}", v.rule_count);
        // /dev/i2c-* only through uaccess (the active session's ACL).
        assert!(v.text.contains(r#"KERNEL=="i2c-[0-99]*", TAG+="uaccess""#));
    }

    #[test]
    fn rejects_empty() {
        assert!(validate("# only comments\n\n").is_err());
    }

    #[test]
    fn complement_keeps_only_missing_rules() {
        let vendor = "#---\n#  Old name\n#---\nSUBSYSTEMS==\"usb|hidraw\", ATTRS{idVendor}==\"0b05\", ATTRS{idProduct}==\"1866\", TAG+=\"uaccess\", TAG+=\"Old_Name\"\n\nKERNEL==\"i2c-[0-99]*\", TAG+=\"uaccess\"\n";
        let ours = "#---\n#  New name\n#---\nSUBSYSTEMS==\"usb|hidraw\", ATTRS{idVendor}==\"0b05\", ATTRS{idProduct}==\"1866\", TAG+=\"uaccess\", TAG+=\"New_Name\"\n\n#---\n#  ASUS ROG Strix LC III\n#---\nSUBSYSTEMS==\"usb|hidraw\", ATTRS{idVendor}==\"0b05\", ATTRS{idProduct}==\"1b29\", TAG+=\"uaccess\", TAG+=\"ASUS_ROG_Strix_LC_III\"\n\nKERNEL==\"i2c-[0-99]*\", TAG+=\"uaccess\"\n";
        let (extra, n) = complement(ours, vendor);
        assert_eq!(n, 1, "{extra}");
        assert!(extra.contains("1b29") && extra.contains("ASUS ROG Strix LC III"));
        assert!(!extra.contains("1866"));
        assert!(validate(&extra).is_ok());
        let (none, n) = complement(ours, ours);
        assert_eq!((none.as_str(), n), ("", 0));
    }
}
