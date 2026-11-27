//! Restarts the computer, for the wizard's Devices step (4.0): the RAM
//! sticks only answer once the SMBus driver is loaded at boot on some
//! systems. Linux: logind's `Reboot` (Polkit asks if the session may not
//! restart on its own). Windows: `shutdown /r`, as the Start menu does.

#[cfg(not(windows))]
pub async fn reboot() -> Result<(), String> {
    let conn = zbus::Connection::system()
        .await
        .map_err(|e| e.to_string())?;
    let proxy = zbus::Proxy::new(
        &conn,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
    )
    .await
    .map_err(|e| e.to_string())?;
    tracing::info!("restarting the computer (wizard)");
    // Interactive: Polkit may ask for a password instead of refusing.
    proxy
        .call_method("Reboot", &(true,))
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(windows)]
pub async fn reboot() -> Result<(), String> {
    tracing::info!("restarting the computer (wizard)");
    let status = crate::win::hidden_command("shutdown.exe")
        .args(["/r", "/t", "0"])
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("shutdown.exe: {status}"))
    }
}
