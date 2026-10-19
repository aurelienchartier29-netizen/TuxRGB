//! System tray (StatusNotifierItem) – KDE Plasma native, GNOME with the
//! AppIndicator extension. Pure Rust (ksni + zbus), no GTK.

use crate::i18n::t;
use crate::paths::APP_ID;
use crate::services::ServiceEvent;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, TrayMethods};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::UnboundedSender;
use tuxrgb_core::EffectKind;

#[derive(Clone, Debug, PartialEq)]
pub struct TrayState {
    pub effect: EffectKind,
    pub brightness: f32,
    pub output_enabled: bool,
    pub status: String,
    /// Interface language (the menu is rebuilt when it changes).
    pub lang: tuxrgb_core::i18n::Lang,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrayAction {
    ShowWindow,
    SetEffect(EffectKind),
    SetBrightness(f32),
    SetOutput(bool),
    OpenSettings,
    Quit,
}

/// Handle of the running icon (the Windows beta build has its own type).
pub type Handle = ksni::Handle<TuxTray>;

pub struct TuxTray {
    pub state: TrayState,
    tx: UnboundedSender<ServiceEvent>,
    /// A panel (StatusNotifierWatcher) currently shows the icon.
    online: Arc<AtomicBool>,
}

impl TuxTray {
    fn emit(&self, action: TrayAction) {
        let _ = self.tx.send(ServiceEvent::Tray(action));
    }
}

const BRIGHTNESS_STEPS: [f32; 5] = [0.1, 0.25, 0.5, 0.75, 1.0];

impl ksni::Tray for TuxTray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "TuxRGB".into()
    }

    fn category(&self) -> ksni::Category {
        // An application (next to Steam, Discord…), not a system control
        // (Wi-Fi, volume, Bluetooth): Plasma and GNOME extensions sort by it.
        ksni::Category::ApplicationStatus
    }

    fn icon_name(&self) -> String {
        // Resolved from the icon theme when installed (Flatpak / packages);
        // the embedded pixmap covers the AppImage case.
        APP_ID.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // Every size the panels ask for (22 px at 1x up to 2x scaling), so
        // the host never has to rescale the penguin.
        macro_rules! px {
            ($s:literal) => {
                ksni::Icon {
                    width: $s,
                    height: $s,
                    data: include_bytes!(concat!("../assets/tray-", $s, ".argb")).to_vec(),
                }
            };
        }
        vec![px!(22), px!(32), px!(48), px!(64), px!(128)]
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "TuxRGB".into(),
            description: if self.state.output_enabled {
                format!(
                    "{} · {} %\n{}",
                    t(self.state.effect.name()),
                    (self.state.brightness * 100.0).round(),
                    self.state.status
                )
            } else {
                format!("{}\n{}", t("Éclairage éteint"), self.state.status)
            },
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.emit(TrayAction::ShowWindow);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let effects: Vec<RadioItem> = EffectKind::ALL
            .iter()
            .map(|e| RadioItem {
                label: t(e.name()).into(),
                ..Default::default()
            })
            .collect();
        let selected_effect = EffectKind::ALL
            .iter()
            .position(|e| *e == self.state.effect)
            .unwrap_or(0);
        let brightness_items: Vec<RadioItem> = BRIGHTNESS_STEPS
            .iter()
            .map(|b| RadioItem {
                label: format!("{} %", (b * 100.0) as u32),
                ..Default::default()
            })
            .collect();
        let selected_brightness = BRIGHTNESS_STEPS
            .iter()
            .enumerate()
            .min_by(|a, b| {
                (a.1 - self.state.brightness)
                    .abs()
                    .total_cmp(&(b.1 - self.state.brightness).abs())
            })
            .map(|(i, _)| i)
            .unwrap_or(4);
        vec![
            StandardItem {
                label: t("Ouvrir TuxRGB").into(),
                icon_name: "window-new".into(),
                activate: Box::new(|t: &mut Self| t.emit(TrayAction::ShowWindow)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: t("Éclairage allumé").into(),
                checked: self.state.output_enabled,
                activate: Box::new(|t: &mut Self| {
                    t.state.output_enabled = !t.state.output_enabled;
                    t.emit(TrayAction::SetOutput(t.state.output_enabled));
                }),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: t("Effet").into(),
                icon_name: "color-management".into(),
                submenu: vec![
                    RadioGroup {
                        selected: selected_effect,
                        select: Box::new(|t: &mut Self, i| {
                            if let Some(e) = EffectKind::ALL.get(i).copied() {
                                t.state.effect = e;
                                t.emit(TrayAction::SetEffect(e));
                            }
                        }),
                        options: effects,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: t("Luminosité").into(),
                icon_name: "brightness-high".into(),
                submenu: vec![
                    RadioGroup {
                        selected: selected_brightness,
                        select: Box::new(|t: &mut Self, i| {
                            let b = BRIGHTNESS_STEPS[i.min(BRIGHTNESS_STEPS.len() - 1)];
                            t.state.brightness = b;
                            t.emit(TrayAction::SetBrightness(b));
                        }),
                        options: brightness_items,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: t("Paramètres…").into(),
                icon_name: "configure".into(),
                activate: Box::new(|t: &mut Self| t.emit(TrayAction::OpenSettings)),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: t("Quitter").into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| t.emit(TrayAction::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }

    fn watcher_online(&self) {
        tracing::info!("system tray available");
        self.online.store(true, Ordering::Release);
        let _ = self.tx.send(ServiceEvent::TrayAvailable(true));
    }

    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        tracing::info!("system tray not available (yet): {reason:?}");
        self.online.store(false, Ordering::Release);
        let _ = self.tx.send(ServiceEvent::TrayAvailable(false));
        // Keep the service alive: the panel may come back (Plasma restart)
        // or start after TuxRGB (autostart at login).
        true
    }
}

pub async fn spawn(
    state: TrayState,
    tx: UnboundedSender<ServiceEvent>,
) -> Option<ksni::Handle<TuxTray>> {
    let online = Arc::new(AtomicBool::new(true));
    let tray = TuxTray {
        state,
        tx: tx.clone(),
        online: online.clone(),
    };
    // Started at login, TuxRGB can be up before the panel: the icon is then
    // registered as soon as the panel appears (`watcher_online`) instead of
    // never. Until then "close to tray" is not offered.
    match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => {
            let _ = tx.send(ServiceEvent::TrayAvailable(online.load(Ordering::Acquire)));
            Some(handle)
        }
        Err(e) => {
            tracing::warn!("system tray unavailable: {e}");
            let _ = tx.send(ServiceEvent::TrayAvailable(false));
            None
        }
    }
}
