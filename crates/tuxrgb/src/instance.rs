//! Single instance over the D-Bus session bus.
//!
//! The first TuxRGB owns the bus name `io.github.tuxrgb.TuxRGB` and serves
//! `io.github.tuxrgb.TuxRGB1.Activate`. A second launch (application menu,
//! desktop action, autostart…) finds the name taken, asks the running
//! instance to show its window, and exits immediately: there is never more
//! than one TuxRGB – and one OpenRGB server – driving the hardware.

use crate::paths::APP_ID;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

const OBJECT_PATH: &str = "/io/github/tuxrgb/TuxRGB";

/// What a second launch asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivateRequest {
    /// `--reset-hardware`: show the hardware access wizard.
    pub reset_hardware: bool,
    /// `--minimized` (autostart): nothing to show.
    pub minimized: bool,
}

pub enum Role {
    /// This process is the instance (or the session bus is unavailable).
    Primary,
    /// Another TuxRGB is running and has been asked to come forward.
    Secondary,
}

static ACTIVATIONS: OnceLock<Mutex<Option<mpsc::UnboundedReceiver<ActivateRequest>>>> =
    OnceLock::new();

/// Activation requests received by the primary instance (taken once by the
/// services worker).
pub fn take_activations() -> Option<mpsc::UnboundedReceiver<ActivateRequest>> {
    ACTIVATIONS.get()?.lock().ok()?.take()
}

struct Instance {
    tx: mpsc::UnboundedSender<ActivateRequest>,
}

#[zbus::interface(name = "io.github.tuxrgb.TuxRGB1")]
impl Instance {
    /// Shows the window of the running instance.
    async fn activate(&self, reset_hardware: bool, minimized: bool) {
        tracing::info!(
            reset_hardware,
            minimized,
            "activation requested by a new launch"
        );
        let _ = self.tx.send(ActivateRequest {
            reset_hardware,
            minimized,
        });
    }
}

#[zbus::proxy(
    interface = "io.github.tuxrgb.TuxRGB1",
    default_service = "io.github.tuxrgb.TuxRGB",
    default_path = "/io/github/tuxrgb/TuxRGB"
)]
trait InstanceClient {
    fn activate(&self, reset_hardware: bool, minimized: bool) -> zbus::Result<()>;
}

/// Claims the instance, or forwards `request` to the running one.
///
/// Runs its own D-Bus thread for the whole life of the process (the
/// connection must stay open to keep the name).
pub fn claim(request: ActivateRequest) -> Role {
    let (role_tx, role_rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("tuxrgb-instance".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => {
                    let _ = role_tx.send(Role::Primary);
                    return;
                }
            };
            rt.block_on(async move {
                match serve_or_forward(request).await {
                    Ok(Some(conn)) => {
                        let _ = role_tx.send(Role::Primary);
                        // Keep the connection (and the bus name) alive.
                        let _keep = conn;
                        std::future::pending::<()>().await;
                    }
                    Ok(None) => {
                        let _ = role_tx.send(Role::Secondary);
                    }
                    Err(e) => {
                        tracing::warn!("single instance unavailable (D-Bus: {e})");
                        let _ = role_tx.send(Role::Primary);
                    }
                }
            });
        });
    if spawned.is_err() {
        return Role::Primary;
    }
    role_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap_or(Role::Primary)
}

async fn serve_or_forward(request: ActivateRequest) -> zbus::Result<Option<zbus::Connection>> {
    let conn = zbus::Connection::session().await?;
    let (tx, rx) = mpsc::unbounded_channel();
    // Object first, name second: a launch racing us never finds the name
    // without the method behind it.
    conn.object_server()
        .at(OBJECT_PATH, Instance { tx })
        .await?;
    let reply = conn
        .request_name_with_flags(APP_ID, zbus::fdo::RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner)
        | Ok(zbus::fdo::RequestNameReply::AlreadyOwner) => {
            let _ = ACTIVATIONS.set(Mutex::new(Some(rx)));
            Ok(Some(conn))
        }
        Ok(_) | Err(zbus::Error::NameTaken) => {
            // Another TuxRGB owns the hardware: never start a second one,
            // even if it does not answer (it is busy, not gone – the bus
            // releases the name as soon as a process exits).
            let proxy = InstanceClientProxy::new(&conn).await?;
            match tokio::time::timeout(
                Duration::from_secs(2),
                proxy.activate(request.reset_hardware, request.minimized),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("the running TuxRGB refused activation: {e}"),
                Err(_) => tracing::warn!("the running TuxRGB did not answer"),
            }
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
