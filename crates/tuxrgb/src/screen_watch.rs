//! Is the screen on? TuxRGB turns the LEDs off with the screens and back on
//! with them (Réglages → « Éteindre les LED avec l'écran »).
//!
//! * KDE Plasma: KWin's DPMS state of every output (`org_kde_kwin_dpms`,
//!   open to every client): exact, including the screens turned off by
//!   the lock screen.
//! * GNOME: Mutter's `PowerSaveMode` (D-Bus `org.gnome.Mutter.DisplayConfig`).
//! * Other Wayland desktops (Sway, Hyprland, niri, COSMIC…): inactivity
//!   (`ext-idle-notify-v1`, which also respects video players' inhibitors)
//!   for [`IDLE_FALLBACK`], the usual delay before a screen turns off.
//!
//! Reports `true` when every screen is off, `false` when one is back.

use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, globals};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_plasma::dpms::client::{
    org_kde_kwin_dpms::{self, OrgKdeKwinDpms},
    org_kde_kwin_dpms_manager::OrgKdeKwinDpmsManager,
};

/// Inactivity before the LEDs go off where the screen state is unknown.
pub const IDLE_FALLBACK: Duration = Duration::from_secs(10 * 60);

/// Starts watching (nothing outside a Wayland session).
pub fn spawn(tx: UnboundedSender<bool>) {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop.split(':').any(|d| d.eq_ignore_ascii_case("GNOME")) {
        tokio::spawn(gnome(tx));
        return;
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("tuxrgb-screen".into())
        .spawn(move || {
            if let Err(e) = wayland(tx) {
                tracing::info!("screen state not followed: {e}");
            }
        });
}

/// All screens off (`None` while nothing is known).
pub fn all_off(modes: impl IntoIterator<Item = u32>) -> Option<bool> {
    let mut any = false;
    for m in modes {
        if m == 0 {
            return Some(false);
        }
        any = true;
    }
    any.then_some(true)
}

struct State {
    tx: UnboundedSender<bool>,
    /// KWin's DPMS manager, when the compositor has it.
    dpms: Option<OrgKdeKwinDpmsManager>,
    /// Mode of each output (by its global name).
    modes: HashMap<u32, u32>,
    last: Option<bool>,
}

impl State {
    fn report(&mut self, off: bool) {
        if self.last != Some(off) {
            self.last = Some(off);
            tracing::info!(off, "screens");
            let _ = self.tx.send(off);
        }
    }

    fn update(&mut self) {
        if let Some(off) = all_off(self.modes.values().copied()) {
            self.report(off);
        }
    }

    /// Follows the DPMS state of one output.
    fn watch(
        &self,
        registry: &wl_registry::WlRegistry,
        name: u32,
        version: u32,
        qh: &QueueHandle<Self>,
    ) {
        if let Some(manager) = &self.dpms {
            let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
            manager.get(&output, qh, name);
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, globals::GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &globals::GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            // A screen plugged in.
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.watch(registry, name, version, qh),
            // A screen unplugged: its last state no longer counts.
            wl_registry::Event::GlobalRemove { name } if state.modes.remove(&name).is_some() => {
                state.update();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinDpmsManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &OrgKdeKwinDpmsManager,
        _: <OrgKdeKwinDpmsManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<OrgKdeKwinDpms, u32> for State {
    fn event(
        state: &mut Self,
        _: &OrgKdeKwinDpms,
        event: org_kde_kwin_dpms::Event,
        output: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            org_kde_kwin_dpms::Event::Mode { mode } => {
                state.modes.insert(*output, mode);
            }
            org_kde_kwin_dpms::Event::Done => state.update(),
            _ => {}
        }
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.report(true),
            ext_idle_notification_v1::Event::Resumed => state.report(false),
            _ => {}
        }
    }
}

fn wayland(tx: UnboundedSender<bool>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (list, mut queue) =
        globals::registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let mut state = State {
        tx,
        dpms: list.bind(&qh, 1..=1, ()).ok(),
        modes: HashMap::new(),
        last: None,
    };
    if state.dpms.is_some() {
        let outputs: Vec<(u32, u32)> = list.contents().with_list(|g| {
            g.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| (g.name, g.version))
                .collect()
        });
        for (name, version) in outputs {
            state.watch(list.registry(), name, version, &qh);
        }
        tracing::info!("screen state: KWin DPMS");
    } else {
        let notifier: ExtIdleNotifierV1 = list
            .bind(&qh, 1..=1, ())
            .map_err(|_| "ni DPMS KWin ni ext-idle-notify".to_owned())?;
        let seat: wl_seat::WlSeat = list.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
        notifier.get_idle_notification(IDLE_FALLBACK.as_millis() as u32, &seat, &qh, ());
        tracing::info!(
            minutes = IDLE_FALLBACK.as_secs() / 60,
            "screen state: inactivity"
        );
    }
    loop {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|e| e.to_string())?;
    }
}

/// GNOME: `PowerSaveMode` of Mutter (0 on, 1 standby, 2 suspend, 3 off).
async fn gnome(tx: UnboundedSender<bool>) {
    use futures::StreamExt;
    let Ok(conn) = zbus::Connection::session().await else {
        return;
    };
    let Ok(proxy) = zbus::Proxy::new(
        &conn,
        "org.gnome.Mutter.DisplayConfig",
        "/org/gnome/Mutter/DisplayConfig",
        "org.gnome.Mutter.DisplayConfig",
    )
    .await
    else {
        return;
    };
    tracing::info!("screen state: Mutter PowerSaveMode");
    let mut changes = proxy.receive_property_changed::<i32>("PowerSaveMode").await;
    let mut last = None;
    while let Some(change) = changes.next().await {
        if let Ok(mode) = change.get().await {
            let off = mode != 0;
            if last != Some(off) {
                last = Some(off);
                let _ = tx.send(off);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_must_be_off() {
        assert_eq!(all_off([]), None);
        assert_eq!(all_off([3]), Some(true));
        assert_eq!(all_off([3, 2]), Some(true));
        assert_eq!(all_off([3, 0]), Some(false));
    }
}
