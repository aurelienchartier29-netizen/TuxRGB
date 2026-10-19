//! Administrator actions on Windows (beta build): the same interface as the
//! Linux Polkit runner, carried out by `tuxrgb.exe --windows-setup` started
//! with the UAC prompt (see `win::elevation`).

use crate::win::elevation::{self, Elevated};

#[derive(Clone, Debug, PartialEq)]
pub struct HelperReport {
    pub steps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HelperError {
    /// The user said no to the UAC prompt.
    Cancelled,
    /// Never on Windows (kept for the shared interface code).
    #[allow(dead_code)]
    NoPolkit,
    #[allow(dead_code)]
    NotFound,
    /// The change cannot be made automatically here; the text says how.
    #[allow(dead_code)]
    Manual(String),
    Failed(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str(crate::i18n::t("autorisation administrateur refusée")),
            Self::NoPolkit | Self::NotFound => f.write_str(crate::i18n::t(
                "le programme d'installation de TuxRGB est incomplet",
            )),
            Self::Manual(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

fn parse(code: i32, text: &str) -> Result<HelperReport, HelperError> {
    let steps: Vec<String> = text
        .lines()
        .filter_map(|l| l.strip_prefix("STEP ").map(str::to_owned))
        .collect();
    if code == elevation::EXIT_OK && text.contains("RESULT ok") {
        return Ok(HelperReport { steps });
    }
    let msg = text
        .lines()
        .find_map(|l| l.strip_prefix("RESULT error "))
        .map(str::to_owned)
        .unwrap_or_else(|| crate::i18n::trf!("échec (code {})", code));
    Err(HelperError::Failed(msg))
}

/// Runs an administrator action: `install` (PawnIO + administrator mode),
/// `reset` (removes administrator mode), `enable-smbus` (nothing to do on
/// Windows). The udev rules given by the shared code are not used.
pub async fn run_helper(action: &str, _stdin: Option<String>) -> Result<HelperReport, HelperError> {
    let action = action.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        if elevation::is_elevated() {
            // Already elevated: no prompt needed.
            match elevation::run_action(&action) {
                Ok(steps) => Ok(HelperReport { steps }),
                Err(e) => Err(HelperError::Failed(e)),
            }
        } else {
            match elevation::run_elevated(&action) {
                Elevated::Done(code, text) => parse(code, &text),
                Elevated::Cancelled => Err(HelperError::Cancelled),
                Elevated::Failed(e) => Err(HelperError::Failed(e)),
            }
        }
    })
    .await
    .map_err(|e| HelperError::Failed(e.to_string()))?;
    if result.is_ok() {
        // Administrator mode just configured: hand over when appropriate.
        elevation::mark_handover_pending();
    }
    result
}

/// Instructions when the automatic set-up is impossible.
pub fn manual_commands() -> String {
    crate::i18n::t(
        "Installez PawnIO (https://pawnio.eu), puis lancez TuxRGB avec « Exécuter en tant qu'administrateur ».",
    )
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_are_parsed() {
        let ok = parse(0, "STEP pawnio-present\nSTEP elevated-task\nRESULT ok\n").unwrap();
        assert_eq!(ok.steps, vec!["pawnio-present", "elevated-task"]);
        assert!(matches!(
            parse(1, "RESULT error schtasks : accès refusé\n"),
            Err(HelperError::Failed(m)) if m.contains("accès refusé")
        ));
    }
}
